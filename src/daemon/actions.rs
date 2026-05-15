use crate::config::TargetConfig;
use crate::error::{Result, ResultContext};
use crate::runner::codex::{DiscussionPrompt, DiscussionPromptContext, TriagePrompt};
use crate::store::StoreItemKey;
use crate::workflow::{AdminMention, CommentThread, ManagedState};

use super::{Daemon, DiscussionRequest, TriageRequest};

impl<G: crate::github::GitHub, R: crate::runner::codex::AgentRunner, W> Daemon<G, R, W> {
    pub(crate) async fn admin_mention(
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

    pub(crate) async fn triage(&self, request: TriageRequest<'_>) -> Result<()> {
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

    pub(crate) async fn monitor_discussion(&self, request: DiscussionRequest<'_>) -> Result<()> {
        let discussion = self
            .comment_composer
            .discussion_text(request.thread.comments());
        let prompt = DiscussionPrompt::new(
            &request.content.title,
            &request.content.body,
            &discussion,
            DiscussionPromptContext {
                state: request.state.to_string(),
                base_branch: request.target.base_branch.clone(),
                start_status: request.target.workflow.start_status.clone(),
                readonly_checkout: request.target.checkout_path.clone(),
            },
        )
        .render();
        let decision = self
            .runner
            .monitor_discussion(
                &request.target.commands,
                &request.target.checkout_path,
                &prompt,
            )
            .await?;
        let store_key = StoreItemKey::new(
            &request.target.owner,
            &request.target.repo,
            &request.item.id,
        );
        self.store
            .mark_state(store_key, request.state, request.latest_comment_id)?;
        if decision.should_reply && !decision.reply.trim().is_empty() {
            self.try_create_issue_comment(
                request.target,
                request.content.number,
                &self.comment_composer.discussion_monitor_comment(
                    request.item,
                    request.state,
                    &decision.reply,
                ),
            )
            .await;
        }
        Ok(())
    }
}
