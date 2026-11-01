use crate::{
    lifecycle::{Lifecycle, Phase},
    media::{self, ConversionResult},
    recording,
    settings::{self, Settings},
};
use chrono::Local;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{channel, Receiver, Sender},
    thread::JoinHandle,
    time::Instant,
};
#[derive(PartialEq, Clone, Copy)]
pub enum ActiveTab {
    Recorder,
    Settings,
    Recovery,
}
enum JobEvent {
    Status(Phase, String),
    Progress(f32),
    Finished(Result<Option<ConversionResult>, String>),
}
struct Worker {
    stop: Option<Sender<()>>,
    events: Receiver<JobEvent>,
    handle: Option<JoinHandle<()>>,
    completed: bool,
}
impl Worker {
    fn spawn(work: impl FnOnce(Receiver<()>, Sender<JobEvent>) + Send + 'static) -> Self {
        let (stop_tx, stop_rx) = channel();
        let (tx, events) = channel();
        let handle = std::thread::spawn(move || work(stop_rx, tx));
        Self {
            stop: Some(stop_tx),
            events,
            handle: Some(handle),
            completed: false,
        }
    }
    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
    fn finished(&self) -> bool {
        self.handle.as_ref().is_none_or(|h| h.is_finished())
    }
    fn join_if_finished(&mut self) -> Option<(bool, Vec<JobEvent>)> {
        if !self.finished() {
            return None;
        }
        let panicked = self.handle.take()?.join().is_err();
        Some((panicked, self.events.try_iter().collect()))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct RecorderApp {
    pub active_tab: ActiveTab,
    pub settings: Settings,
    saved_settings: Settings,
    config_path: PathBuf,
    pub lifecycle: Lifecycle,
    pub start_time: Option<Instant>,
    pub status_msg: String,
    worker: Option<Worker>,
    ipc_rx: Receiver<String>,
    pub conversion_progress: f32,
    pub pending_recordings: Vec<PathBuf>,
    pub manual_mkv: String,
    pub last_output: Option<PathBuf>,
    pub settings_error: Option<String>,
}
impl RecorderApp {
    pub fn new(ipc_rx: Receiver<String>) -> Self {
        let config_path = settings::config_path();
        let (settings, settings_error) = match settings::load(&config_path) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        };
        let pending = media::recoverable_files(Path::new(&settings.save_directory));
        let (pending_recordings, scan_error) = match pending {
            Ok(v) => (v, None),
            Err(e) => (vec![], Some(e)),
        };
        let status_msg = if pending_recordings.is_empty() {
            "就緒".into()
        } else {
            format!(
                "發現 {} 個尚未轉換的 MKV，可至「錄影救援」處理",
                pending_recordings.len()
            )
        };
        Self {
            active_tab: if pending_recordings.is_empty() {
                ActiveTab::Recorder
            } else {
                ActiveTab::Recovery
            },
            saved_settings: settings.clone(),
            settings,
            config_path,
            lifecycle: Lifecycle::new(),
            start_time: None,
            status_msg,
            worker: None,
            ipc_rx,
            conversion_progress: 0.0,
            pending_recordings,
            manual_mkv: String::new(),
            last_output: None,
            settings_error: settings_error.or(scan_error),
        }
    }
    pub fn busy(&self) -> bool {
        self.worker.is_some() || self.lifecycle.busy()
    }
    pub fn toggle_recording(&mut self) {
        match self.lifecycle.phase {
            Phase::Idle | Phase::Failed if !self.busy() => self.start_recording(),
            Phase::Starting | Phase::Recording => self.stop_recording(),
            _ => {}
        }
    }
    pub fn refresh_recovery(&mut self) {
        match media::recoverable_files(Path::new(&self.settings.save_directory)) {
            Ok(files) => self.pending_recordings = files,
            Err(e) => self.status_msg = e,
        }
    }
    pub fn start_recording(&mut self) {
        if self.busy() {
            return;
        }
        let folder = PathBuf::from(&self.settings.save_directory);
        let prepare = (|| -> Result<PathBuf, String> {
            if folder.as_os_str().is_empty() {
                return Err("請先設定錄影儲存目錄".into());
            }
            fs::create_dir_all(&folder).map_err(|e| format!("無法建立儲存目錄: {e}"))?;
            if fs2::available_space(&folder).map_err(|e| e.to_string())? < recording::MIN_FREE_BYTES
            {
                return Err("磁碟空間不足，至少需要 256 MiB 可用空間".into());
            }
            // Both programs are required before capture so a missing converter is discovered early.
            for tool in ["ffmpeg", "ffprobe"] {
                if !Command::new(tool)
                    .arg("-version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
                {
                    return Err(format!("請先安裝 FFmpeg（找不到 {tool}）"));
                }
            }
            for suffix in 0..1000 {
                let path = folder.join(format!(
                    "recording_{}_{suffix}.mkv",
                    Local::now().format("%Y%m%d_%H%M%S_%3f")
                ));
                match OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(_) => return Ok(path),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(format!("無法建立錄影檔: {e}")),
                }
            }
            Err("無法產生唯一錄影檔名".into())
        })();
        let path = match prepare {
            Ok(path) => path,
            Err(e) => {
                self.lifecycle.finish(false);
                self.status_msg = e;
                return;
            }
        };
        let prefs = self.settings.clone();
        self.lifecycle.set_phase(Phase::Starting);
        self.start_time = Some(Instant::now());
        self.conversion_progress = 0.0;
        self.status_msg = "正在請求錄影授權…".into();
        self.worker = Some(Worker::spawn(move |stop, tx| {
            let result = recording::record(&prefs, &path, &stop, |phase, text| {
                let _ = tx.send(JobEvent::Status(phase, text));
            })
            .and_then(|recorded| {
                if !recorded {
                    return Ok(None);
                }
                let _ = tx.send(JobEvent::Status(
                    Phase::Converting,
                    "正在將 MKV 轉為 MP4…".into(),
                ));
                media::convert_mkv(&path, prefs.keep_mkv, |p| {
                    let _ = tx.send(JobEvent::Progress(p));
                })
                .map(Some)
            });
            if !matches!(result, Ok(Some(_))) && path.metadata().is_ok_and(|m| m.len() == 0) {
                let _ = fs::remove_file(&path);
            }
            let _ = tx.send(JobEvent::Finished(result));
        }));
    }
    pub fn stop_recording(&mut self) {
        if matches!(self.lifecycle.phase, Phase::Starting | Phase::Recording) {
            self.lifecycle.stop();
            if let Some(w) = &mut self.worker {
                w.stop();
            }
            self.status_msg = "正在停止並儲存，完成前請稍候…".into();
        }
    }
    pub fn start_conversion(&mut self, path: PathBuf) {
        if self.busy() {
            return;
        }
        let keep = self.settings.keep_mkv;
        self.lifecycle.set_phase(Phase::Converting);
        self.conversion_progress = 0.0;
        self.status_msg = format!("正在轉換 {}", path.display());
        self.worker = Some(Worker::spawn(move |_stop, tx| {
            let result = media::convert_mkv(&path, keep, |p| {
                let _ = tx.send(JobEvent::Progress(p));
            })
            .map(Some);
            let _ = tx.send(JobEvent::Finished(result));
        }));
    }
    pub fn poll_worker(&mut self) {
        let mut events: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| w.events.try_iter().collect())
            .unwrap_or_default();
        let joined = self.worker.as_mut().and_then(|w| w.join_if_finished());
        let panicked = joined.map(|(panicked, pending)| {
            events.extend(pending);
            panicked
        });
        for event in events {
            match event {
                JobEvent::Status(phase, msg) => {
                    self.lifecycle.set_phase(phase);
                    self.status_msg = msg;
                }
                JobEvent::Progress(value) => self.conversion_progress = value,
                JobEvent::Finished(result) => {
                    if let Some(w) = &mut self.worker {
                        w.completed = true;
                    }
                    self.start_time = None;
                    self.lifecycle.finish(result.is_ok());
                    match result {
                        Ok(Some(out)) => {
                            self.status_msg = format!(
                                "錄影已完成，已儲存至: {}（{}）",
                                out.mp4.display(),
                                if out.mkv_retained {
                                    "保留 MKV"
                                } else {
                                    "已依設定刪除 MKV"
                                }
                            );
                            if let Some(warning) = out.warning {
                                self.status_msg.push_str(&format!("\n{warning}"));
                            }
                            if !self.lifecycle.close_requested {
                                if self.settings.auto_play {
                                    let _ = Command::new("xdg-open").arg(&out.mp4).spawn();
                                }
                                if self.settings.auto_open_folder {
                                    let _ = Command::new("xdg-open")
                                        .arg(out.mp4.parent().unwrap_or(Path::new(".")))
                                        .spawn();
                                }
                            }
                            self.last_output = Some(out.mp4);
                        }
                        Ok(None) => {
                            self.status_msg = "已取消錄影".into();
                        }
                        Err(e) => {
                            self.lifecycle.close_requested = false;
                            self.status_msg = format!("未完成: {e}");
                            self.active_tab = ActiveTab::Recovery;
                        }
                    }
                    self.refresh_recovery();
                }
            }
        }
        if let Some(panicked) = panicked {
            let w = self.worker.take().unwrap();
            if panicked || !w.completed {
                self.lifecycle.finish(false);
                self.lifecycle.close_requested = false;
                self.status_msg = "背景錄影工作意外結束，請至錄影救援檢查 MKV".into();
                self.refresh_recovery();
            }
        }
    }
    pub fn install_desktop_entry(&mut self) {
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "capra".to_string());

