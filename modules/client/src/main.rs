use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    time::Instant,
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use com::{AddrArgs, CheckError, CheckOpts, DEFAULT_PORT, LayerOpts, RpcClient, init_logging};
use tarpc::{client, context, tokio_serde::formats::Json};

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
    Check {
        #[arg(long = "layer-height", requires = "number")]
        height: Option<f32>,

        #[arg(long = "layer", requires = "height")]
        number: Option<u32>,

        stl: PathBuf,
    },
}

struct Client {
    addr: SocketAddr,
    channel: RpcClient,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let args = CliArgs::parse();

    let addrs = match (args.addr, args.nodes) {
        (Some(specific), _) => vec![specific.get_client_addr()],
        (None, Some(load)) => load_nodes(&load).await?,
        (None, None) => bail!("no addr or nodes file specified"),
    };

    let clients = load_clients(&addrs).await?;

    match args.cmd {
        Cmd::Health => {
            for client in &clients {
                let status = client
                    .channel
                    .health(context::current())
                    .await
                    .context("failed requesting rpc server health")?;

                println!("{} server status: {status}", client.addr);
            }
        }
        Cmd::Check {
            stl,
            height,
            number,
        } => {
            let stl_contents = tokio::fs::read(&stl)
                .await
                .context("failed to load stl contents")?;
            let layer = match (height, number) {
                (Some(height), Some(number)) => Some(LayerOpts { height, number }),
                (None, None) => None,
                _ => unreachable!("clap failed to properly parse layer options"),
            };

            let start = Instant::now();
            let mut results = Vec::with_capacity(clients.len());

            for client in clients {
                let status = client
                    .channel
                    .print_check(
                        context::current(),
                        CheckOpts {
                            layer: layer.clone(),
                            stl: stl_contents.clone(),
                        },
                    )
                    .await
                    .context("failed requesting rpc server print check")?;

                results.push((client.addr, status));
            }

            let duration = start.elapsed();

            println!("duration: {duration:#?}");

            for (addr, status) in results {
                match status {
                    Ok(success) => println!("{addr} print check: {success}"),
                    Err(err) => match err {
                        CheckError::Stl => {
                            println!("{addr} failed during stl write process");
                        }
                        CheckError::StlRender => {
                            println!("{addr} failed during stl-render process");
                        }
                        CheckError::Validator => {
                            println!("{addr} failed during validator process");
                        }
                    },
                }
            }
        }
    }

    Ok(())
}

async fn load_nodes<P>(path: P) -> anyhow::Result<Vec<SocketAddr>>
where
    P: AsRef<Path>,
{
    let path_ref = path.as_ref();
    let contents = tokio::fs::read_to_string(path_ref)
        .await
        .context("failed reading nodes file")?;
    let split = contents.split("\n");
    let mut rtn = Vec::new();

    for (index, line) in split.into_iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("#") {
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        let addr = match SocketAddr::from_str(line.trim()) {
            Ok(valid) => valid,
            Err(_) => match IpAddr::from_str(line.trim()) {
                Ok(valid) => (valid, DEFAULT_PORT).into(),
                Err(_) => bail!(
                    "invalid address in nodes file {}:{} \"{line}\"",
                    path_ref.display(),
                    index + 1
                ),
            },
        };

        rtn.push(addr);
    }

    Ok(rtn)
}

async fn load_clients(addrs: &[SocketAddr]) -> anyhow::Result<Vec<Client>> {
    let mut rtn = Vec::with_capacity(addrs.len());

    for addr in addrs {
        let mut transport = tarpc::serde_transport::tcp::connect(*addr, Json::default);
        transport.config_mut().max_frame_length(usize::MAX);
        let conn = transport
            .await
            .with_context(|| format!("failed connecting to rpc server: {addr}"))?;
        let channel = RpcClient::new(client::Config::default(), conn).spawn();

        rtn.push(Client {
            addr: *addr,
            channel,
        });
    }

    Ok(rtn)
}
