use std::fs;
use std::path::{Path, PathBuf};

use getset::{CopyGetters, Getters};
use serde::Deserialize;

use crate::error::{Error, Result, ResultContext};

/// Top-level daemon configuration loaded from the TOML config file.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Getters)]
pub struct Config {
    /// GitHub login name used to identify daemon-authored comments and mentions.
    pub(crate) bot_login: String,
    /// Seconds between project polling passes in `run` mode.
    #[serde(default = "serde_defaults::poll_interval_secs")]
    pub(crate) poll_interval_secs: u64,
    /// Root directory where per-item Git worktrees are created.
    pub(crate) worktrees_root: PathBuf,
    /// Optional SQLite state path. Defaults under `worktrees_root`.
    pub(crate) state_path: Option<PathBuf>,
    /// Upper bound for per-target fix attempts.
    #[serde(default = "serde_defaults::max_fix_attempts_limit")]
    pub(crate) max_fix_attempts_limit: usize,
    /// GitHub API authentication and endpoint settings.
    #[serde(default)]
    #[getset(get = "pub")]
    pub(crate) github: GitHubConfig,
    /// Repository/project targets polled by this daemon.
    #[serde(default)]
    pub(crate) targets: Vec<TargetConfig>,
}

/// GitHub API settings.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Getters)]
pub struct GitHubConfig {
    /// Token source. Currently only `gh` is supported.
    #[serde(default = "serde_defaults::token_source")]
    #[getset(get = "pub")]
    pub(crate) token_source: TokenSource,
    /// REST API base URL.
    #[serde(default = "serde_defaults::api_url")]
    #[getset(get = "pub")]
    pub(crate) api_url: String,
    /// GraphQL API endpoint.
    #[serde(default = "serde_defaults::graphql_url")]
    #[getset(get = "pub")]
    pub(crate) graphql_url: String,
}

/// Supported GitHub authentication source.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Gh,
}

/// A repository and GitHub Project target managed by the daemon.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, CopyGetters, Getters)]
pub struct TargetConfig {
    /// Repository owner.
    #[getset(get = "pub")]
    pub(crate) owner: String,
    /// Repository name.
    #[getset(get = "pub")]
    pub(crate) repo: String,
    /// Local checkout used as read-only base/reference.
    #[getset(get = "pub")]
    pub(crate) checkout_path: PathBuf,
    /// Base branch for generated work branches.
    #[serde(default = "serde_defaults::base_branch")]
    #[getset(get = "pub")]
    pub(crate) base_branch: String,
    /// Optional project owner override. Defaults to repository owner.
    pub(crate) project_owner: Option<String>,
    /// Optional Projects v2 node ID.
    pub(crate) project_id: Option<String>,
    /// Optional Projects v2 number, resolved at runtime when `project_id` is unset.
    #[getset(get_copy = "pub")]
    pub(crate) project_number: Option<i64>,
    /// Project status names that drive the item state machine.
    #[getset(get = "pub")]
    pub(crate) workflow: WorkflowConfig,
    /// Pull request review settings.
    #[serde(default)]
    #[getset(get = "pub")]
    pub(crate) review: ReviewConfig,
    /// Commands used to invoke Codex and validation.
    #[getset(get = "pub")]
    pub(crate) commands: CommandsConfig,
}

/// Project workflow field and status names.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Getters)]
pub struct WorkflowConfig {
    /// Projects v2 single-select status field name.
    #[getset(get = "pub")]
    pub(crate) status_field: String,
    /// Status written after triage succeeds.
    #[getset(get = "pub")]
    pub(crate) triaged_status: String,
    /// Status that starts implementation.
    #[getset(get = "pub")]
    pub(crate) start_status: String,
    /// Status written after the PR is ready for human review.
    #[getset(get = "pub")]
    pub(crate) review_status: String,
    /// Status written after the PR is merged.
    #[getset(get = "pub")]
    pub(crate) done_status: String,
    /// Status written when the daemon cannot proceed.
    #[getset(get = "pub")]
    pub(crate) blocked_status: String,
}

/// Pull request reviewer configuration.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize, Getters)]
pub struct ReviewConfig {
    /// GitHub usernames requested after AI self-review passes.
    #[serde(default)]
    #[getset(get = "pub")]
    pub(crate) reviewers: Vec<String>,
}

/// External commands used by a target.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, CopyGetters, Getters)]
pub struct CommandsConfig {
    /// Command used to run Codex.
    #[serde(default = "serde_defaults::codex_command")]
    #[getset(get = "pub")]
    pub(crate) codex: Vec<String>,
    /// Validation command run after implementation and repairs.
    #[serde(default)]
    #[getset(get = "pub")]
    pub(crate) test: Vec<String>,
    /// Maximum repair attempts after validation, completion check, or self-review failures.
    #[serde(default = "serde_defaults::max_fix_attempts")]
    #[getset(get_copy = "pub")]
    pub(crate) max_fix_attempts: usize,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: Config = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn state_path(&self) -> PathBuf {
        self.state_path
            .clone()
            .unwrap_or_else(|| self.worktrees_root.join("autono.sqlite3"))
    }

    pub fn github_config(&self) -> &GitHubConfig {
        &self.github
    }

