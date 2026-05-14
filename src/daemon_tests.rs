use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::tempdir;
use time::OffsetDateTime;

use crate::codex_runner::{AgentRunner, ImplementationResult};
use crate::config::{
    CommandsConfig, Config, GitHubConfig, ReviewConfig, TargetConfig, TokenSource, WorkflowConfig,
};
use crate::daemon::{CommentComposer, Daemon};
use crate::error::{Error, Result};
use crate::git_workspace::{WorkIdentity, WorkspaceManager};
use crate::github::{
    GitHub, NewPullRequest, ProjectContent, ProjectContentKind, ProjectItem, PullRequestInfo,
};
use crate::store::Store;
use crate::workflow::{CommentView, ManagedState, ReviewDecision, TriageResult};

const TEST_BOT_LOGIN: &str = "mtshit";

fn target(checkout_path: PathBuf) -> TargetConfig {
    TargetConfig {
        owner: "owner".to_string(),
        repo: "repo".to_string(),
        checkout_path,
        base_branch: "main".to_string(),
        project_owner: Some("owner".to_string()),
        project_id: Some("PVT_test".to_string()),
        project_number: None,
        workflow: WorkflowConfig {
            status_field: "Status".to_string(),
            triaged_status: "Triaged".to_string(),
            start_status: "In Progress".to_string(),
            review_status: "In Review".to_string(),
            done_status: "Done".to_string(),
            blocked_status: "Blocked".to_string(),
        },
        review: ReviewConfig {
            reviewers: vec!["reviewer".to_string()],
        },
        commands: CommandsConfig {
            codex: vec!["codex".to_string()],
            test: Vec::new(),
            max_fix_attempts: 1,
        },
    }
}

fn config(root: &Path, target: TargetConfig) -> Config {
    Config {
        bot_login: TEST_BOT_LOGIN.to_string(),
        poll_interval_secs: 60,
        worktrees_root: root.join("worktrees"),
        state_path: Some(root.join("state.sqlite3")),
        github: GitHubConfig {
            token_source: TokenSource::Env,
            api_url: "https://api.github.com".to_string(),
            graphql_url: "https://api.github.com/graphql".to_string(),
        },
        targets: vec![target],
    }
}

#[derive(Clone)]
struct FakeGitHub {
    state: Arc<Mutex<FakeGitHubState>>,
}

struct FakeGitHubState {
    item: ProjectItem,
    comments: Vec<CommentView>,
    admins: HashSet<String>,
    statuses: Vec<String>,
    pr: Option<PullRequestInfo>,
    next_review_id: i64,
    prs_created: usize,
    reviewers_requested: Vec<String>,
    fail_comments: bool,
    fail_statuses: bool,
    fail_reviewers: bool,
}

impl FakeGitHub {
    fn new(item: ProjectItem, comments: Vec<CommentView>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeGitHubState {
                item,
                comments,
                admins: HashSet::from(["admin".to_string()]),
                statuses: Vec::new(),
                pr: None,
                next_review_id: 1,
                prs_created: 0,
                reviewers_requested: Vec::new(),
                fail_comments: false,
                fail_statuses: false,
                fail_reviewers: false,
            })),
        }
    }

    fn set_status(&self, status: &str) {
        self.state.lock().unwrap().item.status = Some(status.to_string());
    }

    fn merge_pr(&self) {
        let mut state = self.state.lock().unwrap();
        state.next_review_id += 1;
        let review_id = state.next_review_id;
        if let Some(pr) = &mut state.pr {
            pr.merged = true;
            pr.review_decision = ReviewDecision::Approved;
            pr.latest_review_id = Some(review_id);
        }
    }

    fn request_changes(&self) {
        let mut state = self.state.lock().unwrap();
        state.next_review_id += 1;
        let review_id = state.next_review_id;
        if let Some(pr) = &mut state.pr {
            pr.review_decision = ReviewDecision::ChangesRequested;
            pr.latest_review_id = Some(review_id);
        }
    }

    fn comments(&self) -> Vec<CommentView> {
        self.state.lock().unwrap().comments.clone()
    }

    fn statuses(&self) -> Vec<String> {
        self.state.lock().unwrap().statuses.clone()
    }

    fn fail_comments(&self) {
        self.state.lock().unwrap().fail_comments = true;
    }

    fn fail_statuses(&self) {
        self.state.lock().unwrap().fail_statuses = true;
    }

    fn fail_reviewers(&self) {
        self.state.lock().unwrap().fail_reviewers = true;
    }
}

