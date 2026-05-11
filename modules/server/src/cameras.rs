use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use anyhow::Context;
use serde::Deserialize;
use tracing::instrument;

#[derive(Debug, Deserialize)]
pub struct CameraConfig {
    pub serial: String,

    #[serde(flatten)]
    pub keys: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct CaptureDevice {
    pub path: PathBuf,
    pub video_id: i32,
    pub serial: String,
}

#[derive(Debug)]
pub struct CameraInfo {
    pub serial: String,

    pub device: Option<CaptureDevice>,

    pub config: HashMap<String, serde_json::Value>,
}

pub type KnownCameras = HashMap<String, CameraInfo>;

pub fn find_capture_devices() -> anyhow::Result<Vec<CaptureDevice>> {
    let mut rtn = Vec::new();
    let dir_contents =
        std::fs::read_dir("/dev").context("failed reading directory contents for /dev")?;

    for entry in dir_contents {
        let entry = entry.context("failed reading directory entry")?;
        let path = entry.path();

        let maybe_video = path
            .file_name()
            .and_then(OsStr::to_str)
            .map(|v| v.strip_prefix("video"))
            .flatten();

        let Some(str_video_id) = maybe_video else {
            continue;
        };

        let Ok(video_id) = i32::from_str(str_video_id) else {
            continue;
        };

        if let Some(serial) = check_if_video_capture(&path)? {
            rtn.push(CaptureDevice {
                path,
                video_id,
                serial,
            });
        }
    }

    Ok(rtn)
}

#[instrument]
pub fn check_if_video_capture<P>(path: P) -> anyhow::Result<Option<String>>
where
    P: AsRef<OsStr> + std::fmt::Debug,
{
    let output = Command::new("udevadm")
        .arg("info")
        .arg("--query=all")
        .arg(path)
        .output()
        .context("failed reading udevadm for device path")?;

    if !output.status.success() {
        tracing::warn!("failed retrieving udevadm info for device path");

        return Ok(None);
    }

    let Ok(output) = std::str::from_utf8(&output.stdout) else {
        tracing::warn!("non-utf8 output");

        return Ok(None);
    };

    let mut serial: Option<String> = None;

    for (_index, line) in output.split("\n").enumerate() {
        let Some((t, data)) = line.split_once(": ") else {
            continue;
        };

        if t != "E" {
            continue;
        }

        let Some((key, value)) = data.split_once("=") else {
            continue;
        };

        match key {
            "ID_V4L_CAPABILITIES" => {
                if !value.contains(":capture") {
                    // not a capture device
                    return Ok(None);
                }
            }
            "ID_SERIAL_SHORT" => {
                serial = Some(value.to_owned());
            }
            _ => {}
        }
    }

    Ok(serial)
}

pub fn load_cameras_json<P>(path: P) -> anyhow::Result<HashMap<String, CameraConfig>>
where
    P: AsRef<Path>,
{
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .context("failed opening cameras json file")?;
    let buf = std::io::BufReader::new(file);

    serde_json::from_reader(buf).context("failed parsing cameras json")
}

pub fn load_known_cameras<P>(path: P) -> anyhow::Result<KnownCameras>
where
    P: AsRef<Path>,
{
    let mut rtn = HashMap::new();
    let known_devices: HashMap<String, CaptureDevice> = find_capture_devices()?
        .into_iter()
        .map(|v| (v.serial.clone(), v))
        .collect();

    for (name, config) in load_cameras_json(path)? {
        let device = known_devices.get(&config.serial).cloned();

        rtn.insert(
            name,
            CameraInfo {
                serial: config.serial,
                device,
                config: config.keys,
            },
        );
    }

    Ok(rtn)
}
