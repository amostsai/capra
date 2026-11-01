use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
pub struct ConversionResult {
    pub mp4: PathBuf,
    pub mkv_retained: bool,
    pub warning: Option<String>,
}
struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn probe(path: &Path, count: bool) -> Result<Value, String> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error"]);
    if count {
        cmd.arg("-count_packets");
    }
    let result = cmd
        .args([
            "-show_entries",
            "stream=codec_type,nb_read_packets:format=duration,format_name",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("無法執行 ffprobe，請安裝 FFmpeg: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "無法讀取影片 {}: {}",
            path.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    serde_json::from_slice(&result.stdout).map_err(|e| format!("無法讀取影片資訊: {e}"))
}
fn has_video(info: &Value) -> bool {
    info["streams"]
        .as_array()
        .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "video"))
}
fn duration(info: &Value) -> Option<f64> {
    info["format"]["duration"]
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
}

/// Publish a verified MP4 without overwriting anything; only then apply retention.
pub fn convert_mkv(
    source: &Path,
    keep_mkv: bool,
    mut progress: impl FnMut(f32),
) -> Result<ConversionResult, String> {
    if !source
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("mkv"))
    {
        return Err("請選擇 MKV 檔案".into());
    }
    let source_info = probe(source, false)?;
    if !has_video(&source_info)
        || !source_info["format"]["format_name"]
            .as_str()
            .unwrap_or("")
            .contains("matroska")
    {
        return Err("來源不是含有畫面的 MKV 錄影".into());
    }
    let source_streams = source_info["streams"]
        .as_array()
        .ok_or("來源缺少影音軌道")?;
    if source_streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .count()
        != 1
        || source_streams
            .iter()
            .any(|s| s["codec_type"] != "video" && s["codec_type"] != "audio")
    {
        return Err("此 MKV 含有額外影像、字幕或附件軌道，目前不支援完整轉換；來源已保留".into());
    }
    let folder = parent(source);
    let size = source.metadata().map_err(|e| e.to_string())?.len();
    let available = fs2::available_space(folder).map_err(|e| format!("無法讀取磁碟空間: {e}"))?;
    if available < size.saturating_add(64 * 1024 * 1024) {
        return Err("磁碟空間不足以產生 MP4，MKV 已保留；請清出空間後重試".into());
    }
    let stem = source.file_stem().ok_or("無效檔名")?.to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temp = TemporaryFile(folder.join(format!(
        ".{stem}.converting-{}-{nonce}.mp4",
        std::process::id()
    )));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp.0)
        .map_err(|e| format!("無法建立暫存 MP4: {e}"))?;
    let log = TemporaryFile(temp.0.with_extension("log"));
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&log.0)
        .map_err(|e| format!("無法建立轉換紀錄: {e}"))?;
    let mut child = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-y",
            "-i",
        ])
        .arg(source)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "0:a?",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
        ])
        .arg(&temp.0)
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("無法執行 ffmpeg，請安裝 FFmpeg；MKV 已保留: {e}"))?;
    progress(0.0);
    let total = duration(&source_info);
    let reader = BufReader::new(child.stdout.take().ok_or("無法讀取轉換進度")?);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if let (Some(us), Some(total)) = (
            line.strip_prefix("out_time_us=")
                .and_then(|s| s.parse::<f64>().ok()),
            total,
        ) {
            progress((us / 1_000_000.0 / total).clamp(0.0, 0.99) as f32);
        }
        if total.is_none() {
            if let Some(bytes) = line
                .strip_prefix("total_size=")
                .and_then(|s| s.parse::<f64>().ok())
            {
                progress((bytes / (size.max(1) as f64)).clamp(0.0, 0.99) as f32);
            }
        }
    }
    let status = child.wait().map_err(|e| format!("等待轉換失敗: {e}"))?;
    if !status.success() {
        let details = fs::read_to_string(&log.0).unwrap_or_default();
        let tail: String = details
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!("MP4 轉換失敗，MKV 已保留: {tail}"));
    }
    let output_info = probe(&temp.0, true)?;
    let streams = output_info["streams"]
        .as_array()
        .ok_or("轉換結果缺少影音軌道，MKV 已保留")?;
    let source_audio = source_info["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["codec_type"] == "audio")
        .count();
    let output_audio = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .count();
    if !has_video(&output_info)
        || duration(&output_info).is_none()
        || source_audio != output_audio
        || streams.iter().any(|s| {
            s["nb_read_packets"]
                .as_str()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
                == 0
        })
    {
        return Err("轉換結果沒有完整的影音資料，MKV 已保留".into());
    }
    File::open(&temp.0)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("寫入 MP4 失敗，MKV 已保留: {e}"))?;
    let mut index = 0_u32;
    let output = loop {
        let target = if index == 0 {
            source.with_extension("mp4")
        } else {
            folder.join(format!("{stem}_converted_{index}.mp4"))
        };
        match fs::hard_link(&temp.0, &target) {
            Ok(()) => break target,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                index = index.checked_add(1).ok_or("同名輸出過多")?;
            }
            Err(e) => return Err(format!("發布 MP4 失敗，MKV 已保留: {e}")),
        }
    };
    File::open(folder)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("MP4 已產生但無法確認目錄寫入，MKV 已保留: {e}"))?;
    let mut warning = None;
    let mut mkv_retained = true;
    if !keep_mkv {
        match fs::remove_file(source) {
            Ok(()) => {
                mkv_retained = false;
            }
            Err(e) => {
                warning = Some(format!("MP4 已完成，但無法刪除 MKV: {e}"));
            }
        }
    }
    progress(1.0);
    Ok(ConversionResult {
        mp4: output,
        mkv_retained,
        warning,
    })
}

