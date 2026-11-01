extern crate gpui_kit as gpui;
mod ui;

use capra::controller::RecorderApp;
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::mpsc::channel,
};

const IPC_SOCKET_PATH: &str = "/tmp/capra_amos.sock";
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let toggle = std::env::args().any(|arg| arg == "--toggle");
    if let Ok(mut stream) = UnixStream::connect(IPC_SOCKET_PATH) {
        stream.write_all(if toggle { b"toggle\n" } else { b"focus\n" })?;
        return Ok(());
    }
    if toggle {
        eprintln!("未發現運行中的錄影主程式");
        return Ok(());
    }
    gstreamer::init()?;
    let _ = std::fs::remove_file(IPC_SOCKET_PATH);
    let listener = UnixListener::bind(IPC_SOCKET_PATH)?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buffer = [0; 16];
            if let Ok(n) = stream.read(&mut buffer) {
                let _ = tx.send(String::from_utf8_lossy(&buffer[..n]).trim().to_string());
            }
        }
    });
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            Theme::update(cx, |theme| {
                theme.font_family = "Noto Sans CJK TC".into();
                theme.font_size = px(14.);
                theme.colors.primary = rgb(0x087f8c).into();
                theme.colors.primary_hover = rgb(0x096c77).into();
                theme.colors.primary_active = rgb(0x075c65).into();
                theme.colors.primary_foreground = rgb(0xffffff).into();
                theme.colors.ring = rgb(0x087f8c).into();
            });
            cx.bind_keys([
                KeyBinding::new("f9", ui::ToggleRecording, Some("Recorder")),
                KeyBinding::new("ctrl-q", ui::CloseRecorder, Some("Recorder")),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1020.), px(760.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("capra".into()),
                        appears_transparent: false,
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(820.), px(620.))),
                    app_id: Some("capra".into()),
                    window_decorations: Some(WindowDecorations::Client),
                    app_owns_titlebar_drag: true,
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let view = cx.new(|cx| ui::RecorderView::new(RecorderApp::new(rx), window, cx));
                    let weak = view.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        weak.update(cx, |view, cx| {
                            let allowed = view.app.request_close();
                            cx.notify();
                            allowed
                        })
                        .unwrap_or(true)
                    });
                    view
                },
            )
            .expect("無法建立錄影視窗");
            cx.activate(true);
        });
    let _ = std::fs::remove_file(IPC_SOCKET_PATH);
    Ok(())
}
