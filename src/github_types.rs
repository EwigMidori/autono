use serde::Deserialize;
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
    #[serde(rename = "fieldValues")]
    field_values: FieldValueConnection,
}

#[derive(Debug, Deserialize)]
struct FieldValueConnection {
    nodes: Vec<FieldValueNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FieldValueNode {
    name: Option<String>,
    field: Option<FieldNameNode>,
}

#[derive(Debug, Deserialize)]
struct FieldNameNode {
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
    pub(crate) fn into_project_item(self, status_field: &str) -> Option<ProjectItem> {
        let status = self
            .field_values
            .nodes
            .into_iter()
            .find(|value| {
                value
                    .field
                    .as_ref()
                    .map(|field| field.name == status_field)
                    .unwrap_or(false)
            })
            .and_then(|value| value.name);

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
pub(crate) struct ProjectFieldsResponse {
    pub(crate) node: ProjectFieldsNode,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectFieldsNode {
    pub(crate) fields: ProjectFieldConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectFieldConnection {
    pub(crate) nodes: Vec<Option<ProjectFieldNode>>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectFieldNode {
    pub(crate) id: String,
    pub(crate) name: String,
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
    #[serde(default)]
    pub(crate) original_line: Option<i64>,
    pub(crate) comments: ReviewThreadCommentConnection,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewThreadCommentConnection {
    pub(crate) nodes: Vec<ReviewThreadCommentNode>,
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
    pub(crate) fn into_review_thread(self) -> ReviewThread {
        ReviewThread {
            id: self.id,
            is_resolved: self.is_resolved,
            is_outdated: self.is_outdated,
            path: self.path,
            line: self.line,
            original_line: self.original_line,
            comments: self
                .comments
                .nodes
                .into_iter()
                .map(|comment| ReviewThreadComment {
                    author: comment.author.map(|user| user.login).unwrap_or_default(),
                    body: comment.body,
                    diff_hunk: comment.diff_hunk,
                    html_url: comment.url,
                })
                .collect(),
        }
    }
}

pub(crate) const RESOLVE_PROJECT_QUERY: &str = r#"
query ResolveProject($login: String!, $number: Int!) {
  organization(login: $login) { projectV2(number: $number) { id } }
  user(login: $login) { projectV2(number: $number) { id } }
}
"#;

pub(crate) const PROJECT_ITEMS_QUERY: &str = r#"
query ProjectItems($projectId: ID!, $after: String) {
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
          fieldValues(first: 20) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { name } }
              }
            }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

pub(crate) const PROJECT_FIELDS_QUERY: &str = r#"
query ProjectFields($projectId: ID!) {
  node(id: $projectId) {
    ... on ProjectV2 {
      fields(first: 50) {
        nodes {
          ... on ProjectV2SingleSelectField {
            id
            name
            options { id name }
          }
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
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;