pub fn recoverable_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("無法掃描錄影資料夾: {e}")),
    };
    let mut pending = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("recording_"))
            && path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("mkv"))
            && entry.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
            && !path.with_extension("mp4").exists()
        {
            pending.push(path);
        }
    }
    pending.sort();
    Ok(pending)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "recorder-media-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn video(&self) -> PathBuf {
            let p = self.0.join("recording_測試 with spaces.mkv");
            assert!(Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=160x120:rate=30",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=sample_rate=48000",
                    "-t",
                    "2",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-g",
                    "15",
                    "-c:a",
                    "aac",
                    "-ac",
                    "2"
                ])
                .arg(&p)
                .status()
                .unwrap()
                .success());
            p
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn successful_conversion_keeps_mkv_when_requested() {
        let f = Fixture::new();
        let src = f.video();
        let mut progress = vec![];
        let r = convert_mkv(&src, true, |p| progress.push(p)).unwrap();
        assert!(src.exists());
        assert!(r.mp4.exists());
        assert!(r.mkv_retained);
        assert_eq!(progress.last(), Some(&1.0));
    }
    #[test]
    fn deletes_mkv_only_after_successful_mp4_publication() {
        let f = Fixture::new();
        let src = f.video();
        let r = convert_mkv(&src, false, |_| {}).unwrap();
        assert!(!src.exists());
        assert!(r.mp4.exists());
        assert!(!r.mkv_retained);
    }
    #[test]
    fn failed_conversion_preserves_source_and_existing_mp4() {
        let f = Fixture::new();
        let src = f.0.join("recording_broken.mkv");
        fs::write(&src, b"truncated header").unwrap();
        let dst = src.with_extension("mp4");
        fs::write(&dst, b"existing video").unwrap();
        assert!(convert_mkv(&src, false, |_| {}).is_err());
        assert_eq!(fs::read(&src).unwrap(), b"truncated header");
        assert_eq!(fs::read(&dst).unwrap(), b"existing video");
    }
    #[test]
    fn existing_mp4_is_never_overwritten() {
        let f = Fixture::new();
        let src = f.video();
        let dst = src.with_extension("mp4");
        fs::write(&dst, b"keep me").unwrap();
        let r = convert_mkv(&src, true, |_| {}).unwrap();
        assert_ne!(r.mp4, dst);
        assert_eq!(fs::read(dst).unwrap(), b"keep me");
    }
    #[test]
    fn detects_unfinished_recordings_but_not_retained_completed_ones() {
        let f = Fixture::new();
        let pending = f.0.join("recording_pending.mkv");
        fs::write(&pending, b"data").unwrap();
        fs::write(f.0.join("recording_done.mkv"), b"data").unwrap();
        fs::write(f.0.join("recording_done.mp4"), b"done").unwrap();
        fs::write(f.0.join("recording_empty.mkv"), b"").unwrap();
        fs::write(f.0.join("other.mkv"), b"data").unwrap();
        assert_eq!(recoverable_files(&f.0).unwrap(), vec![pending]);
    }
    #[test]
    fn truncated_mkv_can_be_remuxed_without_deleting_source() {
        let f = Fixture::new();
        let src = f.video();
        let file = fs::OpenOptions::new().write(true).open(&src).unwrap();
        let size = file.metadata().unwrap().len();
        file.set_len(size * 3 / 4).unwrap();
        drop(file);
        let r = convert_mkv(&src, true, |_| {}).unwrap();
        assert!(r.mp4.exists());
        assert!(src.exists());
    }
    #[test]
    fn unsupported_extra_tracks_never_allow_source_deletion() {
        let f = Fixture::new();
        let original = f.video();
        let src = f.0.join("recording_multiple.mkv");
        assert!(Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(original)
            .args(["-map", "0:v:0", "-map", "0:v:0", "-map", "0:a:0", "-c", "copy"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        assert!(convert_mkv(&src, false, |_| {}).is_err());
        assert!(src.exists());
    }
    #[test]
    fn streamable_mkv_reports_progress_without_duration_metadata() {
        let f = Fixture::new();
        let original = f.video();
        let src = f.0.join("recording_live.mkv");
        assert!(Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(original)
            .args(["-c", "copy", "-live", "1"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        assert!(duration(&probe(&src, false).unwrap()).is_none());
        let mut progress = vec![];
        convert_mkv(&src, true, |p| progress.push(p)).unwrap();
        assert!(progress.iter().any(|p| *p > 0.0 && *p < 1.0));
    }
}
