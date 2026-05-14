use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::config::CommandsConfig;
use crate::error::{OptionContext, Result, ResultContext};
use crate::git_workspace::{CommandOutput, CommandRunner};
use crate::workflow::TriageResult;

const PROMPT_SECTION_LIMIT: usize = 200_000;
const VALIDATION_OUTPUT_PROMPT_LIMIT: usize = 80_000;
const CODEX_SUBCOMMANDS: &[&str] = &[
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "mcp-server",
    "app-server",
    "app",
    "completion",
    "update",
    "sandbox",
    "debug",
    "apply",
    "a",
    "resume",
    "fork",
    "cloud",
    "exec-server",
    "features",
    "help",
];

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
        let command = self.command(commands);
        let (program, args) = command
            .split_first()
            .context("codex command must not be empty")?;
        let output = CommandRunner::new(repo_path)
            .run(program, args, Some(prompt))
            .await?;
        output.ensure_success(program)?;
        Ok(output)
    }

    fn command(&self, commands: &CommandsConfig) -> Vec<String> {
        normalize_codex_command(&commands.codex)
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

fn normalize_codex_command(command: &[String]) -> Vec<String> {
    let Some(program) = command.first() else {
        return Vec::new();
    };
    if !is_codex_executable(program) || command[1..].iter().any(|arg| is_codex_subcommand(arg)) {
        return command.to_vec();
    }

    let mut normalized = command.to_vec();
    normalized.push("exec".to_string());
    normalized
}

fn is_codex_executable(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "codex")
        .unwrap_or(false)
}

fn is_codex_subcommand(arg: &str) -> bool {
    CODEX_SUBCOMMANDS.contains(&arg)
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
        let title = truncate_end(&self.title, PROMPT_SECTION_LIMIT);
        let body = truncate_end(&self.body, PROMPT_SECTION_LIMIT);
        let comments = truncate_end(&self.comments, PROMPT_SECTION_LIMIT);
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
            title, body, comments
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
        let summary = truncate_end(&self.summary, PROMPT_SECTION_LIMIT);
        let discussion = truncate_end(&self.discussion, PROMPT_SECTION_LIMIT);
        let tests = if self.tests.is_empty() {
            "(none configured)".to_string()
        } else {
            truncate_end(&self.tests.join("\n"), PROMPT_SECTION_LIMIT)
        };
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
            summary, discussion, tests
        )
    }

    pub(crate) fn render_repair(&self, validation_output: &str) -> String {
        let validation_output = truncate_start(validation_output, VALIDATION_OUTPUT_PROMPT_LIMIT);
        format!(
            "{}\n\nThe previous validation failed. Fix the repository based on this output:\n{}",
            self.render(),
            validation_output
        )
    }
}

fn truncate_end(input: &str, limit: usize) -> String {
    if input.len() <= limit {
        return input.to_string();
    }
    let note = format!("\n[truncated {} bytes]", input.len() - limit);
    let body_limit = limit.saturating_sub(note.len());
    let end = input
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= body_limit)
        .last()
        .unwrap_or(0);
    format!("{}{}", &input[..end], note)
}

fn truncate_start(input: &str, limit: usize) -> String {
    if input.len() <= limit {
        return input.to_string();
    }
    let note = format!("[truncated {} bytes]\n", input.len() - limit);
    let body_limit = limit.saturating_sub(note.len());
    let start = input
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| input.len() - *index <= body_limit)
        .unwrap_or(input.len());
    format!("{}{}", note, &input[start..])
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

    #[test]
    fn repair_prompt_truncates_large_validation_output_from_start() {
        let prompt = ImplementationPrompt::new("summary", "discussion", &[]);
        let validation_output = format!("{}tail", "a".repeat(VALIDATION_OUTPUT_PROMPT_LIMIT + 100));

        let rendered = prompt.render_repair(&validation_output);

        assert!(rendered.contains("[truncated"));
        assert!(rendered.contains("tail"));
        assert!(rendered.len() < prompt.render().len() + validation_output.len());
    }

    #[test]
    fn normalizes_bare_codex_command_to_exec() {
        let command = normalize_codex_command(&[
            "codex".to_string(),
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
        ]);

        assert_eq!(
            command,
            vec![
                "codex".to_string(),
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
                "exec".to_string(),
            ]
        );
    }

    #[test]
    fn preserves_explicit_codex_subcommand() {
        let command = normalize_codex_command(&[
            "codex".to_string(),
            "exec".to_string(),
            "--json".to_string(),
        ]);

        assert_eq!(
            command,
            vec![
                "codex".to_string(),
                "exec".to_string(),
                "--json".to_string(),
            ]
        );
    }
}
