//! Settings persisted as TOML in %APPDATA%\FreeCamPhone\settings.toml.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum Backend {
    /// Media Foundation on Windows 11, DirectShow otherwise.
    #[default]
    Auto,
    MediaFoundation,
    DirectShow,
    Both,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Empty = auto-detect.
    pub adb_path: String,
    pub backend: Backend,
    pub mirror: bool,
    pub start_with_windows: bool,
    /// 480 / 720 / 1080.
    pub resolution: u16,
    pub fps: u8,
    /// Render device for phone audio (e.g. "CABLE Input"); empty = off.
    pub audio_device: String,
    /// "usb:<serial>" / "wifi:<name>"; empty = automatic (USB preferred).
    pub device: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            adb_path: String::new(),
            backend: Backend::Auto,
            mirror: false,
            start_with_windows: false,
            resolution: 720,
            fps: 30,
            audio_device: "CABLE Input".into(),
            device: String::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("FreeCamPhone")
            .join("settings.toml"),
    )
}

impl Settings {
    pub fn load() -> Settings {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_and_defaults() {
        let s = Settings {
            mirror: true,
            backend: Backend::Both,
            ..Default::default()
        };
        let back: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert!(back.mirror);
        assert_eq!(back.backend, Backend::Both);
        let partial: Settings = toml::from_str("fps = 60").unwrap();
        assert_eq!(partial.fps, 60);
        assert_eq!(partial.resolution, 720);
    }
}
