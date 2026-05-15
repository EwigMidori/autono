use std::process::Stdio;

use tokio::process::Command;
use tokio::time as tokio_time;

use crate::config::TokenSource;
use crate::error::{Error, Result, ResultContext};

use crate::github::GH_TOKEN_TIMEOUT;

#[derive(Debug, Clone, Copy)]
pub(crate) struct GitHubAuthenticator<'a> {
    source: &'a TokenSource,
}

impl<'a> GitHubAuthenticator<'a> {
    pub(crate) fn new(source: &'a TokenSource) -> Self {
        Self { source }
    }

    pub(crate) async fn token(&self) -> Result<String> {
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
