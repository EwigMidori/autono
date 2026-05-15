use time::OffsetDateTime;

use crate::workflow::ReviewDecision;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProjectItem {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    pub content: Option<ProjectContent>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProjectContent {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub created_at: OffsetDateTime,
    pub url: String,
    pub kind: ProjectContentKind,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectContentKind {
    Issue,
    PullRequest,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: i64,
    pub node_id: String,
    pub head_sha: String,
    pub merged: bool,
    pub is_draft: bool,
    pub review_decision: ReviewDecision,
    pub latest_review_id: Option<i64>,
    pub latest_review_body: Option<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ReviewThread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub comments: Vec<ReviewThreadComment>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ReviewThreadComment {
    pub author: String,
    pub body: String,
    pub diff_hunk: String,
    pub html_url: String,
}
