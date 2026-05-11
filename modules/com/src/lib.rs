use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use clap::Args;
use serde::{Deserialize, Serialize};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

pub const DEFAULT_PORT: u16 = 6789;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerOpts {
    pub height: f32,
    pub number: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckOpts {
    pub layer: Option<LayerOpts>,
    pub stl: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum StartError {
    Stl,
    Background,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CheckError {
    Stl,
    StlRender,
    Validator,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    pub hostname: String,
    pub cameras: Vec<Camera>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Camera {
    pub name: String,
    pub serial: String,
    pub avail: bool,
}

#[tarpc::service]
pub trait Rpc {
    async fn health() -> String;

    async fn info() -> Info;

    async fn print_start() -> Result<(), StartError>;

    async fn print_check(opts: CheckOpts) -> Result<bool, CheckError>;

    async fn print_finish();
}

#[derive(Debug, Args)]
pub struct AddrArgs {
    #[arg(short, long, conflicts_with_all = ["host", "port"])]
    addr: Option<SocketAddr>,

    #[arg(long, conflicts_with = "addr")]
    host: Option<IpAddr>,

    #[arg(short, long, conflicts_with = "addr")]
    port: Option<u16>,
}

pub fn default_server() -> SocketAddr {
    (IpAddr::V6(Ipv6Addr::UNSPECIFIED), DEFAULT_PORT).into()
}

impl AddrArgs {
    pub fn get_client_addr(&self) -> SocketAddr {
        match (self.addr, self.host, self.port) {
            (Some(addr), _, _) => addr,
            (None, Some(host), Some(port)) => (host, port).into(),
            (None, Some(host), None) => (host, DEFAULT_PORT).into(),
            (None, None, Some(port)) => (IpAddr::V6(Ipv6Addr::LOCALHOST), port).into(),
            (None, None, None) => (IpAddr::V6(Ipv6Addr::LOCALHOST), DEFAULT_PORT).into(),
        }
    }

    pub fn get_server_addr(&self) -> Option<SocketAddr> {
        match (self.addr, self.host, self.port) {
            (Some(addr), _, _) => Some(addr),
            (None, Some(host), Some(port)) => Some((host, port).into()),
            (None, Some(host), None) => Some((host, DEFAULT_PORT).into()),
            (None, None, Some(port)) => Some((IpAddr::V6(Ipv6Addr::UNSPECIFIED), port).into()),
            (None, None, None) => None,
        }
    }
}

pub fn init_logging() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::ERROR.into())
        .from_env_lossy();

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
