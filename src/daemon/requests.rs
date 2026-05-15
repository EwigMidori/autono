use crate::config::TargetConfig;
use crate::git_workspace::WorkIdentity;
use crate::github::{ProjectContent, ProjectItem};
use crate::store::{StoreItemKey, StoredItem};
use crate::workflow::{CommentThread, ManagedState};

#[derive(Debug)]
pub(crate) struct WorkRequest<'a> {
    pub(crate) target: &'a TargetConfig,
    pub(crate) item: &'a ProjectItem,
    pub(crate) content: &'a ProjectContent,
    pub(crate) thread: &'a CommentThread,
    pub(crate) stored: Option<StoredItem>,
    pub(crate) post_work_state: ManagedState,
    pub(crate) handled_review_id: Option<i64>,
    pub(crate) review_body: Option<String>,
    pub(crate) pr_number: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct SelfReviewRequest<'a, 'b> {
    pub(crate) work: WorkRequest<'a>,
    pub(crate) identity: &'b WorkIdentity,
    pub(crate) store_key: StoreItemKey<'b>,
    pub(crate) pr: crate::github::PullRequestInfo,
    pub(crate) prompt: &'b crate::runner::codex::ImplementationPrompt,
    pub(crate) validation: &'b crate::runner::codex::ValidationRunner,
}

#[derive(Debug)]
pub(crate) struct TriageRequest<'a> {
    pub(crate) target: &'a TargetConfig,
    pub(crate) item: &'a ProjectItem,
    pub(crate) content: &'a ProjectContent,
    pub(crate) thread: &'a CommentThread,
    pub(crate) latest_comment_id: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct DiscussionRequest<'a> {
    pub(crate) target: &'a TargetConfig,
    pub(crate) item: &'a ProjectItem,
    pub(crate) content: &'a ProjectContent,
    pub(crate) thread: &'a CommentThread,
    pub(crate) state: ManagedState,
    pub(crate) latest_comment_id: Option<i64>,
}