    pub fn validate(&self) -> Result<()> {
        if self.bot_login.trim().is_empty() {
            return Err(Error::message("bot_login must not be empty"));
        }
        if self.poll_interval_secs == 0 {
            return Err(Error::message("poll_interval_secs must be greater than 0"));
        }
        if self.targets.is_empty() {
            return Err(Error::message("at least one [[targets]] entry is required"));
        }
        for target in &self.targets {
            target.validate(self.max_fix_attempts_limit)?;
        }
        Ok(())
    }

    /// Checks local runtime prerequisites that are not fully covered by TOML parsing.
    pub fn preflight_check(&self) -> Result<()> {
        ensure_directory(&self.worktrees_root, "worktrees_root")?;
        if let Some(parent) = self.state_path().parent() {
            ensure_directory(parent, "state_path parent")?;
        }
        for target in &self.targets {
            target.preflight_check()?;
        }
        Ok(())
    }
}

impl TargetConfig {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn project_owner(&self) -> &str {
        self.project_owner.as_deref().unwrap_or(&self.owner)
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    fn validate(&self, max_fix_attempts_limit: usize) -> Result<()> {
        if self.owner.trim().is_empty() || self.repo.trim().is_empty() {
            return Err(Error::message("target owner/repo must not be empty"));
        }
        if self.project_id.is_none() && self.project_number.is_none() {
            return Err(Error::message(format!(
                "target {} must set project_id or project_number",
                self.full_name()
            )));
        }
        if self.commands.codex.is_empty() {
            return Err(Error::message(format!(
                "target {} commands.codex must not be empty",
                self.full_name()
            )));
        }
        if self.commands.max_fix_attempts > max_fix_attempts_limit {
            return Err(Error::message(format!(
                "target {} commands.max_fix_attempts must be <= {}",
                self.full_name(),
                max_fix_attempts_limit
            )));
        }
        if self.workflow.status_field.trim().is_empty()
            || self.workflow.start_status.trim().is_empty()
            || self.workflow.done_status.trim().is_empty()
        {
            return Err(Error::message(format!(
                "target {} workflow status values must not be empty",
                self.full_name()
            )));
        }
        Ok(())
    }

    fn preflight_check(&self) -> Result<()> {
        ensure_directory(
            &self.checkout_path,
            &format!("target {} checkout_path", self.full_name()),
        )?;
        ensure_command_available(
            &self.commands.codex[0],
            &format!("target {} commands.codex", self.full_name()),
        )?;
        if let Some(test_command) = self.commands.test.first() {
            ensure_command_available(
                test_command,
                &format!("target {} commands.test", self.full_name()),
            )?;
        }
        Ok(())
    }
}

fn ensure_directory(path: &Path, label: &str) -> Result<()> {
    if !path.is_dir() {
        return Err(Error::message(format!(
            "{label} must be an existing directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_command_available(command: &str, label: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Err(Error::message(format!("{label} command must not be empty")));
    }
    if command.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(command);
        if path.is_file() {
            return Ok(());
        }
        return Err(Error::message(format!(
            "{label} command is not executable: {command}"
        )));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&path).any(|dir| dir.join(command).is_file()) {
        return Ok(());
    }
    Err(Error::message(format!(
        "{label} command was not found in PATH: {command}"
    )))
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            token_source: serde_defaults::token_source(),
            api_url: serde_defaults::api_url(),
            graphql_url: serde_defaults::graphql_url(),
        }
    }
}

mod serde_defaults {
    use super::TokenSource;

    pub(super) fn poll_interval_secs() -> u64 {
        60
    }

    pub(super) fn max_fix_attempts_limit() -> usize {
        10
    }

    pub(super) fn token_source() -> TokenSource {
        TokenSource::Gh
    }

    pub(super) fn api_url() -> String {
        "https://api.github.com".to_string()
    }

    pub(super) fn graphql_url() -> String {
        "https://api.github.com/graphql".to_string()
    }

    pub(super) fn base_branch() -> String {
        "main".to_string()
    }

    pub(super) fn codex_command() -> Vec<String> {
        vec![
            "codex".to_string(),
            "exec".to_string(),
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
        ]
    }

    pub(super) fn max_fix_attempts() -> usize {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_target() -> TargetConfig {
        TargetConfig {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            checkout_path: "/tmp/repo".into(),
            base_branch: "main".to_string(),
            project_owner: None,
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
            review: ReviewConfig::default(),
            commands: CommandsConfig {
                codex: serde_defaults::codex_command(),
                test: Vec::new(),
                max_fix_attempts: 3,
            },
        }
    }

    fn valid_config() -> Config {
        Config {
            bot_login: "bot".to_string(),
            poll_interval_secs: 60,
            worktrees_root: "/tmp/worktrees".into(),
            state_path: None,
            github: GitHubConfig::default(),
            max_fix_attempts_limit: 10,
            targets: vec![valid_target()],
        }
    }

    #[test]
    fn rejects_zero_poll_interval() {
        let mut config = valid_config();
        config.poll_interval_secs = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unbounded_fix_attempts() {
        let mut config = valid_config();
        config.targets[0].commands.max_fix_attempts = config.max_fix_attempts_limit + 1;

        assert!(config.validate().is_err());
    }
}
