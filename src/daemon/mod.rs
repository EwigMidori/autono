use std::time::Duration;

use tokio::time;
use tracing::{error, info, warn};

use crate::config::{Config, TargetConfig};
use crate::error::Result;
use crate::git_workspace::{GitWorkspace, WorkspaceManager};
use crate::github::{GitHub, ProjectItem};
use crate::runner::codex::{AgentRunner, CodexRunner};
use crate::store::{Store, StoreItemKey, StoredItemBuilder};
use crate::workflow::{
    BotMentionPolicy, CommentThread, ItemView, ManagedState, ReviewDecision, WorkflowAction,
    WorkflowPolicy,
};

mod actions;
mod comments;
mod effects;
mod gates;
mod requests;

pub(crate) use comments::CommentComposer;
use requests::{DiscussionRequest, SelfReviewRequest, TriageRequest, WorkRequest};

#[non_exhaustive]
pub struct Daemon<G, R = CodexRunner, W = GitWorkspace> {
    config: Config,
    github: G,
    runner: R,
    store: Store,
    workspace: W,
    mention_policy: BotMentionPolicy,
    comment_composer: CommentComposer,
}

impl<G: GitHub> Daemon<G, CodexRunner, GitWorkspace> {
    pub fn new(config: Config, github: G) -> Result<Self> {
        let runner = CodexRunner;
        let workspace = GitWorkspace::new(config.worktrees_root.clone());
        Self::with_components(config, github, runner, workspace)
    }
}

impl<G: GitHub, R: AgentRunner, W: WorkspaceManager> Daemon<G, R, W> {
    pub fn with_components(config: Config, github: G, runner: R, workspace: W) -> Result<Self> {
        let store = Store::open(config.state_path())?;
        let mention_policy = BotMentionPolicy::new(&config.bot_login);
        Ok(Self {
            config,
            github,
            runner,
            store,
            workspace,
            mention_policy,
            comment_composer: CommentComposer::default(),
        })
    }

