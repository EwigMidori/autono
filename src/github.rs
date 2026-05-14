use std::process::Stdio;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;
use tokio::process::Command;

use crate::config::{GitHubConfig, TargetConfig, TokenSource};
use crate::error::{Error, OptionContext, Result, ResultContext};
use crate::github_types::{
    GraphQlEnvelope, IssueCommentResponse, PermissionResponse, ProjectFieldsResponse,
    ProjectItemsResponse, PullResponse, ResolveProjectResponse, ReviewResponse,
    PROJECT_FIELDS_QUERY, PROJECT_ITEMS_QUERY, RESOLVE_PROJECT_QUERY,
    UPDATE_PROJECT_FIELD_MUTATION,
};
use crate::workflow::{CommentView, ReviewDecision};

const MAX_GRAPHQL_PAGES: usize = 100;
const MAX_REST_PAGES: usize = 100;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GitHubClient {
    rest: reqwest::Client,
    api_url: String,
    graphql_url: String,
}

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
    pub merged: bool,
    pub review_decision: ReviewDecision,
    pub latest_review_id: Option<i64>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
}

#[async_trait]
pub trait GitHub: Send + Sync {
    async fn list_project_items(&self, target: &TargetConfig) -> Result<Vec<ProjectItem>>;
    async fn list_comments(
        &self,
        target: &TargetConfig,
        content: &ProjectContent,
    ) -> Result<Vec<CommentView>>;
    async fn user_can_administer_or_write(
        &self,
        target: &TargetConfig,
        login: &str,
    ) -> Result<bool>;
    async fn create_issue_comment(
        &self,
        target: &TargetConfig,
        issue_number: i64,
        body: &str,
    ) -> Result<()>;
    async fn set_project_status(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        status: &str,
    ) -> Result<()>;
    async fn find_agent_pr(
        &self,
        target: &TargetConfig,
        branch: &str,
    ) -> Result<Option<PullRequestInfo>>;
    async fn create_pull_request(
        &self,
        target: &TargetConfig,
        pr: &NewPullRequest,
    ) -> Result<PullRequestInfo>;
    async fn request_reviewers(
        &self,
        target: &TargetConfig,
        pr_number: i64,
        reviewers: &[String],
    ) -> Result<()>;
}

impl GitHubClient {
    pub async fn from_config(config: &GitHubConfig) -> Result<Self> {
        let token = GitHubAuthenticator::new(&config.token_source)
            .token()
            .await?;
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("autono"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .context("failed to build authorization header")?,
        );
        let rest = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build GitHub HTTP client")?;
        Ok(Self {
            rest,
            api_url: config.api_url.trim_end_matches('/').to_string(),
            graphql_url: config.graphql_url.clone(),
        })
    }