        let desktop_dir = std::env::var("HOME")
            .map(|h| format!("{h}/.local/share/applications"))
            .unwrap_or_else(|_| ".".to_string());

        let _ = std::fs::create_dir_all(&desktop_dir);
        let desktop_file_path = format!("{desktop_dir}/capra.desktop");

        let desktop_content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=capra\n\
             Name[zh_TW]=capra\n\
             Comment=Ubuntu 螢幕畫面與聲音錄影軟體\n\
             Exec={exe_path}\n\
             Icon=media-record\n\
             Terminal=false\n\
             Categories=AudioVideo;Recorder;Utility;\n\
             StartupNotify=true\n"
        );

        match std::fs::write(&desktop_file_path, desktop_content) {
            Ok(_) => {
                let _ = Command::new("update-desktop-database")
                    .arg(&desktop_dir)
                    .status();
                self.status_msg = "已成功新增至 Ubuntu 應用程式選單 (Show Apps)！".to_string();
            }
            Err(e) => {
                self.status_msg = format!("安裝圖示失敗: {e}");
            }
        }
    }

    pub fn register_gnome_global_shortcut(&mut self) {
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "capra".to_string());

        let cmd = format!("{exe_path} --toggle");
        let binding = &self.settings.global_shortcut_key;

        let script = format!(
            r#"
            KEY_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom-capra/"
            gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "['$KEY_PATH']"
            gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$KEY_PATH name 'capra 錄影切換'
            gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$KEY_PATH command '{cmd}'
            gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$KEY_PATH binding '{binding}'
            "#
        );

        match Command::new("bash").arg("-c").arg(&script).output() {
            Ok(output) if output.status.success() => {
                self.status_msg = format!("成功將全域快捷鍵 [{binding}] 寫入 Ubuntu 系統設定！");
            }
            Ok(output) => {
                self.status_msg = format!("寫入失敗: {}", String::from_utf8_lossy(&output.stderr));
            }
            Err(e) => {
                self.status_msg = format!("執行指令失敗: {e}");
            }
        }
    }
}

