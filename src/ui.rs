use capra::{
    controller::{ActiveTab, RecorderApp},
    lifecycle::Phase,
    settings::VideoBackend,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        button::{Button, ButtonCustomVariant, ButtonVariants},
        input::{Input, InputEvent, InputState},
        progress::Progress,
        slider::{Slider, SliderEvent, SliderState, SliderValue},
        switch::Switch,
        Disableable, Icon, IconName, Selectable, Sizable, TitleBar,
    },
    *,
};
use std::{path::PathBuf, process::Command, time::Duration};

fn app_logo() -> std::sync::Arc<Image> {
    static LOGO: std::sync::OnceLock<std::sync::Arc<Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        std::sync::Arc::new(Image::from_bytes(
            ImageFormat::Png,
            capra::APP_ICON_PNG.to_vec(),
        ))
    })
    .clone()
}

gpui_kit::actions!(recorder, [ToggleRecording, CloseRecorder]);
const INK: u32 = 0x172b36;
const MUTED: u32 = 0x74838d;
const LINE: u32 = 0xe5ebef;
const ACCENT: u32 = 0x087f8c;
const BACKGROUND: u32 = 0xf5f7fa;

pub struct RecorderView {
    pub app: RecorderApp,
    focus: FocusHandle,
    directory: Entity<InputState>,
    shortcut: Entity<InputState>,
    manual: Entity<InputState>,
    system_volume: Entity<SliderState>,
    mic_volume: Entity<SliderState>,
    picking: bool,
}
impl RecorderView {
    pub fn new(app: RecorderApp, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let directory = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(app.settings.save_directory.clone())
                .placeholder("影片儲存資料夾")
        });
        let shortcut = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(app.settings.global_shortcut_key.clone())
                .placeholder("F9 或 <Control><Alt>r")
        });
        let manual = cx.new(|cx| {
            InputState::new(window, cx).placeholder("選擇或拖入 MKV 檔案，也可以貼上檔案路徑")
        });
        let system_volume = cx.new(|_| {
            SliderState::new()
                .min(0.1)
                .max(1.5)
                .step(0.05)
                .default_value(app.settings.system_volume)
        });
        let mic_volume = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(3.0)
                .step(0.05)
                .default_value(app.settings.mic_volume)
        });
        cx.subscribe(&directory, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.app.busy() {
                this.app.settings.save_directory = state.read(cx).value().to_string();
                this.changed(cx);
            }
        })
        .detach();
        cx.subscribe(&shortcut, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.app.busy() {
                this.app.settings.global_shortcut_key = state.read(cx).value().to_string();
                this.changed(cx);
            }
        })
        .detach();
        cx.subscribe(&manual, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.app.busy() {
                this.app.manual_mkv = state.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&system_volume, |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                if !this.app.busy() {
                    this.app.settings.system_volume = *value;
                    this.changed(cx);
                }
            }
        })
        .detach();
        cx.subscribe(&mic_volume, |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                if !this.app.busy() {
                    this.app.settings.mic_volume = *value;
                    this.changed(cx);
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.on_focus_lost(window, |this, window, cx| {
            let target = window
                .focus_lost_restore_target(cx)
                .unwrap_or_else(|| this.focus.clone());
            window.focus(&target, cx);
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            if this
                .update_in(cx, |view, window, cx| {
                    let was_busy = view.app.busy();
                    let old_phase = view.app.lifecycle.phase;
                    view.app.poll_worker();
                    if view.app.poll_ipc() {
                        window.activate_window();
                    }
                    if view.app.should_close() {
                        window.remove_window();
                        return;
                    }
                    if was_busy || view.app.busy() || old_phase != view.app.lifecycle.phase {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        Self {
            app,
            focus,
            directory,
            shortcut,
            manual,
            system_volume,
            mic_volume,
            picking: false,
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.app.persist_settings();
        cx.notify();
    }
    fn toggle(&mut self, _: &ToggleRecording, _: &mut Window, cx: &mut Context<Self>) {
        self.app.toggle_recording();
        cx.notify();
    }
    fn close(&mut self, _: &CloseRecorder, window: &mut Window, cx: &mut Context<Self>) {
        if self.app.request_close() {
            window.remove_window();
        }
        cx.notify();
    }
    fn open_folder(&self) {
        let _ = Command::new("xdg-open")
            .arg(&self.app.settings.save_directory)
            .spawn();
    }
    fn pick(&mut self, directory: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking || self.app.busy() {
            return;
        }
        self.picking = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let selected = cx
                .background_executor()
                .spawn(async move {
                    let picker = rfd::FileDialog::new();
                    if directory {
                        picker.pick_folder()
                    } else {
                        picker.add_filter("Matroska 影片", &["mkv"]).pick_file()
                    }
                })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                view.picking = false;
                if let Some(path) = selected {
                    let text = path.to_string_lossy().to_string();
                    if directory {
                        view.app.settings.save_directory = text.clone();
                        view.directory
                            .update(cx, |input, cx| input.set_value(text, window, cx));
                        view.app.persist_settings();
                    } else {
                        view.set_manual(path, window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn set_manual(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.app.busy() {
            return;
        }
        self.app.manual_mkv = path.to_string_lossy().to_string();
        let value = self.app.manual_mkv.clone();
        self.manual
            .update(cx, |input, cx| input.set_value(value, window, cx));
        self.app.active_tab = ActiveTab::Recovery;
        window.focus(&self.focus, cx);
        cx.notify();
    }
    fn phase(&self) -> (&'static str, u32) {
        match self.app.lifecycle.phase {
            Phase::Idle => ("準備就緒", 0x18856b),
            Phase::Starting => ("等待畫面授權", 0xad781c),
            Phase::Recording => ("錄影中", 0xd44550),
            Phase::Saving => ("正在儲存", 0xad781c),
            Phase::Converting => ("正在轉換", ACCENT),
            Phase::Failed => ("需要處理", 0xd44550),
        }
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let mut nav = div().flex().flex_col().gap_2();
        for (id, page, label, icon) in [
            ("nav-recorder", ActiveTab::Recorder, "錄影", IconName::Frame),
            (
                "nav-recovery",
                ActiveTab::Recovery,
                "錄影救援",
                IconName::RotateCw,
            ),
            (
                "nav-settings",
                ActiveTab::Settings,
                "設定",
                IconName::Settings,
            ),
        ] {
            let active = self.app.active_tab == page;
            nav = nav.child(
                Button::new(id)
                    .ghost()
                    .icon(icon)
                    .label(label)
                    .w_full()
                    .h(px(44.))
                    .justify_start()
                    .selected(active)
                    .when(active, |button| {
                        button.bg(rgb(0xe8f5f5)).text_color(rgb(ACCENT))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.app.active_tab = page;
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .w(px(206.))
            .flex_shrink_0()
            .h_full()
            .p_5()
            .bg(rgb(0xffffff))
            .border_r_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pt_3()
                    .pb_8()
                    .child(img(app_logo()).size(px(48.)).object_fit(ObjectFit::Contain))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(17.))
                                    .child("capra"),
                            )
                            .child(muted("螢幕錄影工具").text_size(px(9.))),
                    ),
            )
            .child(nav)
            .child(div().flex_1())
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .text_color(rgb(0x18856b))
                            .text_size(px(12.))
                            .child(Icon::new(IconName::CircleCheck).size(px(15.)))
                            .child("安全儲存已啟用"),
                    )
                    .child(
                        muted("錄下畫面，也安心保留。\n中斷的錄影可於救援頁處理。")
                            .text_size(px(10.)),
                    ),
            )
    }
    fn recorder(&self, cx: &mut Context<Self>) -> Div {
        let phase = self.app.lifecycle.phase;
        let busy = self.app.busy();
        let (label, color) = self.phase();
        let elapsed = self
            .app
            .start_time
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let can_stop = matches!(phase, Phase::Starting | Phase::Recording);
        let button_label = if !busy {
            "開始錄影"
        } else if can_stop {
            "停止並儲存"
        } else {
            "正在處理…"
        };
        let hero = card()
            .p_6()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(muted(if busy {
                                "專注你的畫面，錄影正在進行"
                            } else {
                                "每個值得記錄的時刻"
                            }))
                            .child(
                                div()
                                    .text_size(px(52.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!(
                                        "{:02}:{:02}:{:02}",
                                        elapsed / 3600,
                                        elapsed % 3600 / 60,
                                        elapsed % 60
                                    )),
                            ),
                    )
                    .child(
                        Button::new("record-toggle")
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(rgb(0xfff0f1).into())
                                    .hover(rgb(0xffe0e4).into())
                                    .active(rgb(0xffcdd4).into()),
                            )
                            .size(px(84.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_0()
                            .p_0()
                            .accessibility_label(button_label)
                            .tooltip(button_label)
                            .disabled(busy && !can_stop)
                            .when(busy && !can_stop, |button| button.opacity(0.45))
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex_shrink_0()
                                    .rounded(if can_stop { px(4.) } else { px(17.) })
                                    .bg(rgb(0xe45461)),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.toggle_recording();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .min_h(px(46.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(rgb(color))
                            .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                            .child(label),
                    ),
            )
            .when(busy, |panel| {
                panel.child(
                    Progress::new("record-progress")
                        .color(rgb(ACCENT))
                        .value(self.app.conversion_progress * 100.)
                        .loading(phase != Phase::Converting),
                )
            });
        let audio = div()
            .flex()
            .gap_4()
            .child(
                card()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                Icon::new(IconName::HardDrive)
                                    .size(px(22.))
                                    .text_color(rgb(ACCENT)),
                            )
                            .child(
                                Switch::new("system-audio")
                                    .accessibility_label("錄製系統聲音")
                                    .checked(self.app.settings.record_system)
                                    .disabled(busy)
                                    .color(rgb(ACCENT))
                                    .on_change(cx.listener(|this, value, _, cx| {
                                        this.app.settings.record_system = *value;
                                        this.changed(cx);
                                    })),
                            ),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("系統聲音"))
                    .child(muted("錄下電腦播放的聲音")),
            )
            .child(
                card()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                Icon::new(IconName::Mic)
                                    .size(px(22.))
                                    .text_color(rgb(ACCENT)),
                            )
                            .child(
                                Switch::new("microphone")
                                    .accessibility_label("錄製麥克風")
                                    .checked(self.app.settings.record_mic)
                                    .disabled(busy)
                                    .color(rgb(ACCENT))
                                    .on_change(cx.listener(|this, value, _, cx| {
                                        this.app.settings.record_mic = *value;
                                        this.changed(cx);
                                    })),
                            ),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("麥克風"))
                    .child(muted("加入你的聲音與講解")),
            );
        let mut backends = div().flex().gap_2();
        for (id, value, label) in [
            ("backend-auto", VideoBackend::Auto, "自動偵測"),
            ("backend-wayland", VideoBackend::WaylandPipeWire, "Wayland"),
            ("backend-x11", VideoBackend::X11, "X11"),
        ] {
            backends = backends.child(
                Button::new(id)
                    .label(label)
                    .selected(self.app.settings.video_backend == value)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings.video_backend = value;
                        this.changed(cx);
                    })),
            );
        }
        let mut fps = div().flex().gap_2();
        for (id, value, label) in [("fps-30", 30, "30 FPS"), ("fps-60", 60, "60 FPS")] {
            fps = fps.child(
                Button::new(id)
                    .label(label)
                    .selected(self.app.settings.target_fps == value)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings.target_fps = value;
                        this.changed(cx);
                    })),
            )
        }
        let mut bitrate = div().flex().gap_2();
        for (id, value, label) in [
            ("bitrate-2500", 2500, "標準"),
            ("bitrate-4500", 4500, "高畫質"),
            ("bitrate-8000", 8000, "極致"),
        ] {
            bitrate = bitrate.child(
                Button::new(id)
                    .label(label)
                    .selected(self.app.settings.video_bitrate == value)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app.settings.video_bitrate = value;
                        this.changed(cx);
                    })),
            )
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(hero)
            .child(audio)
            .child(
                card()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("錄製畫面"))
                            .child(backends),
                    )
                    .child(div().h(px(1.)).bg(rgb(LINE)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(muted("流暢度"))
                            .child(fps),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(muted("畫面品質"))
                            .child(bitrate),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(muted("儲存位置").flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .text_ellipsis()
                            .child(self.app.settings.save_directory.clone()),
                    )
                    .child(
                        Button::new("open-record-folder")
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label("開啟資料夾")
                            .on_click(cx.listener(|this, _, _, _| this.open_folder())),
                    ),
            )
            .when_some(self.app.last_output.clone(), |body, path| {
                body.child(
                    Button::new("play-last")
                        .icon(IconName::Play)
                        .label("播放最後完成的影片")
                        .on_click(move |_, _, _| {
                            let _ = Command::new("xdg-open").arg(&path).spawn();
                        }),
                )
            })
    }
    fn settings(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.app.busy();
        let storage = card()
            .flex()
            .flex_col()
            .gap_4()
            .child(section("儲存與完成後動作", "讓每次錄影依照你的習慣完成。"))
            .child(setting_row(
                "保留 MKV 原檔",
                "MP4 成功儲存後，仍保留一份 MKV 備份。",
                Switch::new("keep-mkv")
                    .accessibility_label("MP4 成功儲存後仍保留 MKV 原檔")
                    .checked(self.app.settings.keep_mkv)
                    .disabled(busy)
                    .color(rgb(ACCENT))
                    .on_change(cx.listener(|this, value, _, cx| {
                        this.app.settings.keep_mkv = *value;
                        this.changed(cx);
                    })),
            ))
            .child(setting_row(
                "完成後自動播放",
                "使用系統預設播放器開啟 MP4。",
                Switch::new("auto-play")
                    .accessibility_label("錄影完成後自動播放")
                    .checked(self.app.settings.auto_play)
                    .disabled(busy)
                    .color(rgb(ACCENT))
                    .on_change(cx.listener(|this, value, _, cx| {
                        this.app.settings.auto_play = *value;
                        this.changed(cx);
                    })),
            ))
            .child(setting_row(
                "完成後開啟資料夾",
                "快速找到剛剛儲存的影片。",
                Switch::new("auto-folder")
                    .accessibility_label("錄影完成後開啟資料夾")
                    .checked(self.app.settings.auto_open_folder)
                    .disabled(busy)
                    .color(rgb(ACCENT))
                    .on_change(cx.listener(|this, value, _, cx| {
                        this.app.settings.auto_open_folder = *value;
                        this.changed(cx);
                    })),
            ))
            .child(
                div()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(muted("影片儲存位置"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.directory).disabled(busy)),
                            )
                            .child(
                                Button::new("choose-folder")
                                    .icon(IconName::FolderOpen)
                                    .disabled(busy || self.picking)
                                    .label("選擇")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick(true, window, cx)
                                    })),
                            ),
                    ),
            );
        let audio = card()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section("音訊平衡", "分別調整背景聲音與麥克風的音量。"))
                    .child(
                        Button::new("reset-audio")
                            .ghost()
                            .small()
                            .label("恢復建議值")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.app.settings.system_volume = 0.6;
                                this.app.settings.mic_volume = 1.5;
                                this.system_volume
                                    .update(cx, |state, cx| state.set_value(0.6, window, cx));
                                this.mic_volume
                                    .update(cx, |state, cx| state.set_value(1.5, window, cx));
                                this.changed(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child("系統音量")
                            .child(muted(format!(
                                "{:.0}%",
                                self.app.settings.system_volume * 100.
                            ))),
                    )
                    .child(Slider::new(&self.system_volume).disabled(busy)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child("麥克風增益")
                            .child(muted(format!(
                                "{:.0}%",
                                self.app.settings.mic_volume * 100.
                            ))),
                    )
                    .child(Slider::new(&self.mic_volume).disabled(busy)),
            );
        let desktop = card()
            .flex()
            .flex_col()
            .gap_4()
            .child(section("快捷鍵與桌面", "錄影時，也能從其他視窗快速操作。"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.shortcut).disabled(busy)),
                    )
                    .child(
                        Button::new("register-shortcut")
                            .label("套用全域快捷鍵")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app.register_gnome_global_shortcut();
                                cx.notify();
                            })),
                    ),
            )
            .child(muted(
                "例如 F9、<Control><Alt>r。程式視窗內也可直接使用 F9。",
            ))
            .child(
                Button::new("install-desktop")
                    .icon(IconName::LayoutDashboard)
                    .label("加入 Ubuntu 應用程式清單")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.install_desktop_entry();
                        cx.notify();
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(storage)
            .child(audio)
            .child(desktop)
            .child(muted("設定會自動儲存。轉換失敗時，一律保留 MKV 原檔。"))
    }
    fn recovery(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.app.busy();
        let picker = card()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .size(px(52.))
                            .rounded(px(14.))
                            .bg(rgb(0xe8f5f5))
                            .text_color(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::FolderOpen).size(px(26.))),
                    )
                    .child(section(
                        "把未完成的錄影接回來",
                        "選擇或拖入 MKV，無損轉成可播放的 MP4。",
                    )),
            )
            .child(Input::new(&self.manual).disabled(busy))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        Button::new("choose-mkv")
                            .icon(IconName::FolderOpen)
                            .label("選擇 MKV 檔案")
                            .disabled(busy || self.picking)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("convert-mkv")
                            .primary()
                            .icon(IconName::ArrowRight)
                            .label("轉換為 MP4")
                            .disabled(busy || self.app.manual_mkv.trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let path = PathBuf::from(this.app.manual_mkv.trim());
                                this.app.start_conversion(path);
                                cx.notify();
                            })),
                    ),
            )
            .child(muted(if self.app.settings.keep_mkv {
                "成功後會保留 MKV 原檔，可於設定變更。"
            } else {
                "成功後會依設定刪除 MKV，轉換失敗則保留。"
            }));
        let mut files = card().flex().flex_col().gap_4().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section(
                    "待救援的錄影",
                    format!(
                        "儲存資料夾中找到 {} 個未轉換檔案",
                        self.app.pending_recordings.len()
                    ),
                ))
                .child(
                    Button::new("refresh-recovery")
                        .ghost()
                        .small()
                        .icon(IconName::RefreshCw)
                        .label("重新掃描")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app.refresh_recovery();
                            cx.notify();
                        })),
                ),
        );
        if self.app.pending_recordings.is_empty() {
            files = files.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .text_color(rgb(MUTED))
                    .child(Icon::new(IconName::CircleCheck).size(px(28.)))
                    .child("目前沒有需要救援的錄影")
                    .child(muted("其他資料夾的 MKV，也可以從上方選擇。")),
            );
        }
        for (index, path) in self.app.pending_recordings.iter().enumerate() {
            let selected = path.clone();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = path
                .metadata()
                .map(|m| format!("{:.1} MB", m.len() as f64 / 1_000_000.))
                .unwrap_or_else(|_| "MKV 錄影".into());
            files = files.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .pt_4()
                    .child(Icon::new(IconName::File).text_color(rgb(ACCENT)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_ellipsis().child(name))
                            .child(muted(size)),
                    )
                    .child(
                        Button::new(("recover", index))
                            .label("救援轉換")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.app.start_conversion(selected.clone());
                                cx.notify();
                            })),
                    ),
            );
        }
        div().flex().flex_col().gap_4().child(picker).child(files)
    }
}
impl Focusable for RecorderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for RecorderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, subtitle) = match self.app.active_tab {
            ActiveTab::Recorder => ("錄下你的每個精彩時刻", "簡單操作，專注分享。"),
            ActiveTab::Recovery => ("錄影救援", "找回中斷的畫面，讓錄影繼續被看見。"),
            ActiveTab::Settings => ("偏好設定", "按照你的習慣，調整每次錄影。"),
        };
        let (phase, color) = self.phase();
        let body = match self.app.active_tab {
            ActiveTab::Recorder => self.recorder(cx),
            ActiveTab::Recovery => self.recovery(cx),
            ActiveTab::Settings => self.settings(cx),
        };
        let important = self.app.status_msg != "就緒"
            || self.app.busy()
            || self.app.lifecycle.phase == Phase::Failed
            || self.app.last_output.is_some();
        let content = div()
            .id("recorder-app")
            .key_context("Recorder")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(Self::close))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.set_manual(path.clone(), window, cx);
                }
            }))
            .size_full()
            .flex()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(INK))
            .text_size(px(14.))
            .font_family("Noto Sans CJK TC")
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .px_8()
                            .pt_7()
                            .pb_6()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(25.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(title),
                                    )
                                    .child(muted(subtitle)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .px_3()
                                    .py_2()
                                    .rounded_full()
                                    .bg(rgb(0xffffff))
                                    .text_size(px(11.))
                                    .text_color(rgb(color))
                                    .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                                    .child(phase),
                            ),
                    )
                    .child(
                        div()
                            .id("page-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_8()
                            .pb_6()
                            .child(body),
                    )
                    .when(important || self.app.lifecycle.close_requested, |body| {
                        body.child(
                            div()
                                .id("status-panel")
                                .max_h(px(130.))
                                .overflow_y_scroll()
                                .border_t_1()
                                .border_color(rgb(LINE))
                                .bg(rgb(0xffffff))
                                .px_8()
                                .py_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(
                                            if self.app.lifecycle.phase == Phase::Failed {
                                                0xc54950
                                            } else {
                                                MUTED
                                            },
                                        ))
                                        .child(self.app.status_msg.clone()),
                                )
                                .when(self.app.lifecycle.phase == Phase::Converting, |status| {
                                    status
                                        .child(
                                            Progress::new("conversion-progress")
                                                .color(rgb(ACCENT))
                                                .value(self.app.conversion_progress * 100.),
                                        )
                                        .child(muted(format!(
                                            "轉換進度 {:.0}%",
                                            self.app.conversion_progress * 100.
                                        )))
                                })
                                .when(self.app.lifecycle.close_requested, |status| {
                                    status.child(muted("儲存完成後將自動關閉視窗。"))
                                }),
                        )
                    })
                    .when_some(self.app.settings_error.clone(), |body, error| {
                        body.child(
                            div()
                                .px_8()
                                .py_3()
                                .bg(rgb(0xfff3d9))
                                .text_size(px(11.))
                                .child(error),
                        )
                    }),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(
                matches!(window.window_decorations(), Decorations::Client { .. }),
                |root| {
                    root.child(
                        TitleBar::new()
                            .bg(rgb(0xffffff))
                            .border_color(rgb(LINE))
                            .child(
                                div()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(
                                        img(app_logo())
                                            .size(px(22.))
                                            .object_fit(ObjectFit::Contain),
                                    )
                                    .child("capra"),
                            )
                            .on_close_window(cx.listener(|this, _, window, cx| {
                                this.close(&CloseRecorder, window, cx)
                            })),
                    )
                },
            )
            .child(content.flex_1().min_h_0())
    }
}
fn card() -> Div {
    div()
        .w_full()
        .p_5()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(LINE))
        .rounded(px(14.))
}
fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text.into())
}
fn section(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(15.))
                .child(title.into()),
        )
        .child(muted(description))
}
fn setting_row(title: &'static str, description: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_5()
        .py_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(title)
                .child(muted(description)),
        )
        .child(control)
}
