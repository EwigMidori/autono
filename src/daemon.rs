use std::path::PathBuf;
use std::time::Duration;

use tokio::time;
use tracing::{error, info, warn};

use crate::codex_runner::{
    AgentRunner, CodexRunner, ImplementationPrompt, TriagePrompt, ValidationRunner,
};
use crate::config::{Config, TargetConfig};
use crate::error::{Result, ResultContext};
use crate::git_workspace::{GitWorkspace, WorkIdentity, WorkspaceManager};
use crate::github::{GitHub, NewPullRequest, ProjectContent, ProjectItem};
use crate::store::{Store, StoredItem};
use crate::workflow::{
    AdminMention, BotMentionPolicy, CommentThread, CommentView, ItemView, ManagedState,
    ReforgeMarker, ReviewDecision, TriageResult, WorkflowAction, WorkflowPolicy,
};

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

#[non_exhaustive]
#[derive(Debug)]
struct WorkRequest<'a> {
    target: &'a TargetConfig,
    item: &'a ProjectItem,
    content: &'a ProjectContent,
    thread: &'a CommentThread,
    stored: Option<StoredItem>,
    post_work_state: ManagedState,
    handled_review_id: Option<i64>,
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
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.run_once().await {
                        error!(error = ?err, "poll failed");
                    }
                }
                signal = tokio::signal::ctrl_c() => {
                    signal.context("failed to listen for ctrl-c")?;
                    info!("shutting down");
                    return Ok(());
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
                    let mut stored =
                        StoredItem::new(&target.owner, &target.repo, &item.id, marker.state);
                    stored.last_comment_id = Some(marker.comment_id);
                    stored
                })
            });
        let mention = self.admin_mention(target, &thread).await?;
        let has_admin_mention = mention.is_some();
        let last_seen_comment_id = stored.as_ref().and_then(|item| item.last_comment_id);
        let latest_human_comment_id = thread.latest_human_comment_id(&self.config.bot_login);
        let has_new_human_comment =
            thread.has_new_human_comment_since(last_seen_comment_id, &self.config.bot_login);

        let pr = match stored.as_ref().and_then(|stored| stored.branch.clone()) {
            Some(branch) => self.github.find_agent_pr(target, &branch).await?,
            None => None,
        };
        let latest_review_id = pr.as_ref().and_then(|pr| pr.latest_review_id);
        let has_unhandled_review = latest_review_id.is_some()
            && latest_review_id != stored.as_ref().and_then(|item| item.last_review_id);

        let view = ItemView {
            managed_state: stored.as_ref().map(|stored| stored.state),
            project_status: item.status.clone(),
            has_admin_mention,
            has_new_human_comment,
            has_pr: pr.is_some() || stored.as_ref().and_then(|item| item.pr_number).is_some(),
            pr_merged: pr.as_ref().map(|pr| pr.merged).unwrap_or(false),
            review_decision: pr
                .as_ref()
                .map(|pr| pr.review_decision)
                .unwrap_or(ReviewDecision::None),
            has_unhandled_review,
        };
        let policy = WorkflowPolicy::new(target.workflow.clone());
        let action = policy.decide_next_action(&view);
        match action {
            WorkflowAction::Ignore
            | WorkflowAction::WaitForStart
            | WorkflowAction::WaitForReview => Ok(()),
            WorkflowAction::Triage => {
                self.triage(target, &item, &content, &thread, latest_human_comment_id)
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
                })
                .await
            }
            WorkflowAction::WaitForMerge => {
                self.store.mark_review_handled(
                    &target.owner,
                    &target.repo,
                    &item.id,
                    latest_review_id,
                )?;
                self.store.mark_state(
                    &target.owner,
                    &target.repo,
                    &item.id,
                    ManagedState::ReviewPending,
                    latest_human_comment_id,
                )?;
                Ok(())
            }
            WorkflowAction::Complete => self.complete(target, &item, &content).await,
        }
    }

    async fn admin_mention(
        &self,
        target: &TargetConfig,
        thread: &CommentThread,
    ) -> Result<Option<AdminMention>> {
        let mut admins = std::collections::HashMap::<String, bool>::new();
        for comment in thread.comments() {
            if comment.author.is_empty()
                || admins.contains_key(&comment.author)
                || !self.mention_policy.contains_mention(&comment.body)
            {
                continue;
            }
            let allowed = self
                .github
                .user_can_administer_or_write(target, &comment.author)
                .await
                .with_context(|| {
                    format!(
                        "failed to check repository permission for {}",
                        comment.author
                    )
                })?;
            admins.insert(comment.author.clone(), allowed);
        }
        Ok(thread.latest_admin_mention(&self.mention_policy, |login| {
            admins.get(login).copied().unwrap_or(false)
        }))
    }

    async fn triage(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        content: &ProjectContent,
        thread: &CommentThread,
        latest_comment_id: Option<i64>,
    ) -> Result<()> {
        let discussion = self.comment_composer.discussion_text(thread.comments());
        let prompt = TriagePrompt::new(&content.title, &content.body, &discussion).render();
        let result = self
            .runner
            .triage(&target.commands, &target.checkout_path, &prompt)
            .await?;
        let (state, body) =
            self.comment_composer
                .triage_comment(item, &result, &target.workflow.start_status);
        self.github
            .create_issue_comment(target, content.number, &body)
            .await?;
        match state {
            ManagedState::AwaitingStart => {
                self.github
                    .set_project_status(target, item, &target.workflow.triaged_status)
                    .await?;
            }
            ManagedState::Blocked => {
                self.github
                    .set_project_status(target, item, &target.workflow.blocked_status)
                    .await?;
            }
            _ => {}
        }
        self.store.mark_state(
            &target.owner,
            &target.repo,
            &item.id,
            state,
            latest_comment_id,
        )?;
        Ok(())
    }

    async fn start_or_continue_work(&self, work: WorkRequest<'_>) -> Result<()> {
        let identity = work
            .stored
            .as_ref()
            .and_then(|stored| stored.branch.as_ref().zip(stored.worktree_path.as_ref()))
            .map(|(branch, path)| WorkIdentity {
                branch: branch.clone(),
                worktree_path: PathBuf::from(path),
            })
            .unwrap_or_else(|| {
                self.workspace
                    .identity(work.target, &work.item.id, &work.item.title)
            });
        self.workspace
            .ensure_worktree(work.target, &identity)
            .await?;
        self.store.attach_work(
            &work.target.owner,
            &work.target.repo,
            &work.item.id,
            &identity.branch,
            &identity.worktree_path.to_string_lossy(),
        )?;

        let discussion = self
            .comment_composer
            .discussion_text(work.thread.comments());
        let prompt =
            ImplementationPrompt::new(&work.content.title, &discussion, &work.target.commands.test);
        let prompt_text = prompt.render();
        let validation = ValidationRunner::new(&identity.worktree_path, &work.target.commands.test);
        let mut last_error = None;
        for attempt in 0..=work.target.commands.max_fix_attempts {
            if attempt == 0 {
                self.runner
                    .implement(&work.target.commands, &identity.worktree_path, &prompt_text)
                    .await?;
            } else {
                let repair_prompt = prompt.render_repair(last_error.as_deref().unwrap_or(""));
                self.runner
                    .implement(
                        &work.target.commands,
                        &identity.worktree_path,
                        &repair_prompt,
                    )
                    .await?;
            }

            match validation.run().await {
                Ok(_) => {
                    last_error = None;
                    break;
                }
                Err(err) => {
                    last_error = Some(format!("{err:#}"));
                }
            }
        }
        if let Some(err) = last_error {
            self.github
                .create_issue_comment(
                    work.target,
                    work.content.number,
                    &self
                        .comment_composer
                        .blocked_validation_comment(work.item, &err),
                )
                .await?;
            self.github
                .set_project_status(work.target, work.item, &work.target.workflow.blocked_status)
                .await?;
            self.store.mark_state(
                &work.target.owner,
                &work.target.repo,
                &work.item.id,
                ManagedState::Blocked,
                work.thread
                    .comments()
                    .iter()
                    .map(|comment| comment.id)
                    .max(),
            )?;
            return Ok(());
        }

        let committed = self
            .workspace
            .commit_all(
                &identity.worktree_path,
                &format!("Implement {}", work.content.title),
            )
            .await?;
        if committed {
            self.workspace
                .push(&identity.worktree_path, &identity.branch)
                .await?;
        }

        let pr = match self
            .github
            .find_agent_pr(work.target, &identity.branch)
            .await?
        {
            Some(pr) => pr,
            None if !committed => {
                self.github
                    .create_issue_comment(
                        work.target,
                        work.content.number,
                        &self.comment_composer.no_changes_comment(work.item),
                    )
                    .await?;
                self.github
                    .set_project_status(
                        work.target,
                        work.item,
                        &work.target.workflow.blocked_status,
                    )
                    .await?;
                self.store.mark_state(
                    &work.target.owner,
                    &work.target.repo,
                    &work.item.id,
                    ManagedState::Blocked,
                    work.thread
                        .comments()
                        .iter()
                        .map(|comment| comment.id)
                        .max(),
                )?;
                return Ok(());
            }
            None => {
                let pr = self
                    .github
                    .create_pull_request(
                        work.target,
                        &NewPullRequest {
                            title: work.content.title.clone(),
                            body: self.comment_composer.pr_body(work.item, work.content),
                            head: identity.branch.clone(),
                            base: work.target.base_branch.clone(),
                        },
                    )
                    .await?;
                self.github
                    .request_reviewers(work.target, pr.number, &work.target.review.reviewers)
                    .await?;
                pr
            }
        };
        self.store.attach_pr(
            &work.target.owner,
            &work.target.repo,
            &work.item.id,
            pr.number,
        )?;
        self.store.mark_state(
            &work.target.owner,
            &work.target.repo,
            &work.item.id,
            work.post_work_state,
            work.thread.latest_human_comment_id(&self.config.bot_login),
        )?;
        if work.post_work_state == ManagedState::ReviewPending {
            self.store.mark_review_handled(
                &work.target.owner,
                &work.target.repo,
                &work.item.id,
                work.handled_review_id,
            )?;
        }
        self.github
            .set_project_status(work.target, work.item, &work.target.workflow.review_status)
            .await?;
        Ok(())
    }

    async fn complete(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        content: &ProjectContent,
    ) -> Result<()> {
        self.github
            .set_project_status(target, item, &target.workflow.done_status)
            .await?;
        self.github
            .create_issue_comment(
                target,
                content.number,
                &self.comment_composer.completion_comment(item),
            )
            .await?;
        self.store.mark_state(
            &target.owner,
            &target.repo,
            &item.id,
            ManagedState::Done,
            None,
        )?;
        Ok(())
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct CommentComposer {
    blocked_output_limit: usize,
}

impl Default for CommentComposer {
    fn default() -> Self {
        Self {
            blocked_output_limit: 6000,
        }
    }
}

impl CommentComposer {
    pub(crate) fn triage_comment(
        &self,
        item: &ProjectItem,
        result: &TriageResult,
        start_status: &str,
    ) -> (ManagedState, String) {
        if !result.is_code_change {
            return (
                ManagedState::Blocked,
                format!(
                    "{}\n\nI do not think this is a code-change task.\n\nSummary: {}",
                    self.marker(item, ManagedState::Blocked),
                    result.summary
                ),
            );
        }
        if result.needs_clarification() {
            let questions = self.clarification_questions(result);
            return (
                ManagedState::Blocked,
                format!(
                    "{}\n\nI need clarification before implementation.\n\nSummary: {}\n\nQuestions:\n{}",
                    self.marker(item, ManagedState::Blocked),
                    result.summary,
                    questions
                ),
            );
        }
        (
            ManagedState::AwaitingStart,
            format!(
                "{}\n\nI can implement this code-change request.\n\nSummary: {}\n\nMove this Project item to `{}` when it should start.",
                self.marker(item, ManagedState::AwaitingStart),
                result.summary,
                start_status
            ),
        )
    }

    pub(crate) fn discussion_text(&self, comments: &[CommentView]) -> String {
        comments
            .iter()
            .map(|comment| {
                format!(
                    "### {}\n{}\n",
                    if comment.author.is_empty() {
                        "issue body"
                    } else {
                        &comment.author
                    },
                    comment.body
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn blocked_validation_comment(&self, item: &ProjectItem, err: &str) -> String {
        format!(
            "{}\n\nImplementation is blocked after validation retries:\n\n```text\n{}\n```",
            self.marker(item, ManagedState::Blocked),
            self.truncate(err)
        )
    }

    pub(crate) fn no_changes_comment(&self, item: &ProjectItem) -> String {
        format!(
            "{}\n\nImplementation produced no repository changes, so no pull request was opened.",
            self.marker(item, ManagedState::Blocked)
        )
    }

    pub(crate) fn pr_body(&self, item: &ProjectItem, content: &ProjectContent) -> String {
        format!(
            "{}\n\nImplements GitHub Project item `{}`.\n\nSource discussion: {}\n",
            self.marker(item, ManagedState::PrOpen),
            item.id,
            content.url
        )
    }

    pub(crate) fn completion_comment(&self, item: &ProjectItem) -> String {
        format!(
            "{}\n\nThe linked pull request has been merged. Marking this task complete.",
            self.marker(item, ManagedState::Done)
        )
    }

    fn marker(&self, item: &ProjectItem, state: ManagedState) -> String {
        ReforgeMarker::new(&item.id, state).render()
    }

    fn clarification_questions(&self, result: &TriageResult) -> String {
        if result.questions.is_empty() {
            "- Please clarify the expected code change.".to_string()
        } else {
            result
                .questions
                .iter()
                .map(|question| format!("- {question}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    fn truncate(&self, input: &str) -> String {
        if input.len() <= self.blocked_output_limit {
            input.to_string()
        } else {
            let end = input
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= self.blocked_output_limit)
                .last()
                .unwrap_or(0);
            format!("{}...", &input[..end])
        }
    }
}
