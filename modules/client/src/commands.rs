use std::{
    fmt::Write,
    iter::IntoIterator,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use com::{CheckError, CheckOpts, LayerOpts, StartError};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use tarpc::context;

use crate::node::Client;

pub async fn request_health<'a, I>(iter: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
    for client in iter {
        let status = client
            .channel
            .health(context::current())
            .await
            .context("failed requesting rpc server health")?;

        println!("{} server status: {status}", client.addr);
    }

    Ok(())
}

pub async fn request_info<'a, I>(iter: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
    for client in iter {
        let mut output = format!("{}:\n", client.addr);

        if let Some(info) = &client.info {
            write!(&mut output, "    hostname: {}\n", info.hostname).unwrap();

            if info.cameras.is_empty() {
                write!(&mut output, "    no cameras available").unwrap();
            } else {
                for cam in &info.cameras {
                    if cam.avail {
                        write!(&mut output, "    {}: {} available\n", cam.name, cam.serial)
                            .unwrap();
                    } else {
                        write!(
                            &mut output,
                            "    {}: {} unavailable\n",
                            cam.name, cam.serial
                        )
                        .unwrap();
                    }
                }
            }
        } else {
            write!(&mut output, "    no additional information").unwrap();
        }

        println!("{output}");
    }

    Ok(())
}

pub async fn request_start<'a, I>(iter: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
    let start = Instant::now();
    let mut futs = FuturesUnordered::new();

    for client in iter {
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

    Ok(())
}

pub struct CheckOptions {
    pub height: Option<f32>,
    pub number: Option<u32>,
    pub stl: PathBuf,
}

pub async fn request_check<'a, I>(
    iter: I,
    CheckOptions {
        height,
        number,
        stl,
    }: CheckOptions,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
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

    for client in iter {
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
                Ok((success, duration)) => {
                    println!(
                        "{addr} print check: {success} {:.06} ms",
                        duration.as_secs_f64() * 1000.0,
                    )
                }
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

    Ok(())
}

pub async fn request_finish<'a, I>(iter: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
    for client in iter {
        client
            .channel
            .print_finish(context::current())
            .await
            .context("failed requesting rpc server print finish")?;
    }

    Ok(())
}