impl RecorderApp {
    pub fn request_close(&mut self) -> bool {
        if !self.busy() {
            return true;
        }
        self.lifecycle.request_close();
        if let Some(worker) = &mut self.worker {
            worker.stop();
        }
        self.status_msg = "正在完成儲存，完成後會自動關閉…".into();
        false
    }
    pub fn should_close(&self) -> bool {
        self.lifecycle.can_close(self.worker.is_none())
    }
    pub fn poll_ipc(&mut self) -> bool {
        let mut focus = false;
        while let Ok(command) = self.ipc_rx.try_recv() {
            if command == "toggle" {
                self.toggle_recording();
            } else if command == "focus" {
                focus = true;
            }
        }
        focus
    }
    pub fn persist_settings(&mut self) {
        if self.settings != self.saved_settings {
            if self.settings.save_directory != self.saved_settings.save_directory {
                self.refresh_recovery();
            }
            match settings::save(&self.config_path, &self.settings) {
                Ok(()) => {
                    self.saved_settings = self.settings.clone();
                    self.settings_error = None;
                }
                Err(e) => self.settings_error = Some(e),
            }
        }
    }
}
#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn dropping_worker_waits_for_background_file_write() {
        let path = std::env::temp_dir().join(format!("recorder-worker-{}.txt", std::process::id()));
        let output = path.clone();
        let worker = Worker::spawn(move |stop, _events| {
            stop.recv().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            fs::write(output, b"fully saved").unwrap();
        });
        drop(worker);
        assert_eq!(fs::read(&path).unwrap(), b"fully saved");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn joining_drains_completion_queued_after_an_earlier_poll() {
        let mut worker = Worker::spawn(|stop, tx| {
            stop.recv().unwrap();
            tx.send(JobEvent::Progress(1.0)).unwrap();
        });
        assert!(worker.events.try_iter().next().is_none());
        worker.stop();
        while !worker.finished() {
            std::thread::yield_now();
        }
        let (panicked, events) = worker.join_if_finished().unwrap();
        assert!(!panicked);
        assert!(events
            .iter()
            .any(|e| matches!(e,JobEvent::Progress(p)if *p==1.0)));
    }
    fn isolated_app() -> RecorderApp {
        let (_, ipc_rx) = channel();
        let settings = Settings::default();
        RecorderApp {
            active_tab: ActiveTab::Recorder,
            saved_settings: settings.clone(),
            settings,
            config_path: PathBuf::new(),
            lifecycle: Lifecycle::new(),
            start_time: None,
            status_msg: String::new(),
            worker: None,
            ipc_rx,
            conversion_progress: 0.0,
            pending_recordings: vec![],
            manual_mkv: String::new(),
            last_output: None,
            settings_error: None,
        }
    }
    #[test]
    fn native_close_request_waits_for_worker_and_finalization() {
        let mut app = isolated_app();
        app.lifecycle.set_phase(Phase::Recording);
        app.worker = Some(Worker::spawn(|stop, tx| {
            stop.recv().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            tx.send(JobEvent::Finished(Ok(None))).unwrap();
        }));
        assert!(!app.request_close());
        assert!(app.busy());
        assert!(!app.should_close());
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.busy() && Instant::now() < deadline {
            app.poll_worker();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.should_close());
    }
    #[test]
    fn idle_window_can_close_immediately() {
        assert!(isolated_app().request_close());
    }
    #[test]
    fn invalid_recording_directory_reports_failed_state_to_the_ui() {
        let mut app = isolated_app();
        app.settings.save_directory.clear();
        app.start_recording();
        assert_eq!(app.lifecycle.phase, Phase::Failed);
        assert!(!app.busy());
        assert!(app.status_msg.contains("儲存目錄"));
    }
}
