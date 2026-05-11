use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};
use com::AddrArgs;

mod commands;
mod node;

#[derive(Debug, Parser)]
struct CliArgs {
    #[command(flatten)]
    addr: Option<AddrArgs>,

    #[arg(short, long)]
    nodes: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    Health,
    Info,
    Start,
    Check {
        #[arg(long = "layer-height", requires = "number")]
        height: Option<f32>,

        #[arg(long = "layer", requires = "height")]
        number: Option<u32>,

        stl: PathBuf,
    },
    Finish,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    com::init_logging();

    let args = CliArgs::parse();

    let clients = match (args.addr, args.nodes) {
        (Some(specific), _) => vec![node::Client::load(specific.get_client_addr()).await?],
        (None, Some(load)) => node::Client::load_file(&load).await?,
        (None, None) => bail!("no addr or nodes file specified"),
    };

    run_cmd(&clients, args.cmd).await?;

    Ok(())
}

async fn run_cmd(clients: &[node::Client], cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Health => commands::request_health(clients).await,
        Cmd::Info => commands::request_info(clients).await,
        Cmd::Start => commands::request_start(clients).await,
        Cmd::Check {
            stl,
            height,
            number,
        } => {
            commands::request_check(
                clients,
                commands::CheckOptions {
                    stl,
                    height,
                    number,
                },
            )
            .await
        }
        Cmd::Finish => commands::request_finish(clients).await,
    }
}
