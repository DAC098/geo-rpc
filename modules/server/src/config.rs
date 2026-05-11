use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen: Option<SocketAddr>,
    pub exec: ExecConfig,
}

#[derive(Debug, Deserialize)]
pub struct ExecConfig {
    pub cameras: PathBuf,
    pub background: String,
    pub stl_render: PythonExec,
    pub validator: String,
}

#[derive(Debug, Deserialize)]
pub struct PythonExec {
    pub binary: String,
    pub script: String,
}

impl ServerConfig {
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
