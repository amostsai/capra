use crate::{
    lifecycle::Phase,
    settings::{Settings, VideoBackend},
};
use ashpd::desktop::screencast::{CursorMode, PersistMode, Screencast, SourceType};
use gstreamer::{self as gst, prelude::*};
use std::{
    fs::File,
    future::Future,
    os::fd::AsRawFd,
    path::Path,
    process::Command,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;
pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn stopped(stop: &Receiver<()>) -> bool {
    matches!(stop.try_recv(), Ok(()) | Err(TryRecvError::Disconnected))
}
async fn interruptible<F, T>(future: F, stop: &Receiver<()>) -> Result<Option<T>, String>
where
    F: Future<Output = Result<T, ashpd::Error>>,
{
    let mut future = Box::pin(future);
    loop {
        if stopped(stop) {
            return Ok(None);
        }
        match async_std::future::timeout(Duration::from_millis(100), future.as_mut()).await {
            Ok(result) => return result.map(Some).map_err(|e| format!("螢幕授權失敗: {e}")),
            Err(_) => continue,
        }
    }
}
struct PipelineGuard(gst::Pipeline);
impl Drop for PipelineGuard {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

/// Only a real EOS is a successful finalization; bus inactivity is not success.
pub fn finish_pipeline(pipeline: &gst::Pipeline, timeout: Duration) -> Result<(), String> {
    if !pipeline.send_event(gst::event::Eos::new()) {
        return Err("無法送出停止事件，MKV 已保留".into());
    }
    let bus = pipeline.bus().ok_or("無法取得影音訊息匯流排")?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err("等待錄影收尾逾時，MKV 已保留，可從救援頁重試".into());
        }
        if let Some(msg) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(100),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            match msg.view() {
                gst::MessageView::Eos(..) => return Ok(()),
                gst::MessageView::Error(e) => {
                    return Err(format!("錄影收尾失敗: {}；MKV 已保留", e.error()))
                }
                _ => {}
            }
        }
    }
}

