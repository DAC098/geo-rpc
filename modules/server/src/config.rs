//! provides structs for loading the config file for the server

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Deserialize;

/// the top level server config struct
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// an optional specified socket address to listen on
    pub listen: Option<SocketAddr>,
    /// config information for running necessary processes for
    /// print validation
    pub exec: ExecConfig,
}

/// provides information for executing the necessary process for print
/// validation
#[derive(Debug, Deserialize)]
pub struct ExecConfig {
    /// the path for the json containing camera config information
    pub cameras: PathBuf,
    /// the executable to use for build-background
    pub background: String,
    /// the python script to use for stl-rendering
    pub stl_render: PythonExec,
    /// the executable to use for geometric validation
    pub validator: String,
    /// the optional python script to use for stl-rendering
    pub stereopsis: Option<PythonExec>,
}

#[derive(Debug, Deserialize)]
pub struct PythonExec {
    /// the python binary to for executing the script
    pub binary: String,
    /// the python script to use
    pub script: String,
    /// any addtional static argumets to apply
    pub args: Vec<StaticArg>,
}

#[derive(Debug, Deserialize)]
pub struct StaticArg {
    /// the argument to add
    pub flag: String,
    /// the optiona value for the argument
    pub value: Option<String>,
}

impl ServerConfig {
    /// the config file to load if specified
    ///
    /// will default to the current working directories `config.toml` or
    /// `config.ignore.toml` if they exist
    pub fn get_path(path: Option<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
        if let Some(path) = path {
            Ok(Some(path))
        } else {
            let cwd =
                std::env::current_dir().context("failed retrieving current working directory")?;

            let default_path = cwd.join("config.toml");
            let default_ignore_path = cwd.join("config.ignore.toml");

            if default_path.exists() {
                Ok(Some(default_path))
            } else if default_ignore_path.exists() {
                Ok(Some(default_ignore_path))
            } else {
                Ok(None)
            }
        }
    }

    /// attempts to load the specified toml file
    pub async fn load<P>(path: P) -> anyhow::Result<Self>
    where
        P: AsRef<Path>,
    {
        let path_ref = path.as_ref();

        tracing::debug!("loading config file: {}", path_ref.display());

        let contents = tokio::fs::read_to_string(path_ref).await.with_context(|| {
            format!("failed reading server config file: {}", path_ref.display())
        })?;

        toml::from_str(&contents).with_context(|| {
            format!(
                "failed parsing server config as toml: {}",
                path_ref.display()
            )
        })
    }
}
