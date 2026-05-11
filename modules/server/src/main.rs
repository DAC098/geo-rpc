use std::{
    ffi::OsStr,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, bail};
use clap::Parser;
use com::{AddrArgs, CheckError, CheckOpts, Info, LayerOpts, Rpc, StartError};
use futures::StreamExt;
use tarpc::{
    context,
    server::{self, Channel, incoming::Incoming},
    tokio_serde::formats::Json,
};
use tracing::instrument;

mod cameras;
mod config;

use config::PythonExec;

#[derive(Debug, Parser)]
struct CliArgs {
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(flatten)]
    addr: AddrArgs,
}

#[derive(Clone)]
struct RpcServer {
    peer_addr: SocketAddr,
    state: Arc<ServerState>,
}

#[derive(Debug)]
struct ServerState {
    hostname: String,
    exec: config::ExecConfig,
    known_cameras: cameras::KnownCameras,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    com::init_logging();

    let args = CliArgs::parse();
    let Some(config_path) = config::ServerConfig::get_path(args.config.clone())? else {
        bail!("no server config specified or default found");
    };

    let config = config::ServerConfig::load(&config_path).await?;

    if !config.exec.cameras.exists() {
        bail!("cameras json does not exists");
    }

    let state = ServerState::new(config.exec)?;

    let addr = args
        .addr
        .get_server_addr()
        .or(config.listen)
        .unwrap_or(com::default_server());

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
            let peer_addr = channel.transport().peer_addr().unwrap();
            let server = RpcServer::new(peer_addr, state.clone());

            channel.execute(server.serve()).for_each(async |c| {
                tokio::spawn(c);
            })
        })
        .buffer_unordered(10)
        .for_each(|_| async {})
        .await;

    Ok(())
}

impl ServerState {
    fn new(exec: config::ExecConfig) -> anyhow::Result<Arc<Self>> {
        let hostname = hostname::get()
            .context("failed retrieving server hostname")?
            .to_string_lossy()
            .to_string();
        let known_cameras = cameras::load_known_cameras(&exec.cameras)?;

        tracing::info!("known cameras: {known_cameras:#?}");

        Ok(Arc::new(ServerState {
            hostname,
            exec,
            known_cameras,
        }))
    }
}

impl RpcServer {
    fn new(peer_addr: SocketAddr, state: Arc<ServerState>) -> Self {
        Self { peer_addr, state }
    }
}

impl Rpc for RpcServer {
    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn health(self, _ctx: context::Context) -> String {
        "okay".into()
    }

    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn info(self, _ctx: context::Context) -> Info {
        let cameras = self
            .state
            .known_cameras
            .iter()
            .map(|(name, info)| com::Camera {
                name: name.clone(),
                serial: info.serial.clone(),
                avail: info.device.is_some(),
            })
            .collect();

        Info {
            hostname: self.state.hostname.clone(),
            cameras,
        }
    }

    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn print_start(self, _ctx: context::Context) -> Result<(), StartError> {
        run_start(&self.state.exec).await
    }

    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn print_check(
        self,
        _ctx: context::Context,
        opts: CheckOpts,
    ) -> Result<bool, CheckError> {
        let stl_path = write_tmp_stl(&opts.stl).await.map_err(|err| {
            tracing::error!("failed to create tmp stl file: {err:#?}");

            CheckError::Stl
        })?;

        let result = run_check(&self.state.exec, &stl_path, opts.layer.as_ref()).await;

        if let Err(err) = tokio::fs::remove_file(&stl_path).await {
            tracing::error!("failed to remove tmp stl file: {err:#?}");
        }

        result
    }

    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn print_finish(self, _ctx: context::Context) {
        run_finish().await;
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

async fn run_start(exec: &config::ExecConfig) -> Result<(), StartError> {
    tracing::info!("starting background-builder");

    let status = spawn_background_builder(&exec.background, &exec.cameras)
        .map_err(|err| {
            tracing::error!("failed spawning background-builder: {err:#?}");

            StartError::Background
        })?
        .wait()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving background-builder status: {err:#?}");

            StartError::Background
        })?;

    if !status.success() {
        tracing::error!("background-builder returned non-zero status code");

        return Err(StartError::Background);
    }

    Ok(())
}

fn spawn_background_builder<P>(cmd: &str, cameras_path: P) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>,
{
    tokio::process::Command::new(cmd)
        .arg(cameras_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed running background builder")
}

async fn run_check<P>(
    exec: &config::ExecConfig,
    stl_path: P,
    layer_opts: Option<&LayerOpts>,
) -> Result<bool, CheckError>
where
    P: AsRef<Path>,
{
    let path_ref = stl_path.as_ref();

    tracing::info!("starting stl-render");

    let stl_render = spawn_stl_render(&exec.stl_render, &exec.cameras, path_ref, layer_opts)
        .map_err(|err| {
            tracing::error!("failed spawning stl-render: {err:#?}");

            CheckError::StlRender
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving stl-render status: {err:#?}");

            CheckError::StlRender
        })?;

    if !stl_render.status.success() {
        let stdout = std::str::from_utf8(&stl_render.stdout);
        let stderr = std::str::from_utf8(&stl_render.stderr);

        match (stdout, stderr) {
            (Ok(valid_out), Ok(valid_err)) => {
                tracing::error!(
                    "stl-render returned non-zero status code\n{valid_out}\n{valid_err}"
                );
            }
            _ => {
                tracing::error!("stl-render returned non-zero status code");
            }
        }

        return Err(CheckError::StlRender);
    }

    tracing::info!("starting validator");

    let validator = spawn_validator(&exec.validator, &exec.cameras, layer_opts)
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

    if let Some(code) = validator.status.code() {
        // code 0 is passed
        // code 6 is failed
        if code == 0 || code == 6 {
            return Ok(code == 0);
        }
    }

    let stdout = std::str::from_utf8(&validator.stdout);
    let stderr = std::str::from_utf8(&validator.stderr);

    match (validator.status.code(), stdout, stderr) {
        (Some(code), Ok(valid_out), Ok(valid_err)) => {
            tracing::error!(
                "validator returned non-zero status code {code}\n{valid_out}\n{valid_err}"
            );
        }
        (Some(code), _, _) => tracing::error!("validator returned non-zero status code {code}"),
        _ => tracing::error!("validator returned unsuccessful"),
    }

    Err(CheckError::Validator)
}

fn spawn_stl_render<CP, SP>(
    exec: &PythonExec,
    cameras_path: CP,
    stl_path: SP,
    layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<tokio::process::Child>
where
    CP: AsRef<OsStr>,
    SP: AsRef<OsStr>,
{
    tracing::trace!("creating stl-render cmd: {} {}", exec.binary, exec.script);

    let mut cmd = tokio::process::Command::new(&exec.binary);

    cmd.arg(&exec.script)
        .arg(stl_path)
        .arg("--cameras")
        .arg(cameras_path)
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
    cmd: &str,
    cameras_path: P,
    layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>,
{
    tracing::trace!("creating validator cmd: {cmd}");

    let mut cmd = tokio::process::Command::new(cmd);

    cmd.arg("--live")
        .arg("--cameras")
        .arg(cameras_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(opts) = layer_opts {
        let number_str = opts.number.to_string();

        cmd.arg("--layer-number")
            .arg(number_str)
            .spawn()
            .context("failed starting validator")
    } else {
        cmd.spawn().context("failed starting validator")
    }
}

async fn run_finish() {}
