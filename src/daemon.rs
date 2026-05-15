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
use crate::review_feedback::{FeedbackRequest, ReviewFeedbackComposer};
use crate::store::{Store, StoreItemKey, StoredItem, StoredItemBuilder};
use crate::workflow::{
    AdminMention, AutonoMarker, BotMentionPolicy, CommentThread, CommentView, ItemView,
    ManagedState, ReviewDecision, TriageResult, WorkflowAction, WorkflowPolicy,
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

#[derive(Debug)]
struct WorkRequest<'a> {
    target: &'a TargetConfig,
    item: &'a ProjectItem,
    content: &'a ProjectContent,
    thread: &'a CommentThread,
    stored: Option<StoredItem>,
    post_work_state: ManagedState,
    handled_review_id: Option<i64>,
    review_body: Option<String>,
    pr_number: Option<i64>,
}

#[derive(Debug)]
struct TriageRequest<'a> {
    target: &'a TargetConfig,
    item: &'a ProjectItem,
    content: &'a ProjectContent,
    thread: &'a CommentThread,
    latest_comment_id: Option<i64>,
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
        let has_new_admin_mention = mention
            .as_ref()
            .map(|mention| {
                last_seen_comment_id
                    .map(|last_seen| mention.comment_id > last_seen)
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        let latest_human_comment_id = thread.latest_human_comment_id(&self.config.bot_login);

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
                Some(ManagedState::ReviewPending) => Some(ManagedState::ReviewPending),
                Some(ManagedState::Done | ManagedState::Blocked) => stored_state,
                _ => Some(ManagedState::PrOpen),
            }
        } else {
            match stored_state {
                Some(ManagedState::PrOpen | ManagedState::ReviewPending) => {
                    Some(ManagedState::Working)
                }
                _ => stored_state,
            }
        };

        let view = ItemView {
            managed_state,
            project_status: item.status.clone(),
            has_admin_mention,
            has_new_admin_mention,
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

    async fn triage(&self, request: TriageRequest<'_>) -> Result<()> {
        let discussion = self
            .comment_composer
            .discussion_text(request.thread.comments());
        let prompt =
            TriagePrompt::new(&request.content.title, &request.content.body, &discussion).render();
        let result = self
            .runner
            .triage(
                &request.target.commands,
                &request.target.checkout_path,
                &prompt,
            )
            .await?;
        let (state, body) = self.comment_composer.triage_comment(
            request.item,
            &result,
            &request.target.workflow.start_status,
        );
        self.store.mark_state(
            StoreItemKey::new(
                &request.target.owner,
                &request.target.repo,
                &request.item.id,
            ),
            state,
            request.latest_comment_id,
        )?;
        self.try_create_issue_comment(request.target, request.content.number, &body)
            .await;
        match state {
            ManagedState::AwaitingStart => {
                self.try_set_project_status(
                    request.target,
                    request.item,
                    &request.target.workflow.triaged_status,
                )
                .await;
            }
            ManagedState::Blocked => {
                self.try_set_project_status(
                    request.target,
                    request.item,
                    &request.target.workflow.blocked_status,
                )
                .await;
            }
            _ => {}
        }
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
        let store_key = StoreItemKey::new(&work.target.owner, &work.target.repo, &work.item.id);
        self.store.attach_work(
            store_key,
            &identity.branch,
            &identity.worktree_path.to_string_lossy(),
        )?;

        let feedback = ReviewFeedbackComposer::default();
        let feedback_request = FeedbackRequest::new(
            work.target,
            work.post_work_state,
            work.pr_number,
            work.review_body.as_deref(),
        );
        let review_feedback = feedback
            .trusted_feedback_for_state(&self.github, feedback_request)
            .await?;
        let discussion = self
            .comment_composer
            .discussion_text(work.thread.comments());
        let discussion = feedback.prepend_to_discussion(&review_feedback, &discussion);
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
            self.store.mark_state(
                store_key,
                ManagedState::Blocked,
                work.thread
                    .comments()
                    .iter()
                    .map(|comment| comment.id)
                    .max(),
            )?;
            self.try_create_issue_comment(
                work.target,
                work.content.number,
                &self
                    .comment_composer
                    .blocked_validation_comment(work.item, &err),
            )
            .await;
            self.try_set_project_status(
                work.target,
                work.item,
                &work.target.workflow.blocked_status,
            )
            .await;
            return Ok(());
        }

        let committed = self
            .workspace
            .commit_all(
                &identity.worktree_path,
                &format!("Implement {}", work.content.title),
            )
            .await?;
        let branch_has_changes = committed
            || self
                .workspace
                .has_diff_against_base(&identity.worktree_path, work.target.base_branch())
                .await?;
        if branch_has_changes {
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
            None if !branch_has_changes => {
                self.store.mark_state(
                    store_key,
                    ManagedState::Blocked,
                    work.thread
                        .comments()
                        .iter()
                        .map(|comment| comment.id)
                        .max(),
                )?;
                self.try_create_issue_comment(
                    work.target,
                    work.content.number,
                    &self.comment_composer.no_changes_comment(work.item),
                )
                .await;
                self.try_set_project_status(
                    work.target,
                    work.item,
                    &work.target.workflow.blocked_status,
                )
                .await;
                return Ok(());
            }
            None => {
                self.github
                    .create_pull_request(
                        work.target,
                        &NewPullRequest {
                            title: work.content.title.clone(),
                            body: self.comment_composer.pr_body(work.item, work.content),
                            head: identity.branch.clone(),
                            base: work.target.base_branch.clone(),
                        },
                    )
                    .await?
            }
        };
        self.store.attach_pr(store_key, pr.number)?;
        self.store.mark_state(
            store_key,
            work.post_work_state,
            work.thread.latest_human_comment_id(&self.config.bot_login),
        )?;
        if work.post_work_state == ManagedState::ReviewPending {
            self.store
                .mark_review_handled(store_key, work.handled_review_id)?;
        }
        self.try_create_issue_comment(
            work.target,
            work.content.number,
            &self.comment_composer.pr_progress_comment(
                work.item,
                work.post_work_state,
                &identity.branch,
                pr.number,
            ),
        )
        .await;
        self.try_request_reviewers(work.target, pr.number, &work.target.review.reviewers)
            .await;
        self.try_set_project_status(work.target, work.item, &work.target.workflow.review_status)
            .await;
        Ok(())
    }

    async fn complete(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        content: &ProjectContent,
    ) -> Result<()> {
        self.store.mark_state(
            StoreItemKey::new(&target.owner, &target.repo, &item.id),
            ManagedState::Done,
            None,
        )?;
        self.try_set_project_status(target, item, &target.workflow.done_status)
            .await;
        self.try_create_issue_comment(
            target,
            content.number,
            &self.comment_composer.completion_comment(item),
        )
        .await;
        Ok(())
    }

    async fn try_create_issue_comment(&self, target: &TargetConfig, issue_number: i64, body: &str) {
        if let Err(err) = self
            .github
            .create_issue_comment(target, issue_number, body)
            .await
        {
            warn!(
                repo = target.full_name(),
                issue_number,
                error = ?err,
                "failed to create issue comment"
            );
        }
    }

    async fn try_set_project_status(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        status: &str,
    ) {
        if let Err(err) = self.github.set_project_status(target, item, status).await {
            warn!(
                repo = target.full_name(),
                item_id = item.id,
                status,
                error = ?err,
                "failed to set project status"
            );
        }
    }

    async fn try_request_reviewers(
        &self,
        target: &TargetConfig,
        pr_number: i64,
        reviewers: &[String],
    ) {
        if let Err(err) = self
            .github
            .request_reviewers(target, pr_number, reviewers)
            .await
        {
            warn!(
                repo = target.full_name(),
                pr_number,
                error = ?err,
                "failed to request reviewers"
            );
        }
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

    pub(crate) fn pr_progress_comment(
        &self,
        item: &ProjectItem,
        state: ManagedState,
        branch: &str,
        pr_number: i64,
    ) -> String {
        let marker = AutonoMarker::new(&item.id, state)
            .with_branch(branch)
            .with_pr_number(pr_number)
            .render();
        let action = match state {
            ManagedState::PrOpen => "Opened",
            ManagedState::ReviewPending => "Updated",
            _ => "Tracked",
        };
        format!("{marker}\n\n{action} pull request #{pr_number} for this item.")
    }

    fn marker(&self, item: &ProjectItem, state: ManagedState) -> String {
        AutonoMarker::new(&item.id, state).render()
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
