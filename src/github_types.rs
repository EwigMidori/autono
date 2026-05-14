use serde::Deserialize;
use time::OffsetDateTime;

use crate::github::{ProjectContent, ProjectContentKind, ProjectItem};

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct GraphQlEnvelope<T> {
    pub(crate) data: Option<T>,
    pub(crate) errors: Option<Vec<serde_json::Value>>,
}

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
pub(crate) struct IssueCommentResponse {
    pub(crate) id: i64,
    pub(crate) body: String,
    pub(crate) user: UserResponse,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created_at: OffsetDateTime,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct UserResponse {
    pub(crate) login: String,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PermissionResponse {
    pub(crate) permission: String,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct PullResponse {
    pub(crate) number: i64,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(crate) merged_at: Option<OffsetDateTime>,
}

#[non_exhaustive]
#[derive(Debug, Deserialize)]
pub(crate) struct ReviewResponse {
    pub(crate) id: i64,
    pub(crate) state: String,
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
