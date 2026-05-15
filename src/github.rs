use std::convert::TryFrom;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use octocrab::models::pulls;
use octocrab::params;
use octocrab::{GraphqlResponse, Octocrab, Page};
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;
use tokio::process::Command;
use tokio::time as tokio_time;

use crate::config::{GitHubConfig, TargetConfig, TokenSource};
use crate::error::{Error, OptionContext, Result, ResultContext};
use crate::github_types::{
    ProjectItemsResponse, ProjectStatusFieldResponse, PullRequestReviewDecisionValue,
    PullRequestReviewNode, PullRequestReviewStateResponse, PullRequestReviewStateValue,
    PullRequestReviewThreadsResponse, ResolveProjectResponse, ReviewThreadCommentsResponse,
    CONVERT_PULL_REQUEST_TO_DRAFT_MUTATION, MARK_PULL_REQUEST_READY_MUTATION, PROJECT_ITEMS_QUERY,
    PROJECT_STATUS_FIELD_QUERY, PULL_REQUEST_REVIEW_STATE_QUERY, RESOLVE_PROJECT_QUERY,
    REVIEW_THREADS_QUERY, REVIEW_THREAD_COMMENTS_QUERY, UPDATE_PROJECT_FIELD_MUTATION,
};
use crate::workflow::{CommentView, ReviewDecision};

const MAX_GRAPHQL_PAGES: usize = 100;
const MAX_REST_PAGES: usize = 100;
const GITHUB_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const GH_TOKEN_TIMEOUT: Duration = Duration::from_secs(30);

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GitHubClient {
    octo: Octocrab,
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
    async fn mark_pull_request_ready(
        &self,
        target: &TargetConfig,
        pr: &PullRequestInfo,
    ) -> Result<()>;
    async fn convert_pull_request_to_draft(
        &self,
        target: &TargetConfig,
        pr: &PullRequestInfo,
    ) -> Result<()>;
    async fn list_review_threads(
        &self,
        target: &TargetConfig,
        pr_number: i64,
    ) -> Result<Vec<ReviewThread>>;
}

impl GitHubClient {
    pub async fn from_config(config: &GitHubConfig) -> Result<Self> {
        let token = GitHubAuthenticator::new(&config.token_source)
            .token()
            .await?;
        let octo = Octocrab::builder()
            .base_uri(config.api_url.trim_end_matches('/'))?
            .personal_token(token)
            .set_connect_timeout(Some(GITHUB_CONNECT_TIMEOUT))
            .set_read_timeout(Some(GITHUB_REQUEST_TIMEOUT))
            .set_write_timeout(Some(GITHUB_REQUEST_TIMEOUT))
            .build()
            .context("failed to build GitHub client")?;
        Ok(Self {
            octo,
            graphql_url: config.graphql_url.trim_end_matches('/').to_string(),
        })
    }

