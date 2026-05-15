//! a set of commands for handling stl-render, geometric, stereopsis processes

use std::{
    ffi::OsStr,
    path::PathBuf,
    process::Stdio,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::Context;
use com::{
    BackgroundResults, CheckError, CompareResults, DimOpts, LayerOpts, StartError,
    StereopsisResults,
};
use serde::Deserialize;
use tokio::process::{Child, Command};
use tracing::instrument;

use crate::cameras::{CameraPosition, KnownCameras};
use crate::config::{ExecConfig, PythonExec};

/// outputs the set of arguments for the given command if the level "trace" is
/// enabled
fn trace_arguments(cmd: &Command) {
    if tracing::enabled!(tracing::Level::TRACE) {
        let args: Vec<&OsStr> = cmd.as_std().get_args().collect();

        tracing::trace!("command arguments: {args:#?}");
    }
}

/// runs the stl-render process and handles its output
#[instrument(level = "trace", skip_all)]
pub async fn run_stl_render<P>(
    exec: &ExecConfig,
    stl_path: P,
    layer_opts: Option<&LayerOpts>,
) -> Result<(), CheckError>
where
    P: AsRef<OsStr>,
{
    let stl_render = spawn_stl_render(&exec.stl_render, &exec.cameras, stl_path, layer_opts)
        .map_err(|err| {
            tracing::error!("failed spawning: {err:#?}");

            CheckError::StlRender
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving status: {err:#?}");

            CheckError::StlRender
        })?;

    if !stl_render.status.success() {
        let stdout = std::str::from_utf8(&stl_render.stdout);
        let stderr = std::str::from_utf8(&stl_render.stderr);

        match (stdout, stderr) {
            (Ok(valid_out), Ok(valid_err)) => {
                tracing::error!("returned non-zero status code\n{valid_out}\n{valid_err}");
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

/// creates the stl-render command
#[instrument(level = "trace", skip_all)]
fn spawn_stl_render<CP, SP>(
    exec: &PythonExec,
    cameras_path: CP,
    stl_path: SP,
    layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<Child>
where
    CP: AsRef<OsStr>,
    SP: AsRef<OsStr>,
{
    tracing::trace!("creating cmd: {} {}", exec.binary, exec.script);

    let mut cmd = Command::new(&exec.binary);

    tracing::trace!("adding static arguments");

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
        tracing::trace!("adding layer height and number");

        let height_str = opts.height.to_string();
        let number_str = opts.number.to_string();

        cmd.arg("--layer-height")
            .arg(&height_str)
            .arg("--layers")
            .arg(&number_str);
    }

    trace_arguments(&cmd);

    cmd.spawn().context("spawn failed")
}

/// runs the geometric validation process and handles its output
#[instrument(level = "trace", skip_all)]
pub async fn run_compare(
    exec: &ExecConfig,
    layer_opts: Option<&LayerOpts>,
) -> Result<CompareResults, CheckError> {
    let start = Instant::now();

    let validator = spawn_compare(&exec.validator, &exec.cameras, layer_opts)
        .map_err(|err| {
            tracing::error!("failed spawning: {err:#?}");

            CheckError::Validator
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving status: {err:#?}");

            CheckError::Validator
        })?;

    let exec_time = start.elapsed();

    let stdout = std::str::from_utf8(&validator.stdout);
    let stderr = std::str::from_utf8(&validator.stderr);

    if let Some(code) = validator.status.code() {
        // code 0 is passed
        // code 6 is failed
        if code == 0 || code == 6 {
            // pull timing information from stdout
            let geo_val_time = match stdout {
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

            return Ok(CompareResults {
                success: code == 0,
                geo_val_time,
                exec_time,
            });
        }
    }

    // test

    let valid_out = stdout.unwrap_or("");
    let valid_err = stderr.unwrap_or("");

    if let Some(code) = validator.status.code() {
        tracing::error!(
            "returned non-zero status code {code}\nstdout: \"{valid_out}\"\nstderr: \"{valid_err}\""
        );
    } else {
        tracing::error!(
            "returned no status code\nstdout: \"{valid_out}\"\nstderr: \"{valid_err}\""
        );
    }

    Err(CheckError::Validator)
}

/// creates the geometric validation command
#[instrument(level = "trace", skip_all)]
fn spawn_compare<P>(
    cmd: &str,
    cameras_path: P,
    layer_opts: Option<&LayerOpts>,
) -> anyhow::Result<Child>
where
    P: AsRef<OsStr>,
{
    tracing::trace!("creating cmd: {cmd}");

    let mut cmd = Command::new(cmd);

    cmd.arg("--live")
        .arg("--cameras")
        .arg(cameras_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(opts) = layer_opts {
        tracing::trace!("adding layer number");

        let number_str = opts.number.to_string();

        cmd.arg("--layer-number").arg(number_str);
    }

    trace_arguments(&cmd);

    cmd.spawn().context("spawn failed")
}

/// json struct created by the stereopsis process
#[derive(Debug, Deserialize)]
pub struct StereopsisJson {
    /// overall result of the stereopsis process
    overall_passed: bool,
}

/// runs the stereopsis validation process and handles its output
#[instrument(level = "trace", skip_all)]
pub async fn run_stereopsis(
    cameras: &KnownCameras,
    exec: &ExecConfig,
    dim: &DimOpts,
) -> Result<Option<StereopsisResults>, CheckError> {
    let Some(exec_args) = &exec.stereopsis else {
        tracing::info!("disabled");

        return Ok(None);
    };

    let json_output = PathBuf::from("/tmp/stereopsis_results.json");

    let start = Instant::now();

    let result = spawn_stereopsis(cameras, exec_args, dim, &json_output)
        .map_err(|err| {
            tracing::error!("failed spawning: {err:#?}");

            CheckError::Stereopsis
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving status: {err:#?}");

            CheckError::Stereopsis
        })?;

    let exec_time = start.elapsed();

    let stdout = std::str::from_utf8(&result.stdout).unwrap_or("");
    let stderr = std::str::from_utf8(&result.stderr).unwrap_or("");

    if !result.status.success() {
        if let Some(code) = result.status.code() {
            tracing::error!(
                "returned non-zero status code {code}\nstdout: \"{stdout}\"\nstderr: \"{stderr}\""
            );
        } else {
            tracing::error!("returned no status code\nstdout: \"{stdout}\"\nstderr: \"{stderr}\"");
        }

        return Err(CheckError::Stereopsis);
    }

    let contents = tokio::fs::read(&json_output).await.map_err(|err| {
        tracing::error!("failed reading json results: {err:#?}");

        CheckError::Stereopsis
    })?;

    let json: StereopsisJson = serde_json::from_slice(&contents).map_err(|err| {
        tracing::error!("failed parsing json: {err:#?}");

        CheckError::Stereopsis
    })?;

    Ok(Some(StereopsisResults {
        success: json.overall_passed,
        exec_time,
    }))
}

/// creates the stereopsis validation command
#[instrument(level = "trace", skip_all)]
fn spawn_stereopsis<P>(
    cameras: &KnownCameras,
    exec: &PythonExec,
    dim: &DimOpts,
    json: P,
) -> anyhow::Result<Child>
where
    P: AsRef<OsStr>,
{
    tracing::trace!("creating cmd: {} {}", exec.binary, exec.script);

    let mut cmd = Command::new(&exec.binary);

    cmd.arg(&exec.script);

    for additional in &exec.args {
        cmd.arg(&additional.flag);

        if let Some(value) = &additional.value {
            cmd.arg(value);
        }
    }

    for (key, info) in cameras {
        // will assume that this has been checked before hand that we will have
        // the left and right cameras specified
        match info.position {
            CameraPosition::Left => {
                tracing::trace!("adding left camera: {key}");

                let original = info.full_frame_output_dir.join("full_frame_original.png");
                let overlay = info
                    .full_frame_output_dir
                    .join("full_frame_fitted_cad_overlay.png");

                cmd.arg("--left-image")
                    .arg(original)
                    .arg("--left-edge-overlay")
                    .arg(overlay);
            }
            CameraPosition::Right => {
                tracing::trace!("adding right camera {key}");

                let original = info.full_frame_output_dir.join("full_frame_original.png");
                let overlay = info
                    .full_frame_output_dir
                    .join("full_frame_fitted_cad_overlay.png");

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
        .stderr(Stdio::piped());

    trace_arguments(&cmd);

    cmd.spawn().context("spawn failed")
}

/// runs the build-background process and handles its output
#[instrument(level = "trace", skip_all)]
pub async fn run_background_builder(exec: &ExecConfig) -> Result<BackgroundResults, StartError> {
    let start = Instant::now();

    let result = spawn_background_builder(&exec.background, &exec.cameras)
        .map_err(|err| {
            tracing::error!("failed spawning background-builder: {err:#?}");

            StartError::Background
        })?
        .wait_with_output()
        .await
        .map_err(|err| {
            tracing::error!("failed retrieving background-builder status: {err:#?}");

            StartError::Background
        })?;

    let exec_time = start.elapsed();

    let stdout = std::str::from_utf8(&result.stdout).unwrap_or("");
    let stderr = std::str::from_utf8(&result.stderr).unwrap_or("");

    if !result.status.success() {
        if let Some(code) = result.status.code() {
            tracing::error!(
                "returned non-zero status code {code}\nstdout: \"{stdout}\"\nstderr: \"{stderr}\""
            );
        } else {
            tracing::error!("returned no status code\nstdout: \"{stdout}\"\nstderr: \"{stderr}\"");
        }

        return Err(StartError::Background);
    }

    Ok(BackgroundResults { exec_time })
}

/// creates the build-background command
#[instrument(level = "trace", skip_all)]
fn spawn_background_builder<P>(exec: &str, cameras_path: P) -> anyhow::Result<tokio::process::Child>
where
    P: AsRef<OsStr>,
{
    let mut cmd = Command::new(exec);

    cmd.arg(cameras_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    trace_arguments(&cmd);

    cmd.spawn().context("spawn failed")
}
