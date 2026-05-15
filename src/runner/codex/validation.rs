use std::path::{Path, PathBuf};

use crate::error::{OptionContext, Result};
use crate::git_workspace::{CommandOutput, CommandRunner};

#[non_exhaustive]
#[derive(Debug, Clone)]
pub(crate) struct ValidationRunner {
    repo_path: PathBuf,
    command: Vec<String>,
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