    async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        let response: GraphqlResponse<T> = self
            .octo
            .post(
                &self.graphql_url,
                Some(&json!({ "query": query, "variables": variables })),
            )
            .await
            .context("failed to call GitHub GraphQL")?;
        match response {
            GraphqlResponse::Ok(ok) => Ok(ok.data),
            GraphqlResponse::Err(err) => Err(Error::message(format!(
                "GitHub GraphQL errors: {}",
                serde_json::to_string(&err.errors)?
            ))),
        }
    }

    async fn collect_pages<T: serde::de::DeserializeOwned>(
        &self,
        mut page: Page<T>,
        resource: &str,
    ) -> Result<Vec<T>> {
        let mut items = page.take_items();
        let mut page_count = 1usize;
        while let Some(mut next_page) = self.octo.get_page(&page.next).await? {
            page_count += 1;
            if page_count > MAX_REST_PAGES {
                return Err(Error::message(format!(
                    "GitHub pagination exceeded {MAX_REST_PAGES} pages for {resource}"
                )));
            }
            items.append(&mut next_page.take_items());
            page = next_page;
        }
        Ok(items)
    }

    async fn pull_info(
        &self,
        target: &TargetConfig,
        pull: pulls::PullRequest,
    ) -> Result<PullRequestInfo> {
        let review_state = self.review_state(target, pull.number).await?;
        Ok(PullRequestInfo {
            number: github_id(pull.number)?,
            node_id: pull.node_id,
            head_sha: pull.head.sha,
            merged: pull.merged,
            is_draft: pull.draft.unwrap_or(false),
            review_decision: review_state.decision,
            latest_review_id: review_state.review_id,
            latest_review_body: review_state.review_body,
        })
    }

    async fn review_state(&self, target: &TargetConfig, pr_number: u64) -> Result<ReviewState> {
        let mut reviews = Vec::new();
        let mut after: Option<String> = None;
        let mut page_count = 0usize;
        let mut decision = None;
        loop {
            page_count += 1;
            if page_count > MAX_GRAPHQL_PAGES {
                return Err(Error::message(format!(
                    "GitHub GraphQL pagination exceeded {MAX_GRAPHQL_PAGES} pages for pull request review state"
                )));
            }
            let data: PullRequestReviewStateResponse = self
                .graphql(
                    PULL_REQUEST_REVIEW_STATE_QUERY,
                    json!({
                        "owner": target.owner,
                        "repo": target.repo,
                        "number": pr_number,
                        "after": after,
                    }),
                )
                .await?;
            let pull_request = data
                .repository
                .and_then(|repository| repository.pull_request)
                .context(format!(
                    "pull request {}/{}#{} not found",
                    target.owner, target.repo, pr_number
                ))?;
            let page_decision = review_decision_from_github(pull_request.review_decision);
            if decision.is_none() {
                decision = Some(page_decision);
            }
            let Some(page) = pull_request.latest_opinionated_reviews else {
                break;
            };
            let has_next_page = page.page_info.has_next_page;
            let next_after = page.page_info.end_cursor.clone();
            reviews.extend(page.nodes);
            if !has_next_page {
                break;
            }
            let next_after = next_after.context("pull request review page had no end cursor")?;
            if after.as_deref() == Some(next_after.as_str()) {
                return Err(Error::message(
                    "GitHub pull request review pagination cursor did not advance",
                ));
            }
            after = Some(next_after);
        }
        Ok(review_state_for_decision(
            decision.unwrap_or(ReviewDecision::None),
            reviews,
        ))
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
struct ReviewState {
    decision: ReviewDecision,
    review_id: Option<i64>,
    review_body: Option<String>,
}

impl ReviewState {
    fn new(decision: ReviewDecision, review_id: Option<i64>, review_body: Option<String>) -> Self {
        Self {
            decision,
            review_id,
            review_body,
        }
    }
}

fn review_decision_from_github(decision: Option<PullRequestReviewDecisionValue>) -> ReviewDecision {
    match decision {
        Some(PullRequestReviewDecisionValue::Approved) => ReviewDecision::Approved,
        Some(PullRequestReviewDecisionValue::ChangesRequested) => ReviewDecision::ChangesRequested,
        Some(PullRequestReviewDecisionValue::ReviewRequired) | None => ReviewDecision::None,
    }
}

fn review_state_for_decision(
    decision: ReviewDecision,
    reviews: Vec<PullRequestReviewNode>,
) -> ReviewState {
    let Some(wanted_state) = opinionated_review_state(decision) else {
        return ReviewState::new(decision, None, None);
    };
    let latest_review = reviews
        .into_iter()
        .filter(|review| review.state == wanted_state)
        .max_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then_with(|| left.full_database_id.cmp(&right.full_database_id))
        });
    ReviewState::new(
        decision,
        latest_review
            .as_ref()
            .and_then(|review| review.full_database_id),
        latest_review.and_then(|review| non_empty_review_body(review.body)),
    )
}

