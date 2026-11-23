//! egui window + tray icon.

use crate::conn::{lock, Shared};
use crate::settings::Backend;
use crate::{autostart, vcam};
use eframe::egui::{self, Color32, ViewportCommand};
use protocol::Control;
use std::sync::{mpsc, Arc};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

enum TrayMsg {
    Show,
    Quit,
}

pub struct App {
    shared: Arc<Shared>,
    texture: Option<egui::TextureHandle>,
    shown_seq: Option<u64>,
    rgba: Vec<u8>,
    _tray: Option<TrayIcon>,
    tray_rx: mpsc::Receiver<TrayMsg>,
    quitting: bool,
    // Local control state mirrored to the phone.
    torch: bool,
    ev: i8,
    zoom: f32,
    mic: bool,
    audio_devices: Vec<String>,
}

/// A simple generated icon: blue disc with a white lens.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let mut v = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let px = if d < c * 0.35 {
                [255, 255, 255, 255]
            } else if d < c * 0.95 {
                [30, 110, 230, 255]
            } else {
                [0, 0, 0, 0]
            };
            v.extend_from_slice(&px);
        }
    }
    v
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>) -> App {
        *lock(&shared.repaint) = Some(cc.egui_ctx.clone());
        let (tx, tray_rx) = mpsc::channel();
        let tray = build_tray(&cc.egui_ctx, tx);
        App {
            shared,
            texture: None,
            shown_seq: None,
            rgba: Vec::new(),
            _tray: tray,
            tray_rx,
            quitting: false,
            torch: false,
            ev: 0,
            zoom: 1.0,
            mic: true,
            audio_devices: crate::audio::render_devices(),
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let frame = lock(&self.shared.preview).clone();
        let Some((seq, f)) = frame else { return };
        if self.shown_seq == Some(seq) {
            return;
        }
        self.shown_seq = Some(seq);
        let small = if f.width > 960 {
            f.scaled(f.width / 2, f.height / 2)
        } else {
            (*f).clone()
        };
        small.to_rgba(&mut self.rgba);
        let img = egui::ColorImage::from_rgba_unmultiplied([small.width, small.height], &self.rgba);
        match &mut self.texture {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR))
            }
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let st = lock(&self.shared.state).clone();
        let connected = st.connected.is_some();
        ui.heading("FreeCam Phone");
        ui.label(&st.status);
        if connected {
            ui.label(format!(
                "Latency {:.0} ms · {:.0} fps · {}x{}",
                st.latency_ms, st.fps, st.video_size.0, st.video_size.1
            ));
            if let Some(s) = &st.stats {
                ui.label(format!("Phone: {:.0} fps, {} kbps", s.fps, s.bitrate_kbps));
            }
        }
        ui.separator();

        ui.strong("Devices");
        let mut device = lock(&self.shared.settings).device.clone();
        let before = device.clone();
        ui.radio_value(&mut device, String::new(), "Automatic (USB preferred)");
        for serial in &st.usb {
            ui.radio_value(
                &mut device,
                format!("usb:{serial}"),
                format!("USB: {serial}"),
            );
        }
        for name in st.wifi.keys() {
            ui.radio_value(
                &mut device,
                format!("wifi:{name}"),
                format!("Wi-Fi: {name}"),
            );
        }
        if st.usb.is_empty() && st.wifi.is_empty() {
            ui.weak("No phones found yet. Open FreeCam Phone on the phone.");
        }
        if device != before {
            self.save(|s| s.device = device);
            self.shared.drop_session();
        }
        ui.horizontal(|ui| {
            if st.enabled {
                if ui.button("Disconnect").clicked() {
                    lock(&self.shared.state).enabled = false;
                    self.shared.drop_session();
                }
            } else if ui.button("Connect").clicked() {
                lock(&self.shared.state).enabled = true;
            }
        });
        ui.separator();

        ui.strong("Camera");
        ui.add_enabled_ui(connected, |ui| self.controls(ui, &st));
        ui.separator();

        ui.strong("Settings");
        self.settings_ui(ui);
    }

    fn controls(&mut self, ui: &mut egui::Ui, st: &crate::conn::State) {
        let sh = self.shared.clone();
        if ui.button("Switch camera").clicked() {
            sh.send(Control::SwitchCamera);
        }
        let (mut res, mut fps) = {
            let s = lock(&sh.settings);
            (s.resolution, s.fps)
        };
        let (r0, f0) = (res, fps);
        egui::ComboBox::from_label("Resolution")
            .selected_text(format!("{res}p"))
            .show_ui(ui, |ui| {
                for r in [480u16, 720, 1080] {
                    ui.selectable_value(&mut res, r, format!("{r}p"));
                }
            });
        egui::ComboBox::from_label("Frame rate")
            .selected_text(format!("{fps} fps"))
            .show_ui(ui, |ui| {
                for f in [30u8, 60] {
                    ui.selectable_value(&mut fps, f, format!("{f} fps"));
                }
            });
        if res != r0 {
            sh.send(Control::Resolution(res));
            self.save(|s| s.resolution = res);
        }
        if fps != f0 {
            sh.send(Control::Fps(fps));
            self.save(|s| s.fps = fps);
        }
        let stats = st.stats.clone();
        if stats.as_ref().is_none_or(|s| s.has_torch)
            && ui.checkbox(&mut self.torch, "Torch").changed()
        {
            sh.send(Control::Torch(self.torch));
        }
        let (ev_min, ev_max, zmax) = stats
            .as_ref()
            .map_or((-6, 6, 4.0), |s| (s.ev_min, s.ev_max, s.zoom_max.max(1.0)));
        if ui
            .add(egui::Slider::new(&mut self.ev, ev_min..=ev_max).text("Exposure"))
            .changed()
        {
            sh.send(Control::Exposure(self.ev));
        }
        if ui
            .add(
                egui::Slider::new(&mut self.zoom, 1.0..=zmax)
                    .text("Zoom")
                    .fixed_decimals(1),
            )
            .changed()
        {
            sh.send(Control::Zoom(self.zoom));
        }
        if ui.checkbox(&mut self.mic, "Phone microphone").changed() {
            sh.send(Control::Mic(self.mic));
        }
        ui.weak("Click the preview to focus.");
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut s = lock(&self.shared.settings).clone();
        let before = format!("{s:?}");
        ui.checkbox(&mut s.mirror, "Mirror video");
        ui.checkbox(&mut s.start_with_windows, "Start with Windows");
        egui::ComboBox::from_label("Virtual camera")
            .selected_text(format!("{:?}", s.backend))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut s.backend, Backend::Auto, "Auto");
                ui.selectable_value(
                    &mut s.backend,
                    Backend::MediaFoundation,
                    "Media Foundation (Win 11)",
                );
                ui.selectable_value(&mut s.backend, Backend::DirectShow, "DirectShow");
                ui.selectable_value(&mut s.backend, Backend::Both, "Both");
            });
        ui.small(lock(&self.shared.outputs).status.clone());
        egui::ComboBox::from_label("Phone audio to")
            .selected_text(if s.audio_device.is_empty() {
                "Off".to_string()
            } else {
                s.audio_device.clone()
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut s.audio_device, String::new(), "Off");
                for d in &self.audio_devices {
                    ui.selectable_value(&mut s.audio_device, d.clone(), d);
                }
            });
        ui.horizontal(|ui| {
            ui.label("adb path");
            ui.text_edit_singleline(&mut s.adb_path)
                .on_hover_text("Leave empty to auto-detect");
        });
        if format!("{s:?}") == before {
            return;
        }
        let old = lock(&self.shared.settings).clone();
        if s.backend != old.backend {
            // Drop the old outputs first so their names/devices are released.
            *lock(&self.shared.outputs) = vcam::Outputs::none();
            *lock(&self.shared.outputs) = vcam::Outputs::new(s.backend);
        }
        if s.audio_device != old.audio_device {
            *lock(&self.shared.audio) = crate::audio::Sink::new(&s.audio_device);
        }
        if s.start_with_windows != old.start_with_windows {
            autostart::set(s.start_with_windows);
        }
        *lock(&self.shared.settings) = s.clone();
        s.save();
    }

    fn save(&self, f: impl FnOnce(&mut crate::settings::Settings)) {
        let mut s = lock(&self.shared.settings);
        f(&mut s);
        s.save();
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let Some(tex) = &self.texture else {
            ui.centered_and_justified(|ui| ui.label("Waiting for phone video…"));
            return;
        };
        let avail = ui.available_size();
        let ts = tex.size_vec2();
        let scale = (avail.x / ts.x).min(avail.y / ts.y);
        let resp = ui
            .centered_and_justified(|ui| {
                ui.add(egui::Image::new((tex.id(), ts * scale)).sense(egui::Sense::click()))
            })
            .inner;
        if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.clicked()) {
            let r = resp.rect;
            let (mut x, mut y) = (
                (pos.x - r.left()) / r.width(),
                (pos.y - r.top()) / r.height(),
            );
            // Undo the display transforms to get back to the phone's image space.
            if lock(&self.shared.settings).mirror {
                x = 1.0 - x;
            }
            if lock(&self.shared.state)
                .stats
                .as_ref()
                .is_some_and(|s| s.facing == 1)
            {
                x = 1.0 - x;
                y = 1.0 - y;
            }
            self.shared.send(Control::Focus { x, y });
            ui.painter()
                .circle_stroke(pos, 20.0, (2.0, Color32::YELLOW));
        }
    }
}

fn build_tray(ctx: &egui::Context, tx: mpsc::Sender<TrayMsg>) -> Option<TrayIcon> {
    let menu = Menu::new();
    let show = MenuItem::new("Show FreeCam Phone", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&show).ok()?;
    menu.append(&quit).ok()?;
    let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
    let (c1, t1) = (ctx.clone(), tx.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let msg = if e.id == quit_id {
            TrayMsg::Quit
        } else if e.id == show_id {
            TrayMsg::Show
        } else {
            return;
        };
        let _ = t1.send(msg);
        c1.request_repaint();
    }));
    let c2 = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::DoubleClick { .. } = e {
            let _ = tx.send(TrayMsg::Show);
            c2.request_repaint();
        }
    }));
    let icon = tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("FreeCam Phone")
        .with_icon(icon)
        .build()
        .ok()
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(m) = self.tray_rx.try_recv() {
            match m {
                TrayMsg::Show => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                TrayMsg::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        // Closing the window hides it to the tray; Quit from the tray exits.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self._tray.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.update_texture(&ctx);
        egui::Panel::left("side")
            .resizable(false)
            .exact_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| self.preview(ui));
    }
}
