//! provides a single utilities for managing rpc clients

use std::{
    net::{IpAddr, SocketAddr},
    path::Path,
    str::FromStr,
};

use anyhow::{Context, bail};
use com::{DEFAULT_PORT, RpcClient};
use tarpc::{client, context, tokio_serde::formats::Json};

/// a wrapper struct for an RPC client
pub struct Client {
    /// the address for the remote RPC server
    pub addr: SocketAddr,
    /// the prefetched remote information for the RPC server
    pub info: Option<com::Info>,
    /// the actual client for the RPC server
    pub channel: RpcClient,
}

/// parses a node file that contains a list of RPC servers
async fn parse_file<P>(path: P) -> anyhow::Result<Vec<SocketAddr>>
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

impl Client {
    /// creates a list of clients for the provided nodes file
    pub async fn load_file<P>(path: P) -> anyhow::Result<Vec<Self>>
    where
        P: AsRef<Path>,
    {
        let nodes = parse_file(path).await?;

        Self::load_many(nodes).await
    }

    /// creates a list of clients from the provided list of socket addrs
    pub async fn load_many(addrs: Vec<SocketAddr>) -> anyhow::Result<Vec<Self>> {
        let mut rtn = Vec::with_capacity(addrs.len());

        for addr in addrs {
            rtn.push(Self::load(addr).await?);
        }

        Ok(rtn)
    }

    /// creates a single client from the provided socket addr
    pub async fn load(addr: SocketAddr) -> anyhow::Result<Self> {
        let mut transport = tarpc::serde_transport::tcp::connect(addr, Json::default);
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

        Ok(Client {
            addr,
            info,
            channel,
        })
    }

    /// retrieves the name of the RPC server
    pub fn get_name(&self) -> String {
        if let Some(info) = &self.info {
            format!("{}[{}]", info.hostname, self.addr)
        } else {
            format!("{}", self.addr)
        }
    }
}