fn opinionated_review_state(decision: ReviewDecision) -> Option<PullRequestReviewStateValue> {
    match decision {
        ReviewDecision::Approved => Some(PullRequestReviewStateValue::Approved),
        ReviewDecision::ChangesRequested => Some(PullRequestReviewStateValue::ChangesRequested),
        ReviewDecision::None => None,
    }
}

fn non_empty_review_body(body: String) -> Option<String> {
    if body.trim().is_empty() {
        None
    } else {
        Some(body)
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
                    json!({
                        "projectId": project_id,
                        "statusField": target.workflow.status_field,
                        "after": after,
                    }),
                )
                .await?;
            let page = data.node.items;
            let has_next_page = page.page_info.has_next_page;
            let next_after = page.page_info.end_cursor.clone();
            items.extend(
                page.nodes
                    .into_iter()
                    .filter_map(|node| node.into_project_item()),
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
        let page = self
            .octo
            .issues(&target.owner, &target.repo)
            .list_comments(github_number(content.number)?)
            .per_page(100)
            .send()
            .await
            .with_context(|| format!("failed to list issue comments for {path}"))?;
        let issue_comments = self.collect_pages(page, &path).await?;
        comments.extend(
            issue_comments
                .into_iter()
                .map(|comment| -> Result<CommentView> {
                    Ok(CommentView {
                        id: github_id(comment.id.into_inner())?,
                        author: comment.user.login,
                        body: comment.body.unwrap_or_default(),
                        created_at: github_time(
                            comment.created_at.timestamp(),
                            comment.created_at.timestamp_subsec_nanos(),
                        )?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        );
        Ok(comments)
    }

    async fn user_can_administer_or_write(
        &self,
        target: &TargetConfig,
        login: &str,
    ) -> Result<bool> {
        match self
            .octo
            .repos(&target.owner, &target.repo)
            .get_contributor_permission(login)
            .send()
            .await
        {
            Ok(permission) => Ok(matches!(
                permission.permission,
                params::teams::Permission::Admin
                    | params::teams::Permission::Push
                    | params::teams::Permission::Maintain
            )),
            Err(err) if is_not_found(&err) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    async fn create_issue_comment(
        &self,
        target: &TargetConfig,
        issue_number: i64,
        body: &str,
    ) -> Result<()> {
        self.octo
            .issues(&target.owner, &target.repo)
            .create_comment(github_number(issue_number)?, body)
            .await
            .with_context(|| {
                format!(
                    "failed to create issue comment for {}/{}#{}",
                    target.owner, target.repo, issue_number
                )
            })?;
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
        let data: ProjectStatusFieldResponse = self
            .graphql(
                PROJECT_STATUS_FIELD_QUERY,
                json!({
                    "projectId": project_id,
                    "statusField": target.workflow.status_field,
                }),
            )
            .await?;
        let field = data.node.status_field.with_context(|| {
            format!(
                "status field {} not found or is not a single-select field",
                target.workflow.status_field
            )
        })?;
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
        let page = self
            .octo
            .pulls(&target.owner, &target.repo)
            .list()
            .head(head)
            .state(params::State::All)
            .per_page(10)
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to list pull requests for {}/{}",
                    target.owner, target.repo
                )
            })?;
        let pulls = self.collect_pages(page, "pull requests").await?;
        match pulls
            .into_iter()
            .find(|pull| pull.merged || pull.state == octocrab::models::IssueState::Open)
        {
            Some(pull) => Ok(Some(self.pull_info(target, pull).await?)),
            None => Ok(None),
        }
    }

    async fn create_pull_request(
        &self,
        target: &TargetConfig,
        pr: &NewPullRequest,
    ) -> Result<PullRequestInfo> {
        let response = self
            .octo
            .pulls(&target.owner, &target.repo)
            .create(&pr.title, &pr.head, &pr.base)
            .body(pr.body.clone())
            .draft(pr.draft)
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to create pull request for {}/{}",
                    target.owner, target.repo
                )
            })?;
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
        let _ = self
            .octo
            .pulls(&target.owner, &target.repo)
            .request_reviews(
                github_number(pr_number)?,
                reviewers.to_vec(),
                Vec::<String>::new(),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to request reviewers for {}/{}#{}",
                    target.owner, target.repo, pr_number
                )
            })?;
        Ok(())
    }

    async fn mark_pull_request_ready(
        &self,
        target: &TargetConfig,
        pr: &PullRequestInfo,
    ) -> Result<()> {
        if !pr.is_draft {
            return Ok(());
        }
        let _: serde_json::Value = self
            .graphql(
                MARK_PULL_REQUEST_READY_MUTATION,
                json!({ "pullRequestId": pr.node_id }),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to mark pull request {}/{}#{} ready for review",
                    target.owner, target.repo, pr.number
                )
            })?;
        Ok(())
    }

    async fn convert_pull_request_to_draft(
        &self,
        target: &TargetConfig,
        pr: &PullRequestInfo,
    ) -> Result<()> {
        if pr.is_draft {
            return Ok(());
        }
        let _: serde_json::Value = self
            .graphql(
                CONVERT_PULL_REQUEST_TO_DRAFT_MUTATION,
                json!({ "pullRequestId": pr.node_id }),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to convert pull request {}/{}#{} to draft",
                    target.owner, target.repo, pr.number
                )
            })?;
        Ok(())
    }

    async fn list_review_threads(
        &self,
        target: &TargetConfig,
        pr_number: i64,
    ) -> Result<Vec<ReviewThread>> {
        let mut threads = Vec::new();
        let mut after: Option<String> = None;
        let mut page_count = 0usize;
        loop {
            page_count += 1;
            if page_count > MAX_GRAPHQL_PAGES {
                return Err(Error::message(format!(
                    "GitHub GraphQL pagination exceeded {MAX_GRAPHQL_PAGES} pages for review threads"
                )));
            }
            let data: PullRequestReviewThreadsResponse = self
                .graphql(
                    REVIEW_THREADS_QUERY,
                    json!({
                        "owner": target.owner,
                        "repo": target.repo,
                        "number": pr_number,
                        "after": after,
                    }),
                )
                .await?;
            let pull_request = data
                .repository
                .and_then(|repository| repository.pull_request)
                .context(format!(
                    "pull request {}/{}#{} not found",
                    target.owner, target.repo, pr_number
                ))?;
            let page = pull_request.review_threads;
            let has_next_page = page.page_info.has_next_page;
            let next_after = page.page_info.end_cursor.clone();
            for node in page.nodes {
                let (mut thread, comment_page) = node.into_review_thread_parts();
                if comment_page.has_next_page {
                    let next_comment_after = comment_page
                        .end_cursor
                        .context("review thread comment page had no end cursor")?;
                    let thread_id = thread.id.clone();
                    thread.comments.extend(
                        self.list_review_thread_comments(&thread_id, next_comment_after)
                            .await?,
                    );
                }
                threads.push(thread);
            }
            if !has_next_page {
                break;
            }
            let next_after = next_after.context("review thread page had no end cursor")?;
            if after.as_deref() == Some(next_after.as_str()) {
                return Err(Error::message(
                    "GitHub review thread pagination cursor did not advance",
                ));
            }
            after = Some(next_after);
        }
        Ok(threads)
    }
}

