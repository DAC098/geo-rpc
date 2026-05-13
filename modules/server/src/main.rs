use std::{ffi::OsStr, net::SocketAddr, path::PathBuf, sync::Arc, time::SystemTime};

use anyhow::{Context, bail};
use clap::Parser;
use com::{
    AddrArgs, BackgroundResults, CheckError, CheckOpts, CompareResults, DimOpts, Info, LayerOpts,
    Rpc, StartError, StereopsisResults,
};
use futures::StreamExt;
use tarpc::{
    context,
    server::{self, Channel, incoming::Incoming},
    tokio_serde::formats::Json,
};
use tracing::instrument;

mod cameras;
mod commands;
mod config;

#[derive(Debug, Parser)]
struct CliArgs {
    /// config file to use command execution and additional configuration
    ///
    /// if no file is provided then it will attempt to look for `config.toml`
    /// `config.ignore.toml` in the current working directory.
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

    tracing::debug!("config: {config:#?}");

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

        if exec.stereopsis.is_some() {
            // check to make sure that for now we have exactly one left and
            // right camera
            let mut found_left = 0;
            let mut found_right = 0;

            for info in known_cameras.values() {
                match info.position {
                    cameras::CameraPosition::Left => {
                        if info.device.is_some() {
                            found_left += 1;
                        }
                    }
                    cameras::CameraPosition::Right => {
                        if info.device.is_some() {
                            found_right += 1;
                        }
                    }
                }
            }

            if found_left == 0 {
                bail!("no left camera found for stereopsis");
            }

            if found_right == 0 {
                bail!("no right camera found for stereopsis");
            }

            if found_left > 1 || found_right > 1 {
                bail!(
                    "found more than one camera for left or right: left={found_left} right={found_right}"
                );
            }
        }

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
    async fn print_start(self, _ctx: context::Context) -> Result<BackgroundResults, StartError> {
        run_start(&self.state.exec).await
    }

    #[instrument(level="trace", skip_all, fields(peer_addr=%self.peer_addr))]
    async fn print_check(
        self,
        _ctx: context::Context,
        opts: CheckOpts,
    ) -> Result<(CompareResults, Option<StereopsisResults>), CheckError> {
        let stl_path = write_tmp_stl(&opts.stl).await.map_err(|err| {
            tracing::error!("failed to create tmp stl file: {err:#?}");

            CheckError::Stl
        })?;

        let result = run_check(
            &self.state.exec,
            &self.state.known_cameras,
            &stl_path,
            opts.layer.as_ref(),
            &opts.dim,
        )
        .await;

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

async fn run_start(exec: &config::ExecConfig) -> Result<BackgroundResults, StartError> {
    tracing::info!("running background-builder");

    commands::run_background_builder(exec).await
}

async fn run_check<P>(
    exec: &config::ExecConfig,
    cameras: &cameras::KnownCameras,
    stl_path: P,
    layer: Option<&LayerOpts>,
    dim: &DimOpts,
) -> Result<(CompareResults, Option<StereopsisResults>), CheckError>
where
    P: AsRef<OsStr>,
{
    tracing::info!("running stl-render");

    commands::run_stl_render(exec, stl_path, layer).await?;

    tracing::info!("running compare");

    let compare_check = commands::run_compare(exec, layer).await?;

    let stereopsis_check = if compare_check.success {
        tracing::info!("running stereopsis");

        commands::run_stereopsis(cameras, exec, dim).await?
    } else {
        tracing::info!("skipping stereopsis");

        None
    };

    Ok((compare_check, stereopsis_check))
}

async fn run_finish() {}
