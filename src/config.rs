use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result, ResultContext};

const MAX_FIX_ATTEMPTS: usize = 10;

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub(crate) bot_login: String,
    #[serde(default = "serde_defaults::poll_interval_secs")]
    pub(crate) poll_interval_secs: u64,
    pub(crate) worktrees_root: PathBuf,
    pub(crate) state_path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) github: GitHubConfig,
    #[serde(default)]
    pub(crate) targets: Vec<TargetConfig>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubConfig {
    #[serde(default = "serde_defaults::token_source")]
    pub(crate) token_source: TokenSource,
    #[serde(default = "serde_defaults::api_url")]
    pub(crate) api_url: String,
    #[serde(default = "serde_defaults::graphql_url")]
    pub(crate) graphql_url: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Gh,
    Env,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) checkout_path: PathBuf,
    #[serde(default = "serde_defaults::base_branch")]
    pub(crate) base_branch: String,
    pub(crate) project_owner: Option<String>,
    pub(crate) project_id: Option<String>,
    pub(crate) project_number: Option<i64>,
    pub(crate) workflow: WorkflowConfig,
    #[serde(default)]
    pub(crate) review: ReviewConfig,
    pub(crate) commands: CommandsConfig,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowConfig {
    pub(crate) status_field: String,
    pub(crate) triaged_status: String,
    pub(crate) start_status: String,
    pub(crate) review_status: String,
    pub(crate) done_status: String,
    pub(crate) blocked_status: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReviewConfig {
    #[serde(default)]
    pub(crate) reviewers: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
pub struct CommandsConfig {
    #[serde(default = "serde_defaults::codex_command")]
    pub(crate) codex: Vec<String>,
    #[serde(default)]
    pub(crate) test: Vec<String>,
    #[serde(default = "serde_defaults::max_fix_attempts")]
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
            target.validate()?;
        }
        Ok(())
    }
}

impl TargetConfig {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    pub fn checkout_path(&self) -> &Path {
        &self.checkout_path
    }

    pub fn base_branch(&self) -> &str {
        &self.base_branch
    }

    pub fn project_owner(&self) -> &str {
        self.project_owner.as_deref().unwrap_or(&self.owner)
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn project_number(&self) -> Option<i64> {
        self.project_number
    }

    pub fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
    }

    pub fn review(&self) -> &ReviewConfig {
        &self.review
    }

    pub fn commands(&self) -> &CommandsConfig {
        &self.commands
    }

    fn validate(&self) -> Result<()> {
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
        if self.commands.max_fix_attempts > MAX_FIX_ATTEMPTS {
            return Err(Error::message(format!(
                "target {} commands.max_fix_attempts must be <= {}",
                self.full_name(),
                MAX_FIX_ATTEMPTS
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
}

impl WorkflowConfig {
    pub fn status_field(&self) -> &str {
        &self.status_field
    }

    pub fn triaged_status(&self) -> &str {
        &self.triaged_status
    }

    pub fn start_status(&self) -> &str {
        &self.start_status
    }

    pub fn review_status(&self) -> &str {
        &self.review_status
    }

    pub fn done_status(&self) -> &str {
        &self.done_status
    }

    pub fn blocked_status(&self) -> &str {
        &self.blocked_status
    }
}

impl ReviewConfig {
    pub fn reviewers(&self) -> &[String] {
        &self.reviewers
    }
}

impl CommandsConfig {
    pub fn codex(&self) -> &[String] {
        &self.codex
    }

    pub fn test(&self) -> &[String] {
        &self.test
    }

    pub fn max_fix_attempts(&self) -> usize {
        self.max_fix_attempts
    }
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
        config.targets[0].commands.max_fix_attempts = MAX_FIX_ATTEMPTS + 1;

        assert!(config.validate().is_err());
    }
}
