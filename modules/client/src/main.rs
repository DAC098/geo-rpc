use std::{io::Write, path::PathBuf};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use com::AddrArgs;

mod commands;
mod node;

#[derive(Debug, Parser)]
struct CliArgs {
    #[command(flatten)]
    addr: Option<AddrArgs>,

    /// a list of nodes to connect to
    ///
    /// each line in the file can contain an ip and port or just an ip, any
    /// line that starts with `#` will be ignored
    #[arg(short, long)]
    nodes: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// pings the specifed node(s) to see if they are active
    Health,
    /// displays information for the node(s) specified
    Info,
    /// runs build-background process for the node(s)
    Start,
    /// runs the compare validator process for the node(s)
    Check {
        /// the height of a single print layer
        #[arg(long = "layer-height", requires = "number")]
        height: Option<f32>,

        /// the desired layer to check
        #[arg(long = "layer", requires = "height")]
        number: Option<u32>,

        /// the height of the stl file being used
        #[arg(long = "height")]
        dim_height: f32,

        /// the width of the stl file being used
        #[arg(long = "width")]
        dim_width: f32,

        /// path to the stl file for the current print
        stl: PathBuf,
    },
    /// runs the build-background once and then runs the compare validator,
    /// multiple times if desired
    StartCheck {
        /// the height of a single print layer
        #[arg(long = "layer-height", requires = "number")]
        height: Option<f32>,

        /// the desired layer to check
        #[arg(long = "layer", requires = "height")]
        number: Option<u32>,

        /// the height of the stl file being used
        #[arg(long = "height")]
        dim_height: f32,

        /// the width of the stl file being used
        #[arg(long = "width")]
        dim_width: f32,

        /// will endlessly repeat the compare validator until otherwise
        /// specified
        #[arg(long)]
        repeat: bool,

        /// path to the stl file for the current print
        stl: PathBuf,
    },
    /// runs the cleanup process for each node
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
            dim_height,
            dim_width,
            number,
        } => {
            commands::request_check(
                clients,
                commands::CheckOptions {
                    stl,
                    height,
                    number,
                    dim_width,
                    dim_height,
                },
            )
            .await
        }
        Cmd::StartCheck {
            stl,
            height,
            number,
            dim_width,
            dim_height,
            repeat,
        } => {
            println!("running build-background");

            commands::request_start(clients).await?;

            if repeat {
                while query_continue()? {
                    println!("running compare validator");

                    commands::request_check(
                        clients,
                        commands::CheckOptions {
                            stl: stl.clone(),
                            height,
                            number,
                            dim_width,
                            dim_height,
                        },
                    )
                    .await?;
                }

                Ok(())
            } else {
                if !query_continue()? {
                    return Ok(());
                }

                println!("running compare validator");

                commands::request_check(
                    clients,
                    commands::CheckOptions {
                        stl,
                        height,
                        number,
                        dim_width,
                        dim_height,
                    },
                )
                .await
            }
        }
        Cmd::Finish => commands::request_finish(clients).await,
    }
}

fn get_input(prefix: &str) -> std::io::Result<String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();

    stdout.write_all(prefix.as_bytes())?;
    stdout.flush()?;

    let mut buf = String::new();

    stdin.read_line(&mut buf)?;

    Ok(buf)
}

fn query_continue() -> anyhow::Result<bool> {
    for _ in 0..3 {
        let input = get_input("continue? [y/n] ").context("failed requesting continue")?;
        let given = input.trim().to_lowercase();

        match given.as_str() {
            "y" | "" => return Ok(true),
            "n" => return Ok(false),
            _ => println!("invalid input"),
        }
    }

    Ok(false)
}
