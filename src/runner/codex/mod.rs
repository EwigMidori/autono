use std::path::Path;

use async_trait::async_trait;

use crate::config::CommandsConfig;
use crate::error::{OptionContext, Result, ResultContext};
use crate::git_workspace::{CommandOutput, CommandRunner};
use crate::workflow::TriageResult;

mod command;
mod prompts;
mod results;
mod validation;

pub(crate) use prompts::{
    DiscussionPrompt, DiscussionPromptContext, ImplementationPrompt, TriagePrompt,
};
pub(crate) use results::{
    CompletionCheckResult, CompletionOutcome, DiscussionReplyDecision, ImplementationResult,
    SelfReviewOutcome, SelfReviewResult,
};
pub(crate) use validation::ValidationRunner;

use command::normalize_codex_command;

#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct CodexRunner;

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn triage(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<TriageResult>;
    async fn monitor_discussion(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<DiscussionReplyDecision>;
    async fn implement(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<ImplementationResult>;
    async fn completion_check(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<CompletionCheckResult>;
    async fn self_review(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<SelfReviewResult>;
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

    async fn monitor_discussion(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<DiscussionReplyDecision> {
        let output = self.run(commands, repo_path, prompt).await?;
        let json_slice = self
            .json_object(&output.stdout)
            .context("codex discussion output did not contain a JSON object")?;
        serde_json::from_str(json_slice).context("failed to parse codex discussion JSON")
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

    async fn completion_check(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<CompletionCheckResult> {
        let output = self.run(commands, repo_path, prompt).await?;
        let json_slice = self
            .json_object(&output.stdout)
            .context("codex completion-check output did not contain a JSON object")?;
        serde_json::from_str(json_slice).context("failed to parse codex completion-check JSON")
    }

    async fn self_review(
        &self,
        commands: &CommandsConfig,
        repo_path: &Path,
        prompt: &str,
    ) -> Result<SelfReviewResult> {
        let output = self.run(commands, repo_path, prompt).await?;
        let json_slice = self
            .json_object(&output.stdout)
            .context("codex self-review output did not contain a JSON object")?;
        serde_json::from_str(json_slice).context("failed to parse codex self-review JSON")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        let validation_output = format!(
            "{}tail",
            "a".repeat(prompts::VALIDATION_OUTPUT_PROMPT_LIMIT + 100)
        );

        let rendered = prompt.render_repair(&validation_output);

        assert!(rendered.contains("[truncated"));
        assert!(rendered.contains("tail"));
        assert!(!rendered.contains(&"a".repeat(prompts::VALIDATION_OUTPUT_PROMPT_LIMIT + 100)));
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

    #[test]
    fn discussion_prompt_includes_read_only_checkout_and_gate() {
        let prompt = DiscussionPrompt::new(
            "summary",
            "body",
            "discussion",
            DiscussionPromptContext {
                state: "AwaitingStart".to_string(),
                base_branch: "main".to_string(),
                start_status: "In Progress".to_string(),
                readonly_checkout: Path::new("/tmp/main-checkout").to_path_buf(),
            },
        );

        let rendered = prompt.render();

        assert!(rendered.contains("/tmp/main-checkout"));
        assert!(rendered.contains("base branch for reference is `main`"));
        assert!(rendered.contains("state `AwaitingStart`"));
        assert!(rendered.contains("In Progress"));
        assert!(rendered.contains("Return a single JSON object"));
    }
}
