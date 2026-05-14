use std::path::PathBuf;

use anyhow::{anyhow, Result};
use autono::{Config, Daemon, GitHubClient, Store};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "autono")]
#[command(about = "Drive Codex-backed repo workflows from GitHub Projects")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run(ConfigArgs),
    Once(ConfigArgs),
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },
    Recover(RecoverArgs),
}

#[derive(Debug)]
struct RepoItemKey {
    owner: String,
    repo: String,
    item_id: String,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[arg(short, long, default_value = "autono.toml")]
    config: PathBuf,
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    Item(InspectItemArgs),
}

#[derive(Debug, Args)]
struct InspectItemArgs {
    #[arg(short, long, default_value = "autono.toml")]
    config: PathBuf,
    #[arg(long)]
    repo: String,
    #[arg(long)]
    item_id: String,
}

#[derive(Debug, Args)]
struct RecoverArgs {
    #[arg(short, long, default_value = "autono.toml")]
    config: PathBuf,
    #[arg(long)]
    repo: String,
    #[arg(long)]
    item_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => {
            let config = Config::load(&args.config)?;
            let github = GitHubClient::from_config(config.github_config()).await?;
            Daemon::new(config, github)?.run_forever().await?;
            Ok(())
        }
        Command::Once(args) => {
            let config = Config::load(&args.config)?;
            let github = GitHubClient::from_config(config.github_config()).await?;
            Daemon::new(config, github)?.run_once().await?;
            Ok(())
        }
        Command::Inspect {
            command: InspectCommand::Item(args),
        } => {
            let config = Config::load(&args.config)?;
            let store = Store::open(config.state_path())?;
            let key = RepoItemKey::parse(&args.repo, &args.item_id)?;
            match store.get_item(&key.owner, &key.repo, &key.item_id)? {
                Some(item) => println!("{}", serde_json::to_string_pretty(&item)?),
                None => println!("item not found in local operation cache"),
            }
            Ok(())
        }
        Command::Recover(args) => {
            let config = Config::load(&args.config)?;
            let store = Store::open(config.state_path())?;
            let key = RepoItemKey::parse(&args.repo, &args.item_id)?;
            match store.get_item(&key.owner, &key.repo, &key.item_id)? {
                Some(item) => {
                    println!("{}", serde_json::to_string_pretty(&item)?);
                    println!("recovery data is available; next poll will resume this item");
                }
                None => println!("item not found in local operation cache"),
            }
            Ok(())
        }
    }
}

impl RepoItemKey {
    fn parse(repo: &str, item_id: &str) -> Result<Self> {
        let (owner, repo) = repo
            .split_once('/')
            .ok_or_else(|| anyhow!("--repo must be in owner/name form"))?;
        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            item_id: item_id.to_string(),
        })
    }
}