    async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        let response = self
            .rest
            .post(&self.graphql_url)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .context("failed to call GitHub GraphQL")?
            .error_for_status()
            .context("GitHub GraphQL returned an error status")?;
        let envelope: GraphQlEnvelope<T> = response.json().await.context("invalid GraphQL JSON")?;
        if let Some(errors) = envelope.errors {
            return Err(Error::message(format!(
                "GitHub GraphQL errors: {}",
                serde_json::to_string(&errors)?
            )));
        }
        envelope.data.context("GitHub GraphQL response had no data")
    }

    async fn rest_get_query<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.rest
            .get(format!("{}{}", self.api_url, path))
            .query(query)
            .send()
            .await
            .with_context(|| format!("failed to GET {path}"))?
            .error_for_status()
            .with_context(|| format!("GitHub GET {path} failed"))?
            .json()
            .await
            .with_context(|| format!("invalid JSON from {path}"))
    }

    async fn rest_get_optional<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<Option<T>> {
        let response = self
            .rest
            .get(format!("{}{}", self.api_url, path))
            .send()
            .await
            .with_context(|| format!("failed to GET {path}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        response
            .error_for_status()
            .with_context(|| format!("GitHub GET {path} failed"))?
            .json()
            .await
            .map(Some)
            .with_context(|| format!("invalid JSON from {path}"))
    }

    async fn rest_post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: serde_json::Value,
    ) -> Result<T> {
        self.rest
            .post(format!("{}{}", self.api_url, path))
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("failed to POST {path}"))?
            .error_for_status()
            .with_context(|| format!("GitHub POST {path} failed"))?
            .json()
            .await
            .with_context(|| format!("invalid JSON from {path}"))
    }

    async fn pull_info(
        &self,
        target: &TargetConfig,
        pull: PullResponse,
    ) -> Result<PullRequestInfo> {
        let review_decision = self.review_decision(target, pull.number).await?;
        Ok(PullRequestInfo {
            number: pull.number,
            merged: pull.merged_at.is_some(),
            review_decision: review_decision.decision,
            latest_review_id: review_decision.review_id,
        })
    }

    async fn review_decision(&self, target: &TargetConfig, pr_number: i64) -> Result<ReviewState> {
        let path = format!(
            "/repos/{}/{}/pulls/{}/reviews",
            target.owner, target.repo, pr_number
        );
        let mut reviews = Vec::new();
        let mut page = 1usize;
        loop {
            let batch: Vec<ReviewResponse> = self
                .rest_get_query(
                    &path,
                    &[("per_page", "100".to_string()), ("page", page.to_string())],
                )
                .await?;
            let done = batch.len() < 100;
            reviews.extend(batch);
            if done {
                break;
            }
            if page >= MAX_REST_PAGES {
                return Err(Error::message(format!(
                    "GitHub pagination exceeded {MAX_REST_PAGES} pages for {path}"
                )));
            }
            page += 1;
        }
        for review in reviews.iter().rev() {
            match review.state.as_str() {
                "CHANGES_REQUESTED" => {
                    return Ok(ReviewState::new(
                        ReviewDecision::ChangesRequested,
                        Some(review.id),
                    ));
                }
                "APPROVED" => {
                    return Ok(ReviewState::new(ReviewDecision::Approved, Some(review.id)))
                }
                _ => {}
            }
        }
        Ok(ReviewState::new(ReviewDecision::None, None))
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
struct ReviewState {
    decision: ReviewDecision,
    review_id: Option<i64>,
}

impl ReviewState {
    fn new(decision: ReviewDecision, review_id: Option<i64>) -> Self {
        Self {
            decision,
            review_id,
        }
    }
}

#[async_trait]
impl GitHub for GitHubClient {
    async fn list_project_items(&self, target: &TargetConfig) -> Result<Vec<ProjectItem>> {
        let project_id = if let Some(project_id) = &target.project_id {
            project_id.clone()
        } else {
            self.resolve_project_id(target).await?
        };
        let mut items = Vec::new();
        let mut after: Option<String> = None;
        let mut page_count = 0usize;
        loop {
            page_count += 1;
            if page_count > MAX_GRAPHQL_PAGES {
                return Err(Error::message(format!(
                    "GitHub GraphQL pagination exceeded {MAX_GRAPHQL_PAGES} pages for project items"
                )));
            }
            let data: ProjectItemsResponse = self
                .graphql(
                    PROJECT_ITEMS_QUERY,
                    json!({ "projectId": project_id, "after": after }),
                )
                .await?;
            let page = data.node.items;
            let has_next_page = page.page_info.has_next_page;
            let next_after = page.page_info.end_cursor.clone();
            items.extend(
                page.nodes
                    .into_iter()
                    .filter_map(|node| node.into_project_item(&target.workflow.status_field)),
            );
            if !has_next_page {
                break;
            }
            let next_after = next_after.context("project item page had no end cursor")?;
            if after.as_deref() == Some(next_after.as_str()) {
                return Err(Error::message(
                    "GitHub project item pagination cursor did not advance",
                ));
            }
            after = Some(next_after);
        }
        Ok(items)
    }

    async fn list_comments(
        &self,
        target: &TargetConfig,
        content: &ProjectContent,
    ) -> Result<Vec<CommentView>> {
        let mut comments = Vec::new();
        comments.push(CommentView {
            id: 0,
            author: content.author.clone(),
            body: content.body.clone(),
            created_at: content.created_at,
        });
        let path = format!(
            "/repos/{}/{}/issues/{}/comments",
            target.owner, target.repo, content.number
        );
        let mut page = 1usize;
        loop {
            let rest_comments: Vec<IssueCommentResponse> = self
                .rest_get_query(
                    &path,
                    &[("per_page", "100".to_string()), ("page", page.to_string())],
                )
                .await?;
            let done = rest_comments.len() < 100;
            comments.extend(rest_comments.into_iter().map(|comment| CommentView {
                id: comment.id,
                author: comment.user.login,
                body: comment.body,
                created_at: comment.created_at,
            }));
            if done {
                break;
            }
            if page >= MAX_REST_PAGES {
                return Err(Error::message(format!(
                    "GitHub pagination exceeded {MAX_REST_PAGES} pages for {path}"
                )));
            }
            page += 1;
        }
        Ok(comments)
    }

    async fn user_can_administer_or_write(
        &self,
        target: &TargetConfig,
        login: &str,
    ) -> Result<bool> {
        let path = format!(
            "/repos/{}/{}/collaborators/{}/permission",
            target.owner, target.repo, login
        );
        let Some(response) = self.rest_get_optional::<PermissionResponse>(&path).await? else {
            return Ok(false);
        };
        Ok(matches!(
            response.permission.as_str(),
            "admin" | "write" | "maintain"
        ))
    }

    async fn create_issue_comment(
        &self,
        target: &TargetConfig,
        issue_number: i64,
        body: &str,
    ) -> Result<()> {
        let path = format!(
            "/repos/{}/{}/issues/{}/comments",
            target.owner, target.repo, issue_number
        );
        let _: serde_json::Value = self.rest_post(&path, json!({ "body": body })).await?;
        Ok(())
    }

    async fn set_project_status(
        &self,
        target: &TargetConfig,
        item: &ProjectItem,
        status: &str,
    ) -> Result<()> {
        let project_id = if let Some(project_id) = &target.project_id {
            project_id.clone()
        } else {
            self.resolve_project_id(target).await?
        };
        let data: ProjectFieldsResponse = self
            .graphql(PROJECT_FIELDS_QUERY, json!({ "projectId": project_id }))
            .await?;
        let field = data
            .node
            .fields
            .nodes
            .into_iter()
            .flatten()
            .find(|field| field.name == target.workflow.status_field)
            .with_context(|| format!("status field {} not found", target.workflow.status_field))?;
        let option = field
            .options
            .into_iter()
            .find(|option| option.name == status)
            .with_context(|| format!("status option {status} not found"))?;
        let _: serde_json::Value = self
            .graphql(
                UPDATE_PROJECT_FIELD_MUTATION,
                json!({
                    "projectId": project_id,
                    "itemId": item.id,
                    "fieldId": field.id,
                    "optionId": option.id,
                }),
            )
            .await?;
        Ok(())
    }

    async fn find_agent_pr(
        &self,
        target: &TargetConfig,
        branch: &str,
    ) -> Result<Option<PullRequestInfo>> {
        let head = format!("{}:{branch}", target.owner);
        let path = format!("/repos/{}/{}/pulls", target.owner, target.repo);
        let prs: Vec<PullResponse> = self
            .rest_get_query(
                &path,
                &[
                    ("head", head),
                    ("state", "all".to_string()),
                    ("per_page", "10".to_string()),
                ],
            )
            .await?;
        match prs.into_iter().next() {
            Some(pull) => Ok(Some(self.pull_info(target, pull).await?)),
            None => Ok(None),
        }
    }

    async fn create_pull_request(
        &self,
        target: &TargetConfig,
        pr: &NewPullRequest,
    ) -> Result<PullRequestInfo> {
        let path = format!("/repos/{}/{}/pulls", target.owner, target.repo);
        let response: PullResponse = self
            .rest_post(
                &path,
                json!({
                    "title": pr.title,
                    "body": pr.body,
                    "head": pr.head,
                    "base": pr.base,
                }),
            )
            .await?;
        self.pull_info(target, response).await
    }

    async fn request_reviewers(
        &self,
        target: &TargetConfig,
        pr_number: i64,
        reviewers: &[String],
    ) -> Result<()> {
        if reviewers.is_empty() {
            return Ok(());
        }
        let path = format!(
            "/repos/{}/{}/pulls/{}/requested_reviewers",
            target.owner, target.repo, pr_number
        );
        let _: serde_json::Value = self
            .rest_post(&path, json!({ "reviewers": reviewers }))
            .await?;
        Ok(())
    }
}

impl GitHubClient {
    async fn resolve_project_id(&self, target: &TargetConfig) -> Result<String> {
        let number = target
            .project_number
            .context("project_number is required when project_id is unset")?;
        let data: ResolveProjectResponse = self
            .graphql(
                RESOLVE_PROJECT_QUERY,
                json!({ "login": target.project_owner(), "number": number }),
            )
            .await?;
        data.organization
            .or(data.user)
            .and_then(|owner| owner.project_v2)
            .map(|project| project.id)
            .context("failed to resolve project id")
    }
}

#[derive(Debug, Clone, Copy)]
struct GitHubAuthenticator<'a> {
    source: &'a TokenSource,
}

impl<'a> GitHubAuthenticator<'a> {
    fn new(source: &'a TokenSource) -> Self {
        Self { source }
    }

    async fn token(&self) -> Result<String> {
        match self.source {
            TokenSource::Env => std::env::var("GITHUB_TOKEN").context("GITHUB_TOKEN is not set"),
            TokenSource::Gh => self.gh_token().await,
        }
    }

    async fn gh_token(&self) -> Result<String> {
        let output = Command::new("gh")
            .args(["auth", "token"])
            .stdin(Stdio::null())
            .output()
            .await
            .context("failed to run gh auth token")?;
        if !output.status.success() {
            return Err(Error::message(format!(
                "gh auth token failed with status {}",
                output.status
            )));
        }
        Ok(String::from_utf8(output.stdout)
            .context("gh auth token output was not UTF-8")?
            .trim()
            .to_string())
    }
}