impl GitHubClient {
    async fn list_review_thread_comments(
        &self,
        thread_id: &str,
        initial_after: String,
    ) -> Result<Vec<ReviewThreadComment>> {
        let mut comments = Vec::new();
        let mut after = initial_after;
        let mut page_count = 0usize;
        loop {
            page_count += 1;
            if page_count > MAX_GRAPHQL_PAGES {
                return Err(Error::message(format!(
                    "GitHub GraphQL pagination exceeded {MAX_GRAPHQL_PAGES} pages for review thread comments"
                )));
            }
            let data: ReviewThreadCommentsResponse = self
                .graphql(
                    REVIEW_THREAD_COMMENTS_QUERY,
                    json!({ "threadId": thread_id, "after": after }),
                )
                .await?;
            let page = data
                .node
                .context(format!("review thread {thread_id} not found"))?
                .comments;
            let (page_comments, page_info) = page.into_parts();
            comments.extend(page_comments);
            if !page_info.has_next_page {
                break;
            }
            let next_after = page_info
                .end_cursor
                .context("review thread comments page had no end cursor")?;
            if next_after == after {
                return Err(Error::message(
                    "GitHub review thread comments pagination cursor did not advance",
                ));
            }
            after = next_after;
        }
        Ok(comments)
    }

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
            TokenSource::Gh => self.gh_token().await,
        }
    }

    async fn gh_token(&self) -> Result<String> {
        let mut command = Command::new("gh");
        command
            .args(["auth", "token"])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = tokio_time::timeout(GH_TOKEN_TIMEOUT, command.output())
            .await
            .map_err(|_| Error::timeout("gh auth token exceeded 30 seconds"))?
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

impl ReviewThread {
    pub(crate) fn line_label(&self) -> String {
        match (self.line, self.original_line) {
            (Some(line), _) => line.to_string(),
            (None, Some(original_line)) => format!("original {original_line}"),
            (None, None) => "unknown".to_string(),
        }
    }
}

fn github_number(number: i64) -> Result<u64> {
    u64::try_from(number).context(format!("GitHub number {number} must be non-negative"))
}

fn github_id(id: u64) -> Result<i64> {
    i64::try_from(id).context(format!("GitHub id {id} does not fit in i64"))
}

fn github_time(seconds: i64, nanos: u32) -> Result<OffsetDateTime> {
    Ok(OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(seconds) * 1_000_000_000 + i128::from(nanos),
    )?)
}