pub fn record(
    settings: &Settings,
    path: &Path,
    stop: &Receiver<()>,
    report: impl Fn(Phase, String),
) -> Result<bool, String> {
    async_std::task::block_on(async {
        let mut portal_session = None;
        let mut pipewire_fd = None;
        let result:Result<bool,String>=async {
   macro_rules! wait { ($future:expr) => {match interruptible($future,stop).await? {Some(value)=>value,None=>return Ok(false)}}; }
   let wayland=match settings.video_backend {VideoBackend::Auto=>std::env::var("XDG_SESSION_TYPE").map(|s|s.eq_ignore_ascii_case("wayland")).unwrap_or(false),VideoBackend::WaylandPipeWire=>true,VideoBackend::X11=>false};
   let fps=settings.target_fps;let bitrate=settings.video_bitrate;
   let source=if wayland{
    report(Phase::Starting,"請在系統視窗選擇要錄製的螢幕或視窗".into());
    let proxy=wait!(Screencast::new());
    portal_session=Some(wait!(proxy.create_session()));
    let session=portal_session.as_ref().unwrap();
    wait!(proxy.select_sources(session,CursorMode::Embedded,SourceType::Monitor|SourceType::Window,false,None,PersistMode::DoNot));
    let identifier=ashpd::WindowIdentifier::default();
    let response=wait!(proxy.start(session,&identifier)).response().map_err(|e|format!("畫面選擇已取消: {e}"))?;
    let node_id=response.streams().first().ok_or("未選取任何畫面")?.pipe_wire_node_id();
    let fd=wait!(proxy.open_pipe_wire_remote(session));let raw_fd=fd.as_raw_fd();pipewire_fd=Some(fd);
    format!("pipewiresrc fd={raw_fd} path={node_id} do-timestamp=true keepalive-time=1000")
   }else{"ximagesrc use-damage=0".into()};
   if stopped(stop){return Ok(false);}
   let video=format!("{source} ! videoconvert ! videorate ! video/x-raw,framerate={fps}/1 ! queue ! x264enc tune=zerolatency speed-preset=veryfast bitrate={bitrate} key-int-max={} ! h264parse ! queue ! mux.",fps*2);
   let sys_dev=device("get-default-sink").map(|s|format!("device={}",quote(&format!("{s}.monitor")))).unwrap_or_default();
   let mic_dev=device("get-default-source").map(|s|format!("device={}",quote(&s))).unwrap_or_default();
   let sys=settings.system_volume;let mic=settings.mic_volume;
   let encode="audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate=48000,channels=2 ! voaacenc bitrate=192000 ! aacparse ! queue ! mux.";
   let audio=match(settings.record_system,settings.record_mic){
    (true,true)=>format!("audiomixer name=mix ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! {encode} pulsesrc {sys_dev} do-timestamp=true ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! volume volume={sys:.2} ! queue max-size-time=1000000000 ! mix. pulsesrc {mic_dev} do-timestamp=true ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! volume volume={mic:.2} ! queue max-size-time=1000000000 ! mix."),
    (true,false)=>format!("pulsesrc {sys_dev} do-timestamp=true ! volume volume={sys:.2} ! {encode}"),
    (false,true)=>format!("pulsesrc {mic_dev} do-timestamp=true ! volume volume={mic:.2} ! {encode}"),
    (false,false)=>String::new(),
   };
   let desc=format!("matroskamux name=mux streamable=true max-cluster-duration=2000000000 ! filesink location={} {video} {audio}",quote(&path.to_string_lossy()));
   let pipeline=gst::parse::launch(&desc).map_err(|e|format!("建立錄影管線失敗: {e}"))?.dynamic_cast::<gst::Pipeline>().map_err(|_|"無法建立錄影管線")?;
   let guard=PipelineGuard(pipeline);let pipeline=&guard.0;
   pipeline.set_state(gst::State::Playing).map_err(|e|format!("啟動錄影失敗: {e}"))?;
   report(Phase::Recording,format!("錄影中 → {}",path.display()));
   let bus=pipeline.bus().ok_or("無法取得錄影訊息")?;let mut last_disk_check=Instant::now();let mut ended=false;
   loop{
    if stopped(stop){break;}
    if let Some(msg)=bus.timed_pop_filtered(gst::ClockTime::from_mseconds(100),&[gst::MessageType::Eos,gst::MessageType::Error]){
     match msg.view(){gst::MessageView::Eos(..)=>{ended=true;break;},gst::MessageView::Error(e)=>return Err(format!("錄影失敗: {}；MKV 已保留",e.error())),_=>{}}
    }
    if last_disk_check.elapsed()>=Duration::from_secs(2){
     let available=fs2::available_space(path.parent().unwrap_or(Path::new("."))).map_err(|e|format!("無法監控磁碟空間: {e}"))?;
     let recorded=path.metadata().map(|m|m.len()).unwrap_or(0);
     if available<recorded.saturating_add(MIN_FREE_BYTES){report(Phase::Saving,"磁碟空間不足，已提前停止並保留轉換所需空間".into());break;}
     last_disk_check=Instant::now();
    }
   }
   report(Phase::Saving,"正在完成 MKV 寫入，請稍候…".into());
   if !ended{finish_pipeline(pipeline,Duration::from_secs(30))?;}
   pipeline.set_state(gst::State::Null).map_err(|e|format!("關閉錄影檔失敗: {e}"))?;
   File::open(path).and_then(|f|f.sync_all()).map_err(|e|format!("寫入 MKV 失敗: {e}"))?;
   Ok(true)
  }.await;
        if let Some(session) = portal_session {
            let _ = async_std::future::timeout(Duration::from_secs(5), session.close()).await;
        }
        drop(pipewire_fd);
        result
    })
}
fn device(operation: &str) -> Option<String> {
    let out = Command::new("pactl").arg(operation).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s.starts_with('@') {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finalization_requires_eos_not_just_an_idle_bus() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch("videotestsrc is-live=true ! fakesink name=sink")
            .unwrap()
            .dynamic_cast::<gst::Pipeline>()
            .unwrap();
        let guard = PipelineGuard(pipeline);
        let sink = guard.0.by_name("sink").unwrap();
        sink.static_pad("sink").unwrap().add_probe(
            gst::PadProbeType::EVENT_DOWNSTREAM,
            |_, info| {
                if info
                    .event()
                    .is_some_and(|e| matches!(e.view(), gst::EventView::Eos(_)))
                {
                    gst::PadProbeReturn::Drop
                } else {
                    gst::PadProbeReturn::Ok
                }
            },
        );
        guard.0.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(finish_pipeline(&guard.0, Duration::from_millis(200))
            .unwrap_err()
            .contains("逾時"));
    }
    #[test]
    fn real_gstreamer_mkv_finalizes_and_converts_with_audio() {
        gst::init().unwrap();
        let dir = std::env::temp_dir().join(format!("recorder-gst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recording_test.mkv");
        let desc=format!("matroskamux name=mux streamable=true max-cluster-duration=2000000000 ! filesink location={} videotestsrc is-live=true ! video/x-raw,width=160,height=120,framerate=30/1 ! x264enc tune=zerolatency key-int-max=30 ! h264parse ! queue ! mux. audiotestsrc is-live=true ! audioconvert ! audio/x-raw,format=S16LE,rate=48000,channels=2 ! voaacenc bitrate=192000 ! aacparse ! queue ! mux.",quote(&path.to_string_lossy()));
        let guard = PipelineGuard(
            gst::parse::launch(&desc)
                .unwrap()
                .dynamic_cast::<gst::Pipeline>()
                .unwrap(),
        );
        guard.0.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        finish_pipeline(&guard.0, Duration::from_secs(5)).unwrap();
        drop(guard);
        let output = crate::media::convert_mkv(&path, true, |_| {}).unwrap();
        assert!(output.mp4.exists());
        assert!(path.exists());
        assert!(Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-xerror", "-i"])
            .arg(&output.mp4)
            .args(["-f", "null", "-"])
            .status()
            .unwrap()
            .success());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn closing_before_portal_selection_is_expected_cancellation() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(()).unwrap();
        let settings = Settings {
            video_backend: VideoBackend::WaylandPipeWire,
            ..Settings::default()
        };
        assert!(record(
            &settings,
            Path::new("/tmp/unused-cancelled.mkv"),
            &rx,
            |_, _| {}
        )
        .is_ok());
    }
}

#[cfg(test)]
mod crash_tests {
    use super::*;
    #[test]
    fn killed_gstreamer_recording_can_be_recovered_as_mp4() {
        let dir = std::env::temp_dir().join(format!("recorder-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mkv = dir.join("recording_interrupted.mkv");
        let description=format!("matroskamux name=mux streamable=true max-cluster-duration=2000000000 ! filesink location={} videotestsrc is-live=true pattern=snow ! video/x-raw,width=320,height=240,framerate=30/1 ! x264enc tune=zerolatency bitrate=1000 key-int-max=30 ! h264parse ! queue ! mux. audiotestsrc is-live=true ! audioconvert ! audio/x-raw,format=S16LE,rate=48000,channels=2 ! voaacenc bitrate=192000 ! aacparse ! queue ! mux.",quote(&mkv.to_string_lossy()));
        let mut child = Command::new("gst-launch-1.0")
            .arg("-q")
            .args(description.split_whitespace())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(4));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(mkv.metadata().unwrap().len() > 10000);
        let result = crate::media::convert_mkv(&mkv, true, |_| {}).unwrap();
        assert!(mkv.exists());
        assert!(result.mp4.exists());
        assert!(Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-xerror", "-i"])
            .arg(&result.mp4)
            .args(["-f", "null", "-"])
            .status()
            .unwrap()
            .success());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
