use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::config::TargetConfig;
use crate::error::{Error, OptionContext, Result, ResultContext};

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct WorkIdentity {
    pub branch: String,
    pub worktree_path: PathBuf,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GitWorkspace {
    worktrees_root: PathBuf,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct CommandRunner {
    working_dir: PathBuf,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct CommandOutput {
    pub(crate) status_code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl GitWorkspace {
    pub fn new(worktrees_root: PathBuf) -> Self {
        Self { worktrees_root }
    }

    fn identity(&self, target: &TargetConfig, item_id: &str, title: &str) -> WorkIdentity {
        let digest = self.short_hash(item_id);
        let slug = self.slugify(title);
        let branch = format!("agent/{digest}-{slug}");
        let worktree_path = self
            .worktrees_root
            .join(&target.owner)
            .join(&target.repo)
            .join(branch.replace('/', "__"));
        WorkIdentity {
            branch,
            worktree_path,
        }
    }

    async fn has_changes(&self, worktree: &Path) -> Result<bool> {
        let output = self.git(worktree, &["status", "--porcelain"]).await?;
        output.ensure_success("git status")?;
        Ok(!output.stdout.trim().is_empty())
    }

    async fn branch_exists(&self, working_dir: &Path, branch: &str) -> Result<bool> {
        let output = self
            .git(working_dir, &["rev-parse", "--verify", branch])
            .await?;
        Ok(output.status_code == Some(0))
    }

    async fn git(&self, working_dir: &Path, args: &[&str]) -> Result<CommandOutput> {
        CommandRunner::new(working_dir).git(args).await
    }

    fn path_arg<'a>(&self, path: &'a Path) -> Result<&'a str> {
        path.to_str()
            .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
    }

    fn short_hash(&self, input: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let hash = hasher.finalize();
        format!("{hash:x}")[..10].to_string()
    }

    fn slugify(&self, input: &str) -> String {
        let mut slug = String::new();
        let mut last_dash = false;
        for ch in input.chars().flat_map(|ch| ch.to_lowercase()) {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch);
                last_dash = false;
            } else if !last_dash {
                slug.push('-');
                last_dash = true;
            }
            if slug.len() >= 48 {
                break;
            }
        }
        let slug = slug.trim_matches('-');
        if slug.is_empty() {
            "task".to_string()
        } else {
            slug.to_string()
        }
    }
}

#[async_trait]
pub trait WorkspaceManager: Send + Sync {
    fn identity(&self, target: &TargetConfig, item_id: &str, title: &str) -> WorkIdentity;
    async fn ensure_worktree(&self, target: &TargetConfig, identity: &WorkIdentity) -> Result<()>;
    async fn commit_all(&self, worktree: &Path, message: &str) -> Result<bool>;
    async fn push(&self, worktree: &Path, branch: &str) -> Result<()>;
}

#[async_trait]
impl WorkspaceManager for GitWorkspace {
    fn identity(&self, target: &TargetConfig, item_id: &str, title: &str) -> WorkIdentity {
        self.identity(target, item_id, title)
    }

    async fn ensure_worktree(&self, target: &TargetConfig, identity: &WorkIdentity) -> Result<()> {
        fs::create_dir_all(
            identity
                .worktree_path
                .parent()
                .context("worktree path has no parent")?,
        )?;
        if identity.worktree_path.exists() {
            return Ok(());
        }
        let output = if self
            .branch_exists(&target.checkout_path, &identity.branch)
            .await?
        {
            self.git(
                &target.checkout_path,
                &[
                    "worktree",
                    "add",
                    self.path_arg(&identity.worktree_path)?,
                    &identity.branch,
                ],
            )
            .await?
        } else {
            self.git(
                &target.checkout_path,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &identity.branch,
                    self.path_arg(&identity.worktree_path)?,
                    &target.base_branch,
                ],
            )
            .await?
        };
        output.ensure_success("git worktree add")?;
        Ok(())
    }

    async fn commit_all(&self, worktree: &Path, message: &str) -> Result<bool> {
        if !self.has_changes(worktree).await? {
            return Ok(false);
        }
        self.git(worktree, &["add", "-A"])
            .await?
            .ensure_success("git add")?;
        self.git(worktree, &["commit", "-m", message])
            .await?
            .ensure_success("git commit")?;
        Ok(true)
    }

    async fn push(&self, worktree: &Path, branch: &str) -> Result<()> {
        self.git(worktree, &["push", "-u", "origin", branch])
            .await?
            .ensure_success("git push")
    }
}

impl CommandRunner {
    pub(crate) fn new(working_dir: impl AsRef<Path>) -> Self {
        Self {
            working_dir: working_dir.as_ref().to_path_buf(),
        }
    }

    pub(crate) async fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&str>,
    ) -> Result<CommandOutput> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {program}"))?;
        if let Some(stdin) = stdin {
            let mut child_stdin = child.stdin.take().context("failed to open child stdin")?;
            use tokio::io::AsyncWriteExt;
            child_stdin.write_all(stdin.as_bytes()).await?;
        }
        let output = child
            .wait_with_output()
            .await
            .with_context(|| format!("failed to wait for {program}"))?;
        Ok(CommandOutput {
            status_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    pub(crate) async fn git(&self, args: &[&str]) -> Result<CommandOutput> {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        self.run("git", &args, None).await
    }
}

impl CommandOutput {
    pub(crate) fn ensure_success(&self, label: &str) -> Result<()> {
        if self.status_code == Some(0) {
            Ok(())
        } else {
            Err(Error::message(format!(
                "{label} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
                self.status_code, self.stdout, self.stderr
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandsConfig, ReviewConfig, WorkflowConfig};

    #[test]
    fn identity_is_stable_and_safe() {
        let target = TargetConfig {
            owner: "o".to_string(),
            repo: "r".to_string(),
            checkout_path: "/tmp/repo".into(),
            base_branch: "main".to_string(),
            project_owner: None,
            project_id: Some("P".to_string()),
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
                codex: vec!["codex".to_string()],
                test: vec![],
                max_fix_attempts: 3,
            },
        };
        let workspace = GitWorkspace::new("/tmp/worktrees".into());
        let a = workspace.identity(&target, "ITEM_1", "Fix OAuth callback!");
        let b = workspace.identity(&target, "ITEM_1", "Fix OAuth callback!");
        assert_eq!(a.branch, b.branch);
        assert!(a.branch.starts_with("agent/"));
        assert!(!a.branch.contains(' '));
    }
}
