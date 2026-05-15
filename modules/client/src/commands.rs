//! provides wrapper functions for handling requests that can be moade for a
//! list RPC servers
use std::{
    fmt::Write,
    iter::IntoIterator,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use com::{CheckError, CheckOpts, DimOpts, LayerOpts, StartError};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use tarpc::context;

use crate::node::Client;

/// sends a health request to each RPC server and prints the response
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

/// prints the prefetched RPC server information
///
/// it will print the hostname of the RPC server and any available cameras for
/// the server
pub async fn request_info<'a, I>(iter: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a Client>,
{
    for client in iter {
        let mut output = format!("{}:\n", client.addr);

        if let Some(info) = &client.info {
            writeln!(&mut output, "    hostname: {}", info.hostname).unwrap();

            if info.cameras.is_empty() {
                write!(&mut output, "    no cameras available").unwrap();
            } else {
                for cam in &info.cameras {
                    if cam.avail {
                        writeln!(&mut output, "    {}: {} available", cam.name, cam.serial)
                            .unwrap();
                    } else {
                        writeln!(&mut output, "    {}: {} unavailable", cam.name, cam.serial)
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

/// sends a start request to each RPC server and prints the result
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
                Ok(results) => {
                    println!(
                        "{addr} finished {:.09} secs",
                        results.exec_time.as_secs_f64()
                    );
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

/// the list of options that can be sent to the RPC server for a check request
pub struct CheckOptions {
    /// the layer height to use
    pub height: Option<f32>,
    /// the layer number to use
    pub number: Option<u32>,
    /// the with of the part
    pub dim_width: f32,
    /// the height of the part
    pub dim_height: f32,
    /// the STL to send to the server
    pub stl: PathBuf,
}

/// send a check request to each RPC server and prints the result
pub async fn request_check<'a, I>(
    iter: I,
    CheckOptions {
        height,
        number,
        dim_width,
        dim_height,
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
    let dim = DimOpts {
        width: dim_width,
        height: dim_height,
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
                        dim: dim.clone(),
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
                Ok((compare, stereopsis)) => {
                    println!("{addr} print check");

                    {
                        println!(
                            "compare: {}\ngeo_val_time: {:.06} ms\nexec_time: {:.09} secs",
                            compare.success,
                            compare.geo_val_time.as_secs_f64() * 1000.0,
                            compare.exec_time.as_secs_f64(),
                        );
                    }

                    if let Some(results) = stereopsis {
                        println!(
                            "stereopsis: {}\nexec_time: {:.09} secs",
                            results.success,
                            compare.exec_time.as_secs_f64(),
                        );
                    } else {
                        println!("stereopsis: skipped");
                    }
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
                    CheckError::Stereopsis => {
                        println!("{addr} failed during stereopsis process");
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

/// sends a finish request to each RPC server
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
