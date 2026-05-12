use std::{
    ffi::OsStr,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Context, bail};
use clap::Parser;
use com::{AddrArgs, CheckError, CheckOpts, Info, LayerOpts, DimOpts, Rpc, StartError};
use futures::StreamExt;
use serde::Deserialize;
use tarpc::{
    context,
    server::{self, Channel, incoming::Incoming},
    tokio_serde::formats::Json,
};
use tracing::instrument;

use crate::config::{PythonExec, ExecConfig};
use crate::cameras::{KnownCameras, CameraPosition};

pub async fn run_stl_render<P>(
    exec: &ExecConfig,
    stl_path: P,
    layer_opts: Option<&LayerOpts>
) -> Result<(), CheckError>
where
    P: AsRef<OsStr>,
{
    let stl_render = spawn_stl_render(&exec.stl_render, &exec.cameras, stl_path, layer_opts)
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

        Err(CheckError::StlRender)
    } else {
        Ok(())
    }
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

    for additional in &exec.args {
        cmd.arg(&additional.flag);

        if let Some(value) = &additional.value {
            cmd.arg(value);
        }
    }

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

pub async fn run_compare(
    exec: &ExecConfig,
    layer_opts: Option<&LayerOpts>,
) -> Result<(bool, Duration), CheckError> {
    let validator = spawn_compare(&exec.validator, &exec.cameras, layer_opts)
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

    let stdout = std::str::from_utf8(&validator.stdout);
    let stderr = std::str::from_utf8(&validator.stderr);

    if let Some(code) = validator.status.code() {
        // code 0 is passed
        // code 6 is failed
        if code == 0 || code == 6 {
            // pull timing information from stdout
            let duration = match stdout {
                Ok(utf8) => {
                    let lines = utf8.split("\n").filter(|v| !v.is_empty());

                    match lines.last().map(f64::from_str) {
                        Some(Ok(parsed)) => Duration::from_secs_f64(parsed / 1000.0),
                        Some(Err(_)) => {
                            tracing::warn!("failed parsing timing of validator output");

                            Duration::new(0, 0)
                        }
                        None => {
                            tracing::trace!("no output from validator");

                            Duration::new(0, 0)
                        }
                    }
                }
                Err(_) => Duration::new(0, 0),
            };

            return Ok((code == 0, duration));
        }
    }

    // test

    let valid_out = stdout.unwrap_or("");
    let valid_err = stderr.unwrap_or("");

    if let Some(code) = validator.status.code() {
        tracing::error!(
            "validator returned non-zero status code {code}\nstdout: \"{valid_out}\"\nstderr: \"{valid_err}\""
        );
    } else {
        tracing::error!(
            "validator returned no status code\nstdout: \"{valid_out}\"\nstderr: \"{valid_err}\""
        );
    }

    Err(CheckError::Validator)
}

fn spawn_compare<P>(
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

#[derive(Debug, Deserialize)]
pub struct StereopsisJson {
    overall_passed: bool,
}

pub async fn run_stereopsis(
    cameras: &KnownCameras,
    exec: &ExecConfig,
    dim: &DimOpts,
) -> Result<Option<bool>, CheckError> {
    let Some(exec_args) = &exec.stereopsis else {
        return Ok(None);
    };

    let json_output = PathBuf::from("/tmp/stereopsis_results.json");

    let result = spawn_stereopsis(cameras, exec_args, dim, &json_output)
        .map_err(|err| {
            tracing::error!("failed spawning stereopsis: {err:#?}");

            CheckError::Stereopsis
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving stereopsis status: {err:#?}");

            CheckError::Stereopsis
        })?;

    let stdout = std::str::from_utf8(&result.stdout).unwrap_or("");
    let stderr = std::str::from_utf8(&result.stderr).unwrap_or("");

    if !result.status.success() {
        if let Some(code) = result.status.code() {
            tracing::error!(
                "stereopsis returned non-zero status code {code}\nstdout: \"{stdout}\"\nstderr: \"{stderr}\""
            );
        } else {
            tracing::error!(
                "stereopsis returned no status code\nstdout: \"{stdout}\"\nstderr: \"{stderr}\""
            );
        }

        return Err(CheckError::Stereopsis);
    }

    let contents = tokio::fs::read(&json_output)
        .await
        .map_err(|err| {
            tracing::error!("failed reading stereopsis json results: {err:#?}");

            CheckError::Stereopsis
        })?;

    let json: StereopsisJson = serde_json::from_slice(&contents).map_err(|err| {
        tracing::error!("failed parsing stereopsis json: {err:#?}");

        CheckError::Stereopsis
    })?;

    Ok(Some(json.overall_passed))
}

fn spawn_stereopsis<P>(
    cameras: &KnownCameras,
    exec: &PythonExec,
    dim: &DimOpts,
    json: P,
) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>
{
    tracing::trace!("creating stereopsis cmd: {} {}", exec.binary, exec.script);

    let mut cmd = tokio::process::Command::new(&exec.binary);

    cmd.arg(&exec.script);

    for additional in &exec.args {
        cmd.arg(&additional.flag);

        if let Some(value) = &additional.value {
            cmd.arg(value);
        }
    }

    for (key, info) in cameras {
        match info.position {
            CameraPosition::Left => {
                let original = info.full_frame_output_dir.join("full_frame_original.png");
                let overlay = info.full_frame_output_dir.join("full_frame_fitted_cad_overlay.png");

                cmd.arg("--left-image")
                    .arg(original)
                    .arg("--left-edge-overlay")
                    .arg(overlay);
            }
            CameraPosition::Right => {
                let original = info.full_frame_output_dir.join("full_frame_original.png");
                let overlay = info.full_frame_output_dir.join("full_frame_fitted_cad_overlay.png");

                cmd.arg("--right-image")
                    .arg(original)
                    .arg("--right-edge-overlay")
                    .arg(overlay);
            }
        }
    }

    let width_str = dim.width.to_string();
    let height_str = dim.height.to_string();

    cmd.arg("--expected-width")
        .arg(&width_str)
        .arg("--expected-height")
        .arg(&height_str)
        .arg("--json-output")
        .arg(json)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed starting stereopsis")
}
