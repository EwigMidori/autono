use std::path::PathBuf;

use crate::config::TargetConfig;
use crate::error::{Error, Result};
use crate::git_workspace::{WorkIdentity, WorkspaceManager};
use crate::github::NewPullRequest;
use crate::runner::codex::{
    CompletionOutcome, ImplementationPrompt, SelfReviewOutcome, ValidationRunner,
};
use crate::store::StoreItemKey;
use crate::workflow::ManagedState;

use super::{Daemon, SelfReviewRequest as DaemonSelfReviewRequest, WorkRequest};

impl<G: crate::github::GitHub, R: crate::runner::codex::AgentRunner, W: WorkspaceManager>
    Daemon<G, R, W>
{
    pub(crate) async fn start_or_continue_work(&self, work: WorkRequest<'_>) -> Result<()> {
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

        let feedback = crate::review_feedback::ReviewFeedbackComposer::default();
        let feedback_request = crate::review_feedback::FeedbackRequest::new(
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

        let branch_has_changes = self
            .finalize_worktree(
                &identity,
                work.target,
                &format!("Implement {}", work.content.title),
                &validation,
            )
            .await?;

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
                            draft: true,
                        },
                    )
                    .await?
            }
        };
        self.store.attach_pr(store_key, pr.number)?;
        self.store.mark_state(
            store_key,
            ManagedState::Reviewing,
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
                ManagedState::Reviewing,
                &identity.branch,
                pr.number,
            ),
        )
        .await;
        let validation = ValidationRunner::new(&identity.worktree_path, &work.target.commands.test);
        self.run_self_review_gate(DaemonSelfReviewRequest {
            work,
            identity: &identity,
            store_key,
            pr,
            prompt: &prompt,
            validation: &validation,
        })
        .await
    }

    pub(crate) async fn run_existing_self_review(
        &self,
        work: WorkRequest<'_>,
        pr: &crate::github::PullRequestInfo,
    ) -> Result<()> {
        let Some(stored) = work.stored.as_ref() else {
            return Ok(());
        };
        let Some(branch) = stored.branch.as_deref() else {
            return Ok(());
        };
        let Some(worktree_path) = stored.worktree_path.as_ref() else {
            return Ok(());
        };
        let owner = stored.owner.clone();
        let repo = stored.repo.clone();
        let item_id = stored.item_id.clone();
        let post_work_state = if stored.last_review_id.is_some() {
            ManagedState::ReviewPending
        } else {
            ManagedState::PrOpen
        };
        let handled_review_id = stored.last_review_id;
        let identity = WorkIdentity {
            branch: branch.to_string(),
            worktree_path: PathBuf::from(worktree_path),
        };
        self.workspace
            .ensure_worktree(work.target, &identity)
            .await?;
        let discussion = self
            .comment_composer
            .discussion_text(work.thread.comments());
        let prompt =
            ImplementationPrompt::new(&work.content.title, &discussion, &work.target.commands.test);
        let validation = ValidationRunner::new(&identity.worktree_path, &work.target.commands.test);
        self.run_self_review_gate(DaemonSelfReviewRequest {
            work: WorkRequest {
                post_work_state,
                handled_review_id,
                ..work
            },
            identity: &identity,
            store_key: StoreItemKey::new(owner.as_str(), repo.as_str(), item_id.as_str()),
            pr: pr.clone(),
            prompt: &prompt,
            validation: &validation,
        })
        .await
    }

    pub(crate) async fn run_self_review_gate(
        &self,
        request: DaemonSelfReviewRequest<'_, '_>,
    ) -> Result<()> {
        let DaemonSelfReviewRequest {
            work,
            identity,
            store_key,
            pr,
            prompt,
            validation,
        } = request;
        self.github
            .convert_pull_request_to_draft(work.target, &pr)
            .await?;
        if !self
            .run_completion_gate(&work, identity, prompt, validation)
            .await?
        {
            return Ok(());
        }
        for attempt in 0..=work.target.commands.max_fix_attempts {
            self.finalize_worktree(
                identity,
                work.target,
                &format!("Finalize {}", work.content.title),
                validation,
            )
            .await?;
            let review_prompt = prompt.render_self_review();
            let review = self
                .runner
                .self_review(
                    &work.target.commands,
                    &identity.worktree_path,
                    &review_prompt,
                )
                .await?;
            self.try_create_issue_comment(
                work.target,
                pr.number,
                &self.comment_composer.self_review_comment(&review),
            )
            .await;

            match review.outcome {
                SelfReviewOutcome::Ready => {
                    self.finalize_worktree(
                        identity,
                        work.target,
                        &format!("Finalize {}", work.content.title),
                        validation,
                    )
                    .await?;
                    self.github
                        .mark_pull_request_ready(work.target, &pr)
                        .await?;
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
                        pr.number,
                        &self.comment_composer.review_ready_comment(),
                    )
                    .await;
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
                    self.try_request_reviewers(
                        work.target,
                        pr.number,
                        &work.target.review.reviewers,
                    )
                    .await;
                    self.try_set_project_status(
                        work.target,
                        work.item,
                        &work.target.workflow.review_status,
                    )
                    .await;
                    return Ok(());
                }
                SelfReviewOutcome::NeedsFix if attempt < work.target.commands.max_fix_attempts => {
                    let repair_prompt = prompt.render_self_review_repair(&review);
                    self.runner
                        .implement(
                            &work.target.commands,
                            &identity.worktree_path,
                            &repair_prompt,
                        )
                        .await?;
                    if let Err(err) = validation.run().await {
                        let repair_prompt = prompt.render_repair(&format!("{err:#}"));
                        self.runner
                            .implement(
                                &work.target.commands,
                                &identity.worktree_path,
                                &repair_prompt,
                            )
                            .await?;
                        validation.run().await?;
                    }
                    self.finalize_worktree(
                        identity,
                        work.target,
                        &format!("Address self-review for {}", work.content.title),
                        validation,
                    )
                    .await?;
                }
                SelfReviewOutcome::NeedsFix | SelfReviewOutcome::Blocked => {
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
                            .blocked_self_review_comment(work.item, &review),
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
            }
        }
        Ok(())
    }

    pub(crate) async fn run_completion_gate(
        &self,
        work: &WorkRequest<'_>,
        identity: &WorkIdentity,
        prompt: &ImplementationPrompt,
        validation: &ValidationRunner,
    ) -> Result<bool> {
        for attempt in 0..=work.target.commands.max_fix_attempts {
            self.finalize_worktree(
                identity,
                work.target,
                &format!("Finalize {}", work.content.title),
                validation,
            )
            .await?;
            let completion_prompt = prompt.render_completion_check();
            let completion = self
                .runner
                .completion_check(
                    &work.target.commands,
                    &identity.worktree_path,
                    &completion_prompt,
                )
                .await?;
            match completion.outcome {
                CompletionOutcome::Complete => return Ok(true),
                CompletionOutcome::NeedsWork if attempt < work.target.commands.max_fix_attempts => {
                    let repair_prompt = prompt.render_completion_repair(&completion);
                    self.runner
                        .implement(
                            &work.target.commands,
                            &identity.worktree_path,
                            &repair_prompt,
                        )
                        .await?;
                }
                CompletionOutcome::NeedsWork | CompletionOutcome::Blocked => {
                    self.store.mark_state(
                        StoreItemKey::new(&work.target.owner, &work.target.repo, &work.item.id),
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
                            .blocked_completion_comment(work.item, &completion),
                    )
                    .await;
                    self.try_set_project_status(
                        work.target,
                        work.item,
                        &work.target.workflow.blocked_status,
                    )
                    .await;
                    return Ok(false);
                }
            }
        }
        Ok(false)
    }

    pub(crate) async fn finalize_worktree(
        &self,
        identity: &WorkIdentity,
        target: &TargetConfig,
        commit_message: &str,
        validation: &ValidationRunner,
    ) -> Result<bool> {
        validation.run().await?;
        self.workspace
            .commit_all(&identity.worktree_path, commit_message)
            .await?;
        let branch_has_changes = self
            .workspace
            .has_diff_against_base(&identity.worktree_path, target.base_branch())
            .await?;
        if branch_has_changes {
            let head = self.workspace.head_sha(&identity.worktree_path).await?;
            let remote = self
                .workspace
                .remote_head_sha(&identity.worktree_path, &identity.branch)
                .await?;
            if remote.as_deref() != Some(head.as_str()) {
                self.workspace
                    .push(&identity.worktree_path, &identity.branch)
                    .await?;
            }
            let synced_remote = self
                .workspace
                .remote_head_sha(&identity.worktree_path, &identity.branch)
                .await?;
            if synced_remote.as_deref() != Some(head.as_str()) {
                return Err(Error::message(format!(
                    "remote branch {} is not synchronized with local HEAD {}",
                    identity.branch, head
                )));
            }
        }
        if self
            .workspace
            .has_uncommitted_changes(&identity.worktree_path)
            .await?
        {
            return Err(Error::message(format!(
                "worktree {} still has uncommitted changes after finalization",
                identity.worktree_path.display()
            )));
        }
        Ok(branch_has_changes)
    }
}