fn is_not_found(err: &octocrab::Error) -> bool {
    matches!(err, octocrab::Error::GitHub { source, .. } if source.status_code.as_u16() == 404)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(
        id: i64,
        state: PullRequestReviewStateValue,
        body: &str,
        submitted_at: i64,
    ) -> PullRequestReviewNode {
        PullRequestReviewNode {
            full_database_id: Some(id),
            body: body.to_string(),
            state,
            submitted_at: Some(OffsetDateTime::from_unix_timestamp(submitted_at).unwrap()),
        }
    }

    #[test]
    fn review_state_prefers_latest_matching_changes_request() {
        let state = review_state_for_decision(
            ReviewDecision::ChangesRequested,
            vec![
                review(
                    101,
                    PullRequestReviewStateValue::ChangesRequested,
                    "Please fix the race.",
                    10,
                ),
                review(
                    102,
                    PullRequestReviewStateValue::Approved,
                    "Looks good.",
                    20,
                ),
            ],
        );

        assert_eq!(state.decision, ReviewDecision::ChangesRequested);
        assert_eq!(state.review_id, Some(101));
        assert_eq!(state.review_body.as_deref(), Some("Please fix the race."));
    }

    #[test]
    fn review_state_uses_latest_matching_review_and_drops_blank_body() {
        let state = review_state_for_decision(
            ReviewDecision::Approved,
            vec![
                review(201, PullRequestReviewStateValue::Approved, "Ship it.", 10),
                review(202, PullRequestReviewStateValue::Approved, "   ", 20),
                review(
                    203,
                    PullRequestReviewStateValue::ChangesRequested,
                    "Needs work.",
                    30,
                ),
            ],
        );

        assert_eq!(state.decision, ReviewDecision::Approved);
        assert_eq!(state.review_id, Some(202));
        assert_eq!(state.review_body, None);
    }
}
