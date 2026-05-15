//! provides common options, errors, and results to be used between a server
//! and client for the RPC communication.

use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use clap::Args;
use serde::{Deserialize, Serialize};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

/// default server port when listening for RPC requests
pub const DEFAULT_PORT: u16 = 6789;

/// options for checking a print at a certain layer and layer height
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerOpts {
    /// the height of a single layer (value in millimeters)
    pub height: f32,
    /// the layer to check at
    pub number: u32,
}

/// options for specifying the expected width and height of a print
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimOpts {
    /// the expected width of the part (value in millimeters)
    pub width: f32,
    /// the expected height of the part (value in millimeters)
    pub height: f32,
}

/// options to be sent to the server
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckOpts {
    /// the optional layer at which to check the part
    pub layer: Option<LayerOpts>,
    /// the expected overall dimensions of the part
    pub dim: DimOpts,
    /// the STL file associated wit hthe current print
    pub stl: Vec<u8>,
}

/// the potential errors that can happen when requesting the `print-start`
/// commmand
#[derive(Debug, Serialize, Deserialize)]
pub enum StartError {
    /// an error occured with sending the STL file to the server
    Stl,
    /// an error occured when performing the build-background process
    Background,
}

/// the potential errors that can happen when requesting the `print-check`
/// command
#[derive(Debug, Serialize, Deserialize)]
pub enum CheckError {
    /// an error occured with sending the STL file to the server
    Stl,
    /// an error occured when performing the stl-render process
    StlRender,
    /// an error occured when performing the geometeric valdiation process
    Validator,
    /// an error occured when performing the stereopsis process
    Stereopsis,
}

/// general information returned from a server
#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    /// hostname of the remote server
    pub hostname: String,
    /// the cameras attached to the remote server
    pub cameras: Vec<Camera>,
}

/// information pertaining to the known cameras of the remote server
#[derive(Debug, Serialize, Deserialize)]
pub struct Camera {
    /// the name of the camera specified by the camera config file
    pub name: String,
    /// the serial number of the attached camera
    pub serial: String,
    /// indicates if the camera is available to the system and specified in the
    /// camera config (serial number is specified in the config but may not be
    /// available to the system)
    pub avail: bool,
}

/// statistics and results for the build-background process
#[derive(Debug, Serialize, Deserialize)]
pub struct BackgroundResults {
    /// the overall execution time of the process as captured by the server
    pub exec_time: Duration,
}

/// statistics and results for the geometric validation process
#[derive(Debug, Serialize, Deserialize)]
pub struct CompareResults {
    /// indicates if the process found the print to be valid based on the
    /// provided check information
    pub success: bool,
    /// the reported execution for processing the camera inputs against the
    /// expected results
    pub geo_val_time: Duration,
    /// the overall execution time of the process as captured by the server
    pub exec_time: Duration,
}

/// statistics and results for the stereopsis process
#[derive(Debug, Serialize, Deserialize)]
pub struct StereopsisResults {
    /// indicates if the process found the print to be valid based on the
    /// provided check information
    pub success: bool,
    /// the overall execution time of the process as captured by the server
    pub exec_time: Duration,
}

#[tarpc::service]
pub trait Rpc {
    /// returns information for the health of the server / device
    async fn health() -> String;

    /// returns information regarding information about the server / device
    async fn info() -> Info;

    /// returns the results of running the build-background process
    async fn print_start() -> Result<BackgroundResults, StartError>;

    /// returns the results of running the geometric and stereopsis validation
    async fn print_check(
        opts: CheckOpts,
    ) -> Result<(CompareResults, Option<StereopsisResults>), CheckError>;

    /// runs any cleanup work for when a print has finished
    async fn print_finish();
}

#[derive(Debug, Args)]
pub struct AddrArgs {
    /// a single socket address
    ///
    /// provide an ip and port in the format of 127.0.0.1:1234 or [::1]:1234
    #[arg(short, long, conflicts_with_all = ["host", "port"])]
    addr: Option<SocketAddr>,

    /// a single ip address
    ///
    /// can be Ipv4 or Ipv6, will use the default server port
    #[arg(long, conflicts_with = "addr")]
    host: Option<IpAddr>,

    /// a singe port number
    ///
    /// will use the default localhost ip address
    #[arg(short, long, conflicts_with = "addr")]
    port: Option<u16>,
}

/// returns the default ip and socket for a server to listen on
pub fn default_server() -> SocketAddr {
    (IpAddr::V6(Ipv6Addr::UNSPECIFIED), DEFAULT_PORT).into()
}

impl AddrArgs {
    /// returns the socket address for a client to use when connecting to a
    /// server
    pub fn get_client_addr(&self) -> SocketAddr {
        match (self.addr, self.host, self.port) {
            (Some(addr), _, _) => addr,
            (None, Some(host), Some(port)) => (host, port).into(),
            (None, Some(host), None) => (host, DEFAULT_PORT).into(),
            (None, None, Some(port)) => (IpAddr::V6(Ipv6Addr::LOCALHOST), port).into(),
            (None, None, None) => (IpAddr::V6(Ipv6Addr::LOCALHOST), DEFAULT_PORT).into(),
        }
    }

    /// optionally returns the socket address for a server to listen on for
    /// new connections
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

/// initializes the logging and tracing system for a process
pub fn init_logging() {
    let filter = EnvFilter::builder()
        // set the default log level to ERROR so that if no RUST_LOG is
        // specified then it will always output `tracing::error!` logs
        .with_default_directive(LevelFilter::ERROR.into())
        // try to pull from the env for any additional configuration
        .from_env_lossy();

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
