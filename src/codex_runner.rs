use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::config::CommandsConfig;
use crate::error::{OptionContext, Result, ResultContext};
use crate::git_workspace::{CommandOutput, CommandRunner};
use crate::workflow::TriageResult;

#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct CodexRunner;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ImplementationResult {
    pub summary: String,
    pub tests_run: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct TriagePrompt {
    title: String,
    body: String,
    comments: String,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ImplementationPrompt {
    summary: String,
    discussion: String,
    tests: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ValidationRunner {
    repo_path: PathBuf,
    command: Vec<String>,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn triage(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<TriageResult>;
    async fn implement(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<ImplementationResult>;
}

#[async_trait]
impl AgentRunner for CodexRunner {
    async fn triage(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<TriageResult> {
        let output = self.run(commands, repo_path, prompt).await?;
        let json_slice = self
            .json_object(&output.stdout)
            .context("codex triage output did not contain a JSON object")?;
        serde_json::from_str(json_slice).context("failed to parse codex triage JSON")
    }

    async fn implement(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<ImplementationResult> {
        let output = self.run(commands, repo_path, prompt).await?;
        Ok(ImplementationResult {
            summary: self
                .first_nonempty_line(&output.stdout)
                .unwrap_or("Codex completed changes")
                .to_string(),
            tests_run: Vec::new(),
        })
    }
}

impl CodexRunner {
    async fn run(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<CommandOutput> {
        let (program, args) = self
            .command(commands)
            .split_first()
            .context("codex command must not be empty")?;
        let output = CommandRunner::new(repo_path)
            .run(program, args, Some(prompt))
            .await?;
        output.ensure_success(program)?;
        Ok(output)
    }

    fn command<'a>(&self, commands: &'a CommandsConfig) -> &'a [String] {
        &commands.codex
    }

    fn json_object<'a>(&self, text: &'a str) -> Option<&'a str> {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            return None;
        }
        Some(&text[start..=end])
    }

    fn first_nonempty_line<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.lines().map(str::trim).find(|line| !line.is_empty())
    }
}

impl TriagePrompt {
    pub(crate) fn new(title: &str, body: &str, comments: &str) -> Self {
        Self {
            title: title.to_string(),
            body: body.to_string(),
            comments: comments.to_string(),
        }
    }

    pub(crate) fn render(&self) -> String {
        format!(
            r#"You are triaging a GitHub project item for an autonomous coding daemon.

Return a single JSON object with this shape:
{{"is_code_change":true,"confidence":0.0,"summary":"...","questions":[],"risks":[]}}

Rules:
- Set is_code_change to true only when the request needs repository code/config/docs changes.
- Ask concise clarification questions when the implementation is ambiguous.
- Do not modify files during triage.

Title:
{}

Body:
{}

Discussion:
{}
"#,
            self.title, self.body, self.comments
        )
    }
}

impl ImplementationPrompt {
    pub(crate) fn new(summary: &str, discussion: &str, tests: &[String]) -> Self {
        Self {
            summary: summary.to_string(),
            discussion: discussion.to_string(),
            tests: tests.to_vec(),
        }
    }

    pub(crate) fn render(&self) -> String {
        format!(
            r#"Implement this GitHub task in the current repository worktree.

Requirement summary:
{}

Discussion:
{}

After editing, make commits unnecessary; the daemon will commit all changes.
Expected validation commands:
{}
"#,
            self.summary,
            self.discussion,
            if self.tests.is_empty() {
                "(none configured)".to_string()
            } else {
                self.tests.join("\n")
            }
        )
    }

    pub(crate) fn render_repair(&self, validation_output: &str) -> String {
        format!(
            "{}\n\nThe previous validation failed. Fix the repository based on this output:\n{}",
            self.render(),
            validation_output
        )
    }
}

impl ValidationRunner {
    pub(crate) fn new(repo_path: impl AsRef<Path>, command: &[String]) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            command: command.to_vec(),
        }
    }

    pub(crate) async fn run(&self) -> Result<Option<String>> {
        if self.command.is_empty() {
            return Ok(None);
        }
        let (program, args) = self
            .command
            .split_first()
            .context("validation command is empty")?;
        let output = CommandRunner::new(&self.repo_path)
            .run(program, args, None)
            .await?;
        output.ensure_success(program)?;
        Ok(Some(output.combined_text()))
    }
}

impl CommandOutput {
    fn combined_text(&self) -> String {
        let mut text = String::new();
        if !self.stdout.trim().is_empty() {
            text.push_str(self.stdout.trim());
        }
        if !self.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(self.stderr.trim());
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_noisy_output() {
        let text = "analysis\n{\"is_code_change\":true,\"confidence\":0.9,\"summary\":\"x\"}\n";
        let parsed: TriageResult =
            serde_json::from_str(CodexRunner.json_object(text).unwrap()).unwrap();
        assert!(parsed.is_code_change);
        assert_eq!(parsed.summary, "x");
    }
}