#[async_trait]
impl GitHub for FakeGitHub {
    async fn list_project_items(&self, _target: &TargetConfig) -> Result<Vec<ProjectItem>> {
        Ok(vec![self.state.lock().unwrap().item.clone()])
    }

    async fn list_comments(
        &self,
        _target: &TargetConfig,
        _content: &ProjectContent,
    ) -> Result<Vec<CommentView>> {
        Ok(self.state.lock().unwrap().comments.clone())
    }

    async fn user_can_administer_or_write(
        &self,
        _target: &TargetConfig,
        login: &str,
    ) -> Result<bool> {
        Ok(self.state.lock().unwrap().admins.contains(login))
    }

    async fn create_issue_comment(
        &self,
        _target: &TargetConfig,
        _issue_number: i64,
        body: &str,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_comments {
            return Err(Error::message("comment creation failed"));
        }
        let id = state
            .comments
            .iter()
            .map(|comment| comment.id)
            .max()
            .unwrap_or(0)
            + 1;
        state.comments.push(CommentView {
            id,
            author: TEST_BOT_LOGIN.to_string(),
            body: body.to_string(),
            created_at: OffsetDateTime::now_utc(),
        });
        Ok(())
    }

    async fn set_project_status(
        &self,
        _target: &TargetConfig,
        _item: &ProjectItem,
        status: &str,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_statuses {
            return Err(Error::message("status update failed"));
        }
        state.item.status = Some(status.to_string());
        state.statuses.push(status.to_string());
        Ok(())
    }

    async fn find_agent_pr(
        &self,
        _target: &TargetConfig,
        _branch: &str,
    ) -> Result<Option<PullRequestInfo>> {
        Ok(self.state.lock().unwrap().pr.clone())
    }

    async fn create_pull_request(
        &self,
        _target: &TargetConfig,
        _pr: &NewPullRequest,
    ) -> Result<PullRequestInfo> {
        let mut state = self.state.lock().unwrap();
        state.prs_created += 1;
        let pr = PullRequestInfo {
            number: 1,
            merged: false,
            review_decision: ReviewDecision::None,
            latest_review_id: None,
        };
        state.pr = Some(pr.clone());
        Ok(pr)
    }

    async fn request_reviewers(
        &self,
        _target: &TargetConfig,
        _pr_number: i64,
        reviewers: &[String],
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_reviewers {
            return Err(Error::message("reviewer request failed"));
        }
        state.reviewers_requested.extend(reviewers.iter().cloned());
        Ok(())
    }
}

#[derive(Clone)]
struct FakeRunner;

#[async_trait]
impl AgentRunner for FakeRunner {
    async fn triage(
        &self,
        _commands: &CommandsConfig,
        _repo_path: &Path,
        _prompt: &str,
    ) -> Result<TriageResult> {
        Ok(TriageResult {
            is_code_change: true,
            confidence: 0.95,
            summary: "Implement requested change".to_string(),
            questions: Vec::new(),
            risks: Vec::new(),
        })
    }

    async fn implement(
        &self,
        _commands: &CommandsConfig,
        _repo_path: &Path,
        _prompt: &str,
    ) -> Result<ImplementationResult> {
        Ok(ImplementationResult {
            summary: "implemented".to_string(),
            tests_run: Vec::new(),
        })
    }
}

#[derive(Clone)]
struct FakeWorkspace {
    root: PathBuf,
    committed: Arc<Mutex<usize>>,
    pushed: Arc<Mutex<usize>>,
}

#[async_trait]
impl WorkspaceManager for FakeWorkspace {
    fn identity(&self, _target: &TargetConfig, item_id: &str, _title: &str) -> WorkIdentity {
        WorkIdentity {
            branch: format!("agent/{item_id}"),
            worktree_path: self.root.join(item_id),
        }
    }

    async fn ensure_worktree(&self, _target: &TargetConfig, identity: &WorkIdentity) -> Result<()> {
        std::fs::create_dir_all(&identity.worktree_path)?;
        Ok(())
    }

    async fn commit_all(&self, _worktree: &Path, _message: &str) -> Result<bool> {
        *self.committed.lock().unwrap() += 1;
        Ok(true)
    }

    async fn push(&self, _worktree: &Path, _branch: &str) -> Result<()> {
        *self.pushed.lock().unwrap() += 1;
        Ok(())
    }
}

