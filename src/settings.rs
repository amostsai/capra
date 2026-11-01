use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum VideoBackend {
    #[default]
    Auto,
    WaylandPipeWire,
    X11,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub record_system: bool,
    pub record_mic: bool,
    pub video_backend: VideoBackend,
    pub save_directory: String,
    pub system_volume: f32,
    pub mic_volume: f32,
    pub target_fps: u32,
    pub video_bitrate: u32,
    pub global_shortcut_key: String,
    pub auto_play: bool,
    pub auto_open_folder: bool,
    pub keep_mkv: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            record_system: true,
            record_mic: true,
            video_backend: VideoBackend::Auto,
            save_directory: std::env::var("HOME")
                .map(|h| format!("{h}/Videos"))
                .unwrap_or_else(|_| ".".into()),
            system_volume: 0.6,
            mic_volume: 1.5,
            target_fps: 30,
            video_bitrate: 4500,
            global_shortcut_key: "F9".into(),
            auto_play: false,
            auto_open_folder: false,
            keep_mkv: true,
        }
    }
}
pub fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("capra/settings.json")
}
pub fn load(path: &Path) -> Result<Settings, String> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(format!("讀取設定失敗: {e}")),
    };
    let settings: Settings =
        serde_json::from_slice(&data).map_err(|e| format!("設定檔無法讀取: {e}"))?;
    if !matches!(settings.target_fps, 30 | 60)
        || !matches!(settings.video_bitrate, 2500 | 4500 | 8000)
        || !settings.system_volume.is_finite()
        || !(0.1..=1.5).contains(&settings.system_volume)
        || !settings.mic_volume.is_finite()
        || !(0.5..=3.0).contains(&settings.mic_volume)
    {
        return Err("設定檔含有無效的畫質或音量".into());
    }
    Ok(settings)
}
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("無效的設定檔位置")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let data = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| format!("儲存設定失敗: {e}"))?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_and_recording_preferences_survive_restart() {
        let dir = std::env::temp_dir().join(format!("recorder-settings-{}", std::process::id()));
        let path = dir.join("nested/settings.json");
        let prefs = Settings {
            keep_mkv: false,
            target_fps: 60,
            save_directory: "/tmp/中文 影片".into(),
            ..Settings::default()
        };
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap(), prefs);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn invalid_existing_config_is_reported_instead_of_silently_reset() {
        let p =
            std::env::temp_dir().join(format!("recorder-bad-settings-{}.json", std::process::id()));
        std::fs::write(&p, "broken").unwrap();
        assert!(load(&p).is_err());
        let _ = std::fs::remove_file(p);
    }
}
