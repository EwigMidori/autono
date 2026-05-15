use serde::{Deserialize, Deserializer};
use time::OffsetDateTime;

use crate::github::{
    ProjectContent, ProjectContentKind, ProjectItem, ReviewThread, ReviewThreadComment,
};

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ResolveProjectResponse {
    pub(crate) organization: Option<ProjectOwnerNode>,
    pub(crate) user: Option<ProjectOwnerNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectOwnerNode {
    #[serde(rename = "projectV2")]
    pub(crate) project_v2: Option<ProjectIdNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectIdNode {
    pub(crate) id: String,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectItemsResponse {
    pub(crate) node: ProjectNode,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectNode {
    pub(crate) items: ProjectItemConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectItemConnection {
    pub(crate) nodes: Vec<ProjectItemNode>,
    #[serde(rename = "pageInfo")]
    pub(crate) page_info: PageInfo,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub(crate) has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub(crate) end_cursor: Option<String>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectItemNode {
    id: String,
    content: Option<ProjectContentNode>,
    #[serde(rename = "statusFieldValue")]
    status_field_value: Option<ProjectItemStatusValueNode>,
}

#[derive(Debug, Deserialize)]
struct ProjectItemStatusValueNode {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum ProjectContentNode {
    Issue {
        id: String,
        number: i64,
        title: String,
        body: String,
        author: Option<UserResponse>,
        #[serde(rename = "createdAt", with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        url: String,
    },
    PullRequest {
        id: String,
        number: i64,
        title: String,
        body: String,
        author: Option<UserResponse>,
        #[serde(rename = "createdAt", with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        url: String,
    },
    #[serde(other)]
    Other,
}

impl ProjectItemNode {
    pub(crate) fn into_project_item(self) -> Option<ProjectItem> {
        let status = self.status_field_value.map(|value| value.name);

        let content = self.content.and_then(|content| match content {
            ProjectContentNode::Issue {
                id,
                number,
                title,
                body,
                author,
                created_at,
                url,
            } => Some(ProjectContent {
                id,
                number,
                title,
                body,
                author: author.map(|user| user.login).unwrap_or_default(),
                created_at,
                url,
                kind: ProjectContentKind::Issue,
            }),
            ProjectContentNode::PullRequest {
                id,
                number,
                title,
                body,
                author,
                created_at,
                url,
            } => Some(ProjectContent {
                id,
                number,
                title,
                body,
                author: author.map(|user| user.login).unwrap_or_default(),
                created_at,
                url,
                kind: ProjectContentKind::PullRequest,
            }),
            ProjectContentNode::Other => None,
        });
        let title = content
            .as_ref()
            .map(|content| content.title.clone())
            .unwrap_or_else(|| self.id.clone());
        Some(ProjectItem {
            id: self.id,
            title,
            status,
            content,
        })
    }
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectStatusFieldResponse {
    pub(crate) node: ProjectStatusFieldNode,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectStatusFieldNode {
    #[serde(rename = "statusField")]
    pub(crate) status_field: Option<ProjectFieldNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectFieldNode {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) options: Vec<ProjectFieldOption>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectFieldOption {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct UserResponse {
    pub(crate) login: String,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestReviewStateResponse {
    pub(crate) repository: Option<PullRequestReviewRepositoryNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestReviewRepositoryNode {
    #[serde(rename = "pullRequest")]
    pub(crate) pull_request: Option<PullRequestReviewNodeState>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestReviewNodeState {
    #[serde(rename = "reviewDecision")]
    pub(crate) review_decision: Option<PullRequestReviewDecisionValue>,
    #[serde(rename = "latestOpinionatedReviews")]
    pub(crate) latest_opinionated_reviews: Option<PullRequestReviewConnection>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestReviewConnection {
    pub(crate) nodes: Vec<PullRequestReviewNode>,
    #[serde(rename = "pageInfo")]
    pub(crate) page_info: PageInfo,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PullRequestReviewNode {
    #[serde(
        rename = "fullDatabaseId",
        default,
        deserialize_with = "deserialize_optional_i64"
    )]
    pub(crate) full_database_id: Option<i64>,
    pub(crate) body: String,
    pub(crate) state: PullRequestReviewStateValue,
    #[serde(rename = "submittedAt", default, with = "time::serde::rfc3339::option")]
    pub(crate) submitted_at: Option<OffsetDateTime>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PullRequestReviewDecisionValue {
    ChangesRequested,
    Approved,
    ReviewRequired,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PullRequestReviewStateValue {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullRequestReviewThreadsResponse {
    pub(crate) repository: Option<ReviewThreadRepositoryNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadRepositoryNode {
    #[serde(rename = "pullRequest")]
    pub(crate) pull_request: Option<ReviewThreadPullRequestNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadPullRequestNode {
    #[serde(rename = "reviewThreads")]
    pub(crate) review_threads: ReviewThreadConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadConnection {
    pub(crate) nodes: Vec<ReviewThreadNode>,
    #[serde(rename = "pageInfo")]
    pub(crate) page_info: PageInfo,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadNode {
    pub(crate) id: String,
    #[serde(rename = "isResolved")]
    pub(crate) is_resolved: bool,
    #[serde(rename = "isOutdated")]
    pub(crate) is_outdated: bool,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) line: Option<i64>,
    #[serde(rename = "originalLine", default)]
    pub(crate) original_line: Option<i64>,
    pub(crate) comments: ReviewThreadCommentConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadCommentConnection {
    pub(crate) nodes: Vec<ReviewThreadCommentNode>,
    #[serde(rename = "pageInfo")]
    pub(crate) page_info: PageInfo,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadCommentsResponse {
    pub(crate) node: Option<ReviewThreadCommentsNode>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadCommentsNode {
    pub(crate) comments: ReviewThreadCommentConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadCommentNode {
    pub(crate) author: Option<UserResponse>,
    pub(crate) body: String,
    #[serde(rename = "diffHunk", default)]
    pub(crate) diff_hunk: String,
    pub(crate) url: String,
}

impl ReviewThreadNode {
    pub(crate) fn into_review_thread_parts(self) -> (ReviewThread, PageInfo) {
        let ReviewThreadNode {
            id,
            is_resolved,
            is_outdated,
            path,
            line,
            original_line,
            comments,
        } = self;
        let (comments, page_info) = comments.into_parts();
        (
            ReviewThread {
                id,
                is_resolved,
                is_outdated,
                path,
                line,
                original_line,
                comments,
            },
            page_info,
        )
    }
}

impl ReviewThreadCommentConnection {
    pub(crate) fn into_parts(self) -> (Vec<ReviewThreadComment>, PageInfo) {
        let ReviewThreadCommentConnection { nodes, page_info } = self;
        (
            nodes
                .into_iter()
                .map(|comment| ReviewThreadComment {
                    author: comment.author.map(|user| user.login).unwrap_or_default(),
                    body: comment.body,
                    diff_hunk: comment.diff_hunk,
                    html_url: comment.url,
                })
                .collect(),
            page_info,
        )
    }
}

pub(crate) const RESOLVE_PROJECT_QUERY: &str = r#"
query ResolveProject($login: String!, $number: Int!) {
  organization(login: $login) { projectV2(number: $number) { id } }
  user(login: $login) { projectV2(number: $number) { id } }
}
"#;

pub(crate) const PROJECT_ITEMS_QUERY: &str = r#"
query ProjectItems($projectId: ID!, $statusField: String!, $after: String) {
  node(id: $projectId) {
    ... on ProjectV2 {
      items(first: 100, after: $after) {
        nodes {
          id
          content {
            __typename
            ... on Issue { id number title body author { login } createdAt url }
            ... on PullRequest { id number title body author { login } createdAt url }
          }
          statusFieldValue: fieldValueByName(name: $statusField) {
            ... on ProjectV2ItemFieldSingleSelectValue { name }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

pub(crate) const PROJECT_STATUS_FIELD_QUERY: &str = r#"
query ProjectStatusField($projectId: ID!, $statusField: String!) {
  node(id: $projectId) {
    ... on ProjectV2 {
      statusField: field(name: $statusField) {
        ... on ProjectV2SingleSelectField {
          id
          options { id name }
        }
      }
    }
  }
}
"#;

pub(crate) const UPDATE_PROJECT_FIELD_MUTATION: &str = r#"
mutation UpdateProjectStatus($projectId: ID!, $itemId: ID!, $fieldId: ID!, $optionId: String!) {
  updateProjectV2ItemFieldValue(input: {
    projectId: $projectId,
    itemId: $itemId,
    fieldId: $fieldId,
    value: { singleSelectOptionId: $optionId }
  }) {
    projectV2Item { id }
  }
}
"#;

pub(crate) const MARK_PULL_REQUEST_READY_MUTATION: &str = r#"
mutation MarkPullRequestReady($pullRequestId: ID!) {
  markPullRequestReadyForReview(input: { pullRequestId: $pullRequestId }) {
    pullRequest { id isDraft }
  }
}
"#;

pub(crate) const CONVERT_PULL_REQUEST_TO_DRAFT_MUTATION: &str = r#"
mutation ConvertPullRequestToDraft($pullRequestId: ID!) {
  convertPullRequestToDraft(input: { pullRequestId: $pullRequestId }) {
    pullRequest { id isDraft }
  }
}
"#;

pub(crate) const PULL_REQUEST_REVIEW_STATE_QUERY: &str = r#"
query PullRequestReviewState($owner: String!, $repo: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewDecision
      latestOpinionatedReviews(first: 100, after: $after) {
        nodes {
          fullDatabaseId
          body
          state
          submittedAt
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

pub(crate) const REVIEW_THREADS_QUERY: &str = r#"
query PullRequestReviewThreads($owner: String!, $repo: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          originalLine
          comments(first: 100) {
            nodes {
              author { login }
              body
              diffHunk
              url
            }
            pageInfo { hasNextPage endCursor }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

pub(crate) const REVIEW_THREAD_COMMENTS_QUERY: &str = r#"
query PullRequestReviewThreadComments($threadId: ID!, $after: String) {
  node(id: $threadId) {
    ... on PullRequestReviewThread {
      comments(first: 100, after: $after) {
        nodes {
          author { login }
          body
          diffHunk
          url
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
#[serde(untagged)]
enum I64Value {
    Integer(i64),
    String(String),
}

fn deserialize_optional_i64<'de, D>(deserializer: D) -> std::result::Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<I64Value>::deserialize(deserializer)? {
        Some(I64Value::Integer(value)) => Ok(Some(value)),
        Some(I64Value::String(value)) => value.parse().map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_item_node_reads_targeted_status_field() {
        let node: ProjectItemNode = serde_json::from_value(json!({
            "id": "PVTI_1",
            "content": null,
            "statusFieldValue": { "name": "In Progress" }
        }))
        .unwrap();

        let item = node.into_project_item().unwrap();
        assert_eq!(item.id, "PVTI_1");
        assert_eq!(item.status.as_deref(), Some("In Progress"));
        assert!(item.content.is_none());
    }

    #[test]
    fn review_thread_node_preserves_comment_page_info() {
        let node: ReviewThreadNode = serde_json::from_value(json!({
            "id": "PRRT_1",
            "isResolved": false,
            "isOutdated": false,
            "path": "src/lib.rs",
            "line": null,
            "originalLine": 42,
            "comments": {
                "nodes": [{
                    "author": { "login": "reviewer" },
                    "body": "Please fix this.",
                    "diffHunk": "@@ -1 +1 @@\n-old\n+new",
                    "url": "https://example.com/comment"
                }],
                "pageInfo": {
                    "hasNextPage": true,
                    "endCursor": "cursor-1"
                }
            }
        }))
        .unwrap();

        let (thread, page_info) = node.into_review_thread_parts();
        assert_eq!(thread.id, "PRRT_1");
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(thread.original_line, Some(42));
        assert!(page_info.has_next_page);
        assert_eq!(page_info.end_cursor.as_deref(), Some("cursor-1"));
    }
}
