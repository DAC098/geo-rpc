use std::{
    ffi::OsStr,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    time::SystemTime,
};

use anyhow::{Context, bail};
use clap::Parser;
use com::{AddrArgs, CheckError, CheckOpts, Info, LayerOpts, Rpc, init_logging};
use futures::StreamExt;
use tarpc::{
    context,
    server::{self, Channel, incoming::Incoming},
    tokio_serde::formats::Json,
};

#[derive(Debug, Parser)]
struct CliArgs {
    #[command(flatten)]
    addr: AddrArgs,
}

#[derive(Clone)]
struct RpcServer {
    socket: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let args = CliArgs::parse();

    let addr = args.addr.get_server_addr();

    let mut listener = tarpc::serde_transport::tcp::listen(&addr, Json::default)
        .await
        .context("failed listening on socket address")?;

    tracing::info!("listening: {}", listener.local_addr());

    listener.config_mut().max_frame_length(usize::MAX);
    listener
        .filter_map(|r| match r {
            Ok(valid) => futures::future::ready(Some(valid)),
            Err(err) => {
                tracing::error!("failed accpeting inbound connection: {err:#?}");

                futures::future::ready(None)
            }
        })
        .map(server::BaseChannel::with_defaults)
        .max_channels_per_key(1, |t| t.transport().peer_addr().unwrap().ip())
        .map(|channel| {
            let server = RpcServer {
                socket: channel.transport().peer_addr().unwrap(),
            };

            channel.execute(server.serve()).for_each(async |c| {
                tokio::spawn(c);
            })
        })
        .buffer_unordered(10)
        .for_each(|_| async {})
        .await;

    Ok(())
}

impl Rpc for RpcServer {
    async fn health(self, _ctx: context::Context) -> String {
        tracing::info!("{} requesting health", self.socket);

        "okay".into()
    }

    async fn info(self, _ctx: context::Context) -> Info {
        tracing::info!("{} requesting info", self.socket);

        Info { cameras: 0 }
    }

    async fn print_check(
        self,
        _ctx: context::Context,
        opts: CheckOpts,
    ) -> Result<bool, CheckError> {
        tracing::info!("{} requesting print_check", self.socket);

        let stl_path = write_tmp_stl(&opts.stl).await.map_err(|err| {
            tracing::error!("failed to create tmp stl file: {err:#?}");

            CheckError::Stl
        })?;

        let result = run_validation(&stl_path, opts.layer.as_ref()).await?;

        if let Err(err) = tokio::fs::remove_file(&stl_path).await {
            tracing::error!("failed to remove tmp stl file: {err:#?}");
        }

        Ok(result)
    }
}

async fn write_tmp_stl(data: &[u8]) -> anyhow::Result<PathBuf> {
    let path = get_tmp_file()?;

    tokio::fs::write(&path, data)
        .await
        .context("failed to write tmp stl file")?;

    Ok(path)
}

fn get_time() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .context("check clock settings as system time is before UNIX_EPOCH")
}

fn get_tmp_file() -> anyhow::Result<PathBuf> {
    let time = get_time()?;
    let tmp_dir = PathBuf::from("/tmp");
    let mut count = 1;

    loop {
        let tmp_path = tmp_dir.join(format!("{time}_{count}.stl"));

        if !tmp_path.exists() {
            return Ok(tmp_path);
        }

        count += 1;

        if count > 100 {
            bail!("too many tmp file attempts");
        }
    }
}

async fn run_validation<P>(stl_path: P, layer_opts: Option<&LayerOpts>) -> Result<bool, CheckError>
where
    P: AsRef<Path>,
{
    let path_ref = stl_path.as_ref();

    tracing::info!("starting stl-render");

    let stl_render = spawn_stl_render(path_ref, layer_opts)
        .map_err(|err| {
            tracing::error!("failed spawning stl-render: {err:#?}");

            CheckError::StlRender
        })?
        .wait()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving stl-render status: {err:#?}");

            CheckError::StlRender
        })?;

    if !stl_render.success() {
        return Err(CheckError::StlRender);
    }

    tracing::info!("starting validator");

    let validator = spawn_validator(path_ref, layer_opts)
        .map_err(|err| {
            tracing::error!("failed spawning validator: {err:#?}");

            CheckError::Validator
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving validator status: {err:#?}");

            CheckError::Validator
        })?;

    if !validator.status.success() {
        return Err(CheckError::Validator);
    }

    let Ok(utf8_output) = std::str::from_utf8(&validator.stdout) else {
        tracing::error!("invalid utf-8 output from validator");

        return Err(CheckError::Validator);
    };

    if let Ok(value) = u32::from_str(utf8_output.trim()) {
        tracing::info!("finished validation");

        Ok(value != 0)
    } else {
        tracing::error!("invalid u32 from validator output: \"{utf8_output}\"");

        Err(CheckError::Validator)
    }
}

fn spawn_stl_render<P>(
    stl_path: P,
    layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>,
{
    let mut cmd = tokio::process::Command::new("echo");

    cmd.arg(stl_path)
        .arg("--cameras")
        .arg("/node_cameras.json")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    if let Some(opts) = layer_opts {
        let height_str = opts.height.to_string();
        let number_str = opts.number.to_string();

        cmd.arg("--layer-height")
            .arg(&height_str)
            .arg("--layers")
            .arg(&number_str)
            .spawn()
            .context("failed starting stl-render")
    } else {
        cmd.spawn().context("failed starting stl-render")
    }
}

fn spawn_validator<P>(
    _stl_path: P,
    _layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>,
{
    tokio::process::Command::new("echo")
        .arg("0")
        .stdout(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed starting validator")
}