fn project_item(status: &str) -> ProjectItem {
    ProjectItem {
        id: "ITEM_1".to_string(),
        title: "Fix bug".to_string(),
        status: Some(status.to_string()),
        content: Some(ProjectContent {
            id: "ISSUE_1".to_string(),
            number: 1,
            title: "Fix bug".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            author: "admin".to_string(),
            created_at: OffsetDateTime::now_utc(),
            url: "https://github.com/owner/repo/issues/1".to_string(),
            kind: ProjectContentKind::Issue,
        }),
    }
}

#[tokio::test]
async fn daemon_runs_manage_to_pr_to_done_workflow() {
    let temp = tempdir().unwrap();
    let target = target(temp.path().join("checkout"));
    let config = config(temp.path(), target);
    let github = FakeGitHub::new(
        project_item("Todo"),
        vec![CommentView {
            id: 1,
            author: "admin".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            created_at: OffsetDateTime::now_utc(),
        }],
    );
    let workspace = FakeWorkspace {
        root: temp.path().join("worktrees"),
        committed: Arc::new(Mutex::new(0)),
        pushed: Arc::new(Mutex::new(0)),
    };
    let daemon =
        Daemon::with_components(config, github.clone(), FakeRunner, workspace.clone()).unwrap();

    daemon.run_once().await.unwrap();
    assert!(github
        .comments()
        .iter()
        .any(|comment| comment.body.contains("Move this Project item")));
    assert_eq!(github.statuses(), vec!["Triaged"]);

    github.set_status("In Progress");
    daemon.run_once().await.unwrap();
    assert_eq!(*workspace.committed.lock().unwrap(), 1);
    assert_eq!(*workspace.pushed.lock().unwrap(), 1);
    assert_eq!(github.statuses(), vec!["Triaged", "In Review"]);

    github.merge_pr();
    daemon.run_once().await.unwrap();
    assert_eq!(github.statuses(), vec!["Triaged", "In Review", "Done"]);
    assert!(github
        .comments()
        .iter()
        .any(|comment| comment.body.contains("Marking this task complete")));
}

#[tokio::test]
async fn daemon_does_not_repeat_same_changes_requested_review() {
    let temp = tempdir().unwrap();
    let target = target(temp.path().join("checkout"));
    let config = config(temp.path(), target);
    let github = FakeGitHub::new(
        project_item("Todo"),
        vec![CommentView {
            id: 1,
            author: "admin".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            created_at: OffsetDateTime::now_utc(),
        }],
    );
    let workspace = FakeWorkspace {
        root: temp.path().join("worktrees"),
        committed: Arc::new(Mutex::new(0)),
        pushed: Arc::new(Mutex::new(0)),
    };
    let daemon =
        Daemon::with_components(config, github.clone(), FakeRunner, workspace.clone()).unwrap();

    daemon.run_once().await.unwrap();
    github.set_status("In Progress");
    daemon.run_once().await.unwrap();
    assert_eq!(*workspace.committed.lock().unwrap(), 1);

    github.request_changes();
    daemon.run_once().await.unwrap();
    assert_eq!(*workspace.committed.lock().unwrap(), 2);

    github.request_changes();
    daemon.run_once().await.unwrap();
    assert_eq!(*workspace.committed.lock().unwrap(), 3);

    daemon.run_once().await.unwrap();
    assert_eq!(*workspace.committed.lock().unwrap(), 3);
}

#[tokio::test]
async fn validation_failure_persists_blocked_when_github_side_effects_fail() {
    let temp = tempdir().unwrap();
    let mut target = target(temp.path().join("checkout"));
    target.commands.test = vec!["false".to_string()];
    target.commands.max_fix_attempts = 0;
    let config = config(temp.path(), target);
    let state_path = config.state_path();
    let github = FakeGitHub::new(
        project_item("Todo"),
        vec![CommentView {
            id: 1,
            author: "admin".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            created_at: OffsetDateTime::now_utc(),
        }],
    );
    let workspace = FakeWorkspace {
        root: temp.path().join("worktrees"),
        committed: Arc::new(Mutex::new(0)),
        pushed: Arc::new(Mutex::new(0)),
    };
    let daemon =
        Daemon::with_components(config, github.clone(), FakeRunner, workspace.clone()).unwrap();

    daemon.run_once().await.unwrap();
    github.set_status("In Progress");
    github.fail_comments();
    github.fail_statuses();
    daemon.run_once().await.unwrap();

    let store = Store::open(state_path).unwrap();
    let stored = store.get_item("owner", "repo", "ITEM_1").unwrap().unwrap();
    assert_eq!(stored.state, ManagedState::Blocked);
    assert_eq!(*workspace.committed.lock().unwrap(), 0);
}

