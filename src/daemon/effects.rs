use crate::config::TargetConfig;
use crate::github::ProjectContent;
use crate::github::ProjectItem;
use crate::store::StoreItemKey;
use crate::workflow::ManagedState;

use super::Daemon;

impl<G: crate::github::GitHub, R, W> Daemon<G, R, W> {
    pub(crate) async fn try_create_issue_comment(
        &self,
        target: &TargetConfig,
        issue_number: i64,
        body: &str,
    ) {
        if let Err(err) = self
            .github
            .create_issue_comment(target, issue_number, body)
            .await
        {
            tracing::warn!(
                repo = target.full_name(),
                issue_number,
                error = ?err,
                "failed to create issue comment"
            );
        }
    }

    pub(crate) async fn try_set_project_status(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        status: &str,
    ) {
        if let Err(err) = self.github.set_project_status(target, item, status).await {
            tracing::warn!(
                repo = target.full_name(),
                item_id = item.id,
                status,
                error = ?err,
                "failed to set project status"
            );
        }
    }

    pub(crate) async fn try_request_reviewers(
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
            tracing::warn!(
                repo = target.full_name(),
                pr_number,
                error = ?err,
                "failed to request reviewers"
            );
        }
    }

    pub(crate) async fn complete(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        content: &ProjectContent,
    ) {
        let _ = self.store.mark_state(
            StoreItemKey::new(&target.owner, &target.repo, &item.id),
            ManagedState::Done,
            None,
        );
        self.try_set_project_status(target, item, &target.workflow.done_status)
            .await;
        self.try_create_issue_comment(
            target,
            content.number,
            &self.comment_composer.completion_comment(item),
        )
        .await;
    }
}