    pub async fn run_forever(&self) -> Result<()> {
        let mut interval = time::interval(Duration::from_secs(self.config.poll_interval_secs));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut shutdown = Box::pin(tokio::signal::ctrl_c());
        let mut listen_for_shutdown = true;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.run_once().await {
                        error!(error = ?err, "poll failed");
                    }
                }
                signal = &mut shutdown, if listen_for_shutdown => {
                    match signal {
                        Ok(()) => {
                            info!("shutting down");
                            return Ok(());
                        }
                        Err(err) => {
                            error!(error = ?err, "ctrl-c listener failed; continuing without signal handling");
                            listen_for_shutdown = false;
                        }
                    }
                }
            }
        }
    }

    pub async fn run_once(&self) -> Result<()> {
        for target in &self.config.targets {
            if let Err(err) = self.poll_target(target).await {
                error!(repo = target.full_name(), error = ?err, "target poll failed");
            }
        }
        Ok(())
    }

    async fn poll_target(&self, target: &TargetConfig) -> Result<()> {
        info!(repo = target.full_name(), "polling target");
        let items = self.github.list_project_items(target).await?;
        for item in items {
            if let Err(err) = self.handle_item(target, item).await {
                warn!(repo = target.full_name(), error = ?err, "item handling failed");
            }
        }
        Ok(())
    }

    async fn handle_item(&self, target: &TargetConfig, item: ProjectItem) -> Result<()> {
        let content = match &item.content {
            Some(content) => content.clone(),
            None => return Ok(()),
        };
        let comments = self.github.list_comments(target, &content).await?;
        let thread = CommentThread::from(comments);
        let stored = self
            .store
            .get_item(&target.owner, &target.repo, &item.id)?
            .or_else(|| {
                thread.latest_marker_state().map(|marker| {
                    let mut stored = StoredItemBuilder::default()
                        .owner(&target.owner)
                        .repo(&target.repo)
                        .item_id(&item.id)
                        .state(marker.state)
                        .build()
                        .expect("stored item builder was missing required fields after explicit initialization");
                    stored.last_comment_id = Some(marker.comment_id);
                    stored.branch = marker.branch;
                    stored.pr_number = marker.pr_number;
                    stored
                })
            });
        let mention = self.admin_mention(target, &thread).await?;
        let has_admin_mention = mention.is_some();
        let last_seen_comment_id = stored.as_ref().and_then(|item| item.last_comment_id);
        let latest_human_comment_id = thread.latest_human_comment_id(&self.config.bot_login);
        let has_new_human_comment = latest_human_comment_id
            .map(|latest| {
                last_seen_comment_id
                    .map(|last_seen| latest > last_seen)
                    .unwrap_or(true)
            })
            .unwrap_or(false);

        let candidate_branch = stored
            .as_ref()
            .and_then(|stored| stored.branch.clone())
            .or_else(|| {
                stored.as_ref().map(|_| {
                    self.workspace
                        .identity(target, &item.id, &item.title)
                        .branch
                })
            });
        let pr = match candidate_branch.as_deref() {
            Some(branch) => self.github.find_agent_pr(target, branch).await?,
            None => None,
        };
        let active_pr = pr.as_ref().filter(|pr| !pr.merged);
        let latest_review_id = active_pr.and_then(|pr| pr.latest_review_id);
        let latest_review_body = active_pr.and_then(|pr| pr.latest_review_body.clone());
        let pr_number = active_pr.map(|pr| pr.number);
        let has_unhandled_review = latest_review_id.is_some()
            && latest_review_id != stored.as_ref().and_then(|item| item.last_review_id);
        let stored_state = stored.as_ref().map(|stored| stored.state);
        let managed_state = if active_pr.is_some() {
            match stored_state {
                Some(ManagedState::Reviewing | ManagedState::ReviewPending) => stored_state,
                Some(ManagedState::Done | ManagedState::Blocked) => stored_state,
                _ => Some(ManagedState::PrOpen),
            }
        } else {
            match stored_state {
                Some(
                    ManagedState::PrOpen | ManagedState::Reviewing | ManagedState::ReviewPending,
                ) => Some(ManagedState::Working),
                _ => stored_state,
            }
        };

        let view = ItemView {
            managed_state,
            project_status: item.status.clone(),
            has_admin_mention,
            has_new_human_comment,
            has_pr: active_pr.is_some(),
            pr_merged: pr.as_ref().map(|pr| pr.merged).unwrap_or(false),
            review_decision: active_pr
                .map(|pr| pr.review_decision)
                .unwrap_or(ReviewDecision::None),
            has_unhandled_review,
        };
        let policy = WorkflowPolicy::new(target.workflow.clone());
        let action = policy.decide_next_action(&view);
        let store_key = StoreItemKey::new(&target.owner, &target.repo, &item.id);
        match action {
            WorkflowAction::Ignore
            | WorkflowAction::WaitForStart
            | WorkflowAction::WaitForReview => Ok(()),
            WorkflowAction::RunSelfReview => {
                let Some(pr) = active_pr else {
                    return Ok(());
                };
                self.run_existing_self_review(
                    WorkRequest {
                        target,
                        item: &item,
                        content: &content,
                        thread: &thread,
                        stored,
                        post_work_state: ManagedState::PrOpen,
                        handled_review_id: None,
                        review_body: None,
                        pr_number: Some(pr.number),
                    },
                    pr,
                )
                .await
            }
            WorkflowAction::Triage => {
                self.triage(TriageRequest {
                    target,
                    item: &item,
                    content: &content,
                    thread: &thread,
                    latest_comment_id: latest_human_comment_id,
                })
                .await
            }
            WorkflowAction::MonitorDiscussion => {
                self.monitor_discussion(DiscussionRequest {
                    target,
                    item: &item,
                    content: &content,
                    thread: &thread,
                    state: managed_state.unwrap_or(ManagedState::Blocked),
                    latest_comment_id: latest_human_comment_id,
                })
                .await
            }
            WorkflowAction::StartWork => {
                self.start_or_continue_work(WorkRequest {
                    target,
                    item: &item,
                    content: &content,
                    thread: &thread,
                    stored,
                    post_work_state: ManagedState::PrOpen,
                    handled_review_id: None,
                    review_body: None,
                    pr_number,
                })
                .await
            }
            WorkflowAction::ApplyReviewFeedback => {
                self.start_or_continue_work(WorkRequest {
                    target,
                    item: &item,
                    content: &content,
                    thread: &thread,
                    stored,
                    post_work_state: ManagedState::ReviewPending,
                    handled_review_id: latest_review_id,
                    review_body: latest_review_body,
                    pr_number,
                })
                .await
            }
            WorkflowAction::WaitForMerge => {
                self.store
                    .mark_review_handled(store_key, latest_review_id)?;
                self.store.mark_state(
                    store_key,
                    ManagedState::ReviewPending,
                    latest_human_comment_id,
                )?;
                Ok(())
            }
            WorkflowAction::Complete => {
                self.complete(target, &item, &content).await;
                Ok(())
            }
        }
    }
}
