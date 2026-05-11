use std::{
    fmt::Write,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use com::{AddrArgs, CheckError, CheckOpts, DEFAULT_PORT, LayerOpts, RpcClient, StartError};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
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
    Info,
    Start {},
    Check {
        #[arg(long = "layer-height", requires = "number")]
        height: Option<f32>,

        #[arg(long = "layer", requires = "height")]
        number: Option<u32>,

        stl: PathBuf,
    },
    Finish,
}

struct Client {
    addr: SocketAddr,
    info: Option<com::Info>,
    channel: RpcClient,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    com::init_logging();

    let args = CliArgs::parse();

    let addrs = match (args.addr, args.nodes) {
        (Some(specific), _) => vec![specific.get_client_addr()],
        (None, Some(load)) => load_nodes(&load).await?,
        (None, None) => bail!("no addr or nodes file specified"),
    };

    let clients = load_clients(&addrs).await?;

    run_cmd(&clients, args.cmd).await?;

    Ok(())
}

async fn run_cmd(clients: &[Client], cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Health => {
            for client in clients {
                let status = client
                    .channel
                    .health(context::current())
                    .await
                    .context("failed requesting rpc server health")?;

                println!("{} server status: {status}", client.addr);
            }
        }
        Cmd::Info => {
            for client in clients {
                let mut output = format!("{}:\n", client.addr);

                if let Some(info) = &client.info {
                    write!(&mut output, "    hostname: {}\n", info.hostname).unwrap();

                    if info.cameras.is_empty() {
                        write!(&mut output, "    no cameras available").unwrap();
                    } else {
                        for cam in &info.cameras {
                            if cam.avail {
                                write!(&mut output, "    {}: {} available\n", cam.name, cam.serial).unwrap();
                            } else {
                                write!(&mut output, "    {}: {} unavailable\n", cam.name, cam.serial).unwrap();
                            }
                        }
                    }
                } else {
                    write!(&mut output, "    no additional information").unwrap();
                }

                println!("{output}");
            }
        }
        Cmd::Start {} => {
            let start = Instant::now();
            let mut futs = FuturesUnordered::new();

            for client in clients {
                let client_name = client.get_name();
                let mut client_context = context::current();
                client_context.deadline += Duration::new(60, 0);

                tracing::trace!("sending request to {client_name}");

                futs.push(
                    client
                        .channel
                        .print_start(client_context)
                        .map(|res| (client_name, res)),
                );
            }

            while let Some((addr, res)) = futs.next().await {
                let duration = start.elapsed();

                tracing::info!("response from {addr} {duration:#?}");

                match res {
                    Ok(status) => match status {
                        Ok(()) => {
                            println!("{addr} finished");
                        }
                        Err(err) => match err {
                            StartError::Stl => {
                                println!("{addr} failed during stl write process");
                            }
                            StartError::Background => {
                                println!("{addr} failed during background-builder process");
                            }
                        },
                    },
                    Err(err) => {
                        println!("{addr} error during request: {err:#?}");
                    }
                }
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
            let mut futs = futures::stream::FuturesUnordered::new();

            for client in clients {
                let client_name = client.get_name();
                let mut client_context = context::current();
                client_context.deadline += Duration::new(10 * 60, 0);

                tracing::trace!("sending request to {client_name}");

                futs.push(
                    client
                        .channel
                        .print_check(
                            client_context,
                            CheckOpts {
                                layer: layer.clone(),
                                stl: stl_contents.clone(),
                            },
                        )
                        .map(|res| (client_name, res)),
                );
            }

            while let Some((addr, res)) = futs.next().await {
                let duration = start.elapsed();

                tracing::info!("response from {addr} {duration:#?}");

                match res {
                    Ok(status) => match status {
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
                    },
                    Err(err) => {
                        println!("{addr} error during request: {err:#?}");
                    }
                }
            }
        }
        Cmd::Finish => {
            for client in clients {
                client
                    .channel
                    .print_finish(context::current())
                    .await
                    .context("failed requesting rpc server print finish")?;
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

        let info = match channel.info(context::current()).await {
            Ok(info) => Some(info),
            Err(err) => {
                tracing::error!("failed retrieving node information: {err:#?}");

                None
            }
        };

        rtn.push(Client {
            addr: *addr,
            info,
            channel,
        });
    }

    Ok(rtn)
}

impl Client {
    fn get_name(&self) -> String {
        if let Some(info) = &self.info {
            format!("{}[{}]", info.hostname, self.addr)
        } else {
            format!("{}", self.addr)
        }
    }
}