#[tokio::test]
async fn reviewer_request_failure_does_not_block_pr_state() {
    let temp = tempdir().unwrap();
    let target = target(temp.path().join("checkout"));
    let config = config(temp.path(), target);
    let state_path = config.state_path();
    let github = FakeGitHub::new(
        project_item("Todo"),
        vec![CommentView {
            id: 1,
            author: "admin".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            created_at: OffsetDateTime::now_utc(),
        }],
    );
    let workspace = FakeWorkspace {
        root: temp.path().join("worktrees"),
        committed: Arc::new(Mutex::new(0)),
        pushed: Arc::new(Mutex::new(0)),
    };
    let daemon =
        Daemon::with_components(config, github.clone(), FakeRunner, workspace.clone()).unwrap();

    daemon.run_once().await.unwrap();
    github.set_status("In Progress");
    github.fail_reviewers();
    daemon.run_once().await.unwrap();

    let store = Store::open(state_path).unwrap();
    let stored = store.get_item("owner", "repo", "ITEM_1").unwrap().unwrap();
    assert_eq!(stored.state, ManagedState::PrOpen);
    assert_eq!(stored.pr_number, Some(1));
    assert_eq!(*workspace.committed.lock().unwrap(), 1);
}

#[tokio::test]
async fn completion_persists_done_when_github_side_effects_fail() {
    let temp = tempdir().unwrap();
    let target = target(temp.path().join("checkout"));
    let config = config(temp.path(), target);
    let state_path = config.state_path();
    let github = FakeGitHub::new(
        project_item("Todo"),
        vec![CommentView {
            id: 1,
            author: "admin".to_string(),
            body: format!("@{} please fix the bug", TEST_BOT_LOGIN),
            created_at: OffsetDateTime::now_utc(),
        }],
    );
    let workspace = FakeWorkspace {
        root: temp.path().join("worktrees"),
        committed: Arc::new(Mutex::new(0)),
        pushed: Arc::new(Mutex::new(0)),
    };
    let daemon =
        Daemon::with_components(config, github.clone(), FakeRunner, workspace.clone()).unwrap();

    daemon.run_once().await.unwrap();
    github.set_status("In Progress");
    daemon.run_once().await.unwrap();
    github.merge_pr();
    github.fail_comments();
    github.fail_statuses();
    daemon.run_once().await.unwrap();

    let store = Store::open(state_path).unwrap();
    let stored = store.get_item("owner", "repo", "ITEM_1").unwrap().unwrap();
    assert_eq!(stored.state, ManagedState::Done);
}

#[test]
fn triage_comment_blocks_non_code_tasks() {
    let item = ProjectItem {
        id: "I".to_string(),
        title: "Question".to_string(),
        status: None,
        content: None,
    };
    let result = TriageResult {
        is_code_change: false,
        confidence: 0.9,
        summary: "Discuss deployment".to_string(),
        questions: vec![],
        risks: vec![],
    };
    let (state, body) = CommentComposer::default().triage_comment(&item, &result, "In Progress");
    assert_eq!(state, ManagedState::Blocked);
    assert!(body.contains("not think this is a code-change task"));
}

#[test]
fn triage_comment_waits_for_project_start_status() {
    let item = ProjectItem {
        id: "I".to_string(),
        title: "Fix bug".to_string(),
        status: None,
        content: None,
    };
    let result = TriageResult {
        is_code_change: true,
        confidence: 0.9,
        summary: "Fix bug".to_string(),
        questions: vec![],
        risks: vec![],
    };
    let (state, body) = CommentComposer::default().triage_comment(&item, &result, "In Progress");
    assert_eq!(state, ManagedState::AwaitingStart);
    assert!(body.contains("Move this Project item to `In Progress`"));
}

#[test]
fn validation_comment_truncates_on_utf8_boundary() {
    let item = ProjectItem {
        id: "I".to_string(),
        title: "Fix bug".to_string(),
        status: None,
        content: None,
    };
    let err = format!("{}é", "a".repeat(5999));
    let body = CommentComposer::default().blocked_validation_comment(&item, &err);

    assert!(body.contains("..."));
    assert!(body.contains("Implementation is blocked after validation retries"));
}
