use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Slider, Stroke, TextStyle};
use eframe::wgpu;

use crate::{
    analysis::{AnalysisFrame, AudioAnalyzer},
    audio::{AudioEngine, AudioTrack},
    creator::{CanvasFormat, SnippetRange, VisualControls},
    export::{ExportJob, ExportSettings, ExportStage, locate_ffmpeg, warm_up_encoder_detection},
    gpu,
    tutorial::{Target, Tutorial, TutorialTargets},
};

const PANEL: Color32 = Color32::BLACK;
const TEXT: Color32 = Color32::from_rgb(242, 242, 242);
const MUTED: Color32 = Color32::from_rgb(112, 112, 112);
const FAINT: Color32 = Color32::from_rgb(36, 36, 36);
const ACCENT: Color32 = Color32::WHITE;
const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg"];

pub struct NullframeApp {
    audio_engine: Option<AudioEngine>,
    audio_error: Option<String>,
    track: Option<Arc<AudioTrack>>,
    analyzer: AudioAnalyzer,
    analysis: AnalysisFrame,
    status: Option<String>,
    visual_time: f32,
    visual_kind: gpu::VisualKind,
    canvas_format: CanvasFormat,
    visual_controls: VisualControls,
    snippet: SnippetRange,
    randomize_seed: f32,
    artwork_pixels: Arc<Vec<u8>>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    ffmpeg: Option<PathBuf>,
    export_job: Option<ExportJob>,
    export_message: Option<String>,
    tutorial: Tutorial,
    tutorial_targets: TutorialTargets,
    canvas_rect: egui::Rect,
}

impl NullframeApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        configure_style(&creation_context.egui_ctx);
        let (device, queue) = if let Some(render_state) = &creation_context.wgpu_render_state {
            gpu::init(render_state);
            (
                Some(Arc::new(render_state.device.clone())),
                Some(Arc::new(render_state.queue.clone())),
            )
        } else {
            (None, None)
        };
        let ffmpeg = locate_ffmpeg();
        if let Some(path) = &ffmpeg {
            warm_up_encoder_detection(path.clone());
        }
        let (audio_engine, audio_error) = match AudioEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(error) => (None, Some(error)),
        };

        Self {
            audio_engine,
            audio_error,
            track: None,
            analyzer: AudioAnalyzer::new(),
            analysis: AnalysisFrame::default(),
            status: None,
            visual_time: 0.0,
            visual_kind: gpu::VisualKind::default(),
            canvas_format: CanvasFormat::default(),
            visual_controls: VisualControls::default(),
            snippet: SnippetRange::default(),
            randomize_seed: 1.0,
            artwork_pixels: Arc::new(gpu::create_default_artwork(512, 512)),
            device,
            queue,
            ffmpeg,
            export_job: None,
            export_message: None,
            tutorial: Tutorial::load(),
            tutorial_targets: TutorialTargets::default(),
            canvas_rect: egui::Rect::NOTHING,
        }
    }

    fn choose_artwork(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Imagen", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            match image::open(&path) {
                Ok(img) => {
                    let resized = img.resize_exact(512, 512, image::imageops::FilterType::Lanczos3);
                    let rgba = resized.to_rgba8();
                    self.artwork_pixels = Arc::new(rgba.into_raw());
                    self.status = Some("Carátula cargada".to_owned());
                }
                Err(error) => {
                    self.status = Some(format!("No se pudo cargar la imagen: {error}"));
                }
            }
        }
    }

    fn choose_audio(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", SUPPORTED_EXTENSIONS)
            .pick_file()
        {
            self.load_audio(&path);
        }
    }

    fn load_audio(&mut self, path: &Path) {
        self.status = Some("Analizando audio…".to_owned());
        match AudioTrack::load(path) {
            Ok(track) => {
                if let Some(engine) = &self.audio_engine {
                    engine.set_track(&track);
                }
                self.analyzer.reset();
                self.analysis = AnalysisFrame::default();
                self.snippet.reset(track.duration);
                self.track = Some(Arc::new(track));
                self.export_message = None;
                self.status = None;
            }
            Err(error) => self.status = Some(error),
        }
    }

    fn playback_position(&self) -> f32 {
        self.audio_engine
            .as_ref()
            .map_or(0.0, AudioEngine::position)
    }

    fn is_playing(&self) -> bool {
        self.audio_engine
            .as_ref()
            .is_some_and(AudioEngine::is_playing)
    }

    fn toggle_playback(&mut self) {
        if let Some(engine) = &self.audio_engine {
            if self.track.is_some()
                && (engine.position() < self.snippet.start
                    || engine.position() >= self.snippet.end - 0.02)
            {
                let _ = engine.seek(self.snippet.start);
            }
            engine.toggle_playback();
        }
    }

    fn update_snippet_loop(&mut self) {
        if !self.snippet.loop_enabled || !self.is_playing() {
            return;
        }
        if self.playback_position() >= self.snippet.end
            && let Some(engine) = &self.audio_engine
            && let Err(error) = engine.seek(self.snippet.start)
        {
            self.status = Some(error);
        }
    }

    fn update_analysis(&mut self) {
        if let Some(track) = &self.track {
            self.analysis = self
                .analyzer
                .analyze(track, self.playback_position())
                .clone();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.track.is_some()
            && !self.tutorial.is_active()
            && ctx.input(|input| input.key_pressed(egui::Key::Space))
        {
            self.toggle_playback();
        }
    }

    fn export_in_progress(&self) -> bool {
        self.export_job
            .as_ref()
            .is_some_and(|job| !job.progress().is_finished())
    }

    fn start_export(&mut self) {
        if self.export_in_progress() {
            return;
        }
        let (Some(track), Some(device), Some(queue), Some(ffmpeg)) = (
            self.track.clone(),
            self.device.clone(),
            self.queue.clone(),
            self.ffmpeg.clone(),
        ) else {
            self.export_message =
                Some("Exportación no disponible: falta FFmpeg o la GPU.".to_owned());
            return;
        };

        let default_name = format!(
            "nullframe_{}.mp4",
            track
                .name
                .rsplit_once('.')
                .map_or(track.name.as_str(), |(stem, _)| stem)
        );
        let Some(output) = rfd::FileDialog::new()
            .add_filter("Video MP4", &["mp4"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };

        let settings = ExportSettings {
            track,
            snippet: self.snippet,
            format: self.canvas_format,
            kind: self.visual_kind,
            controls: self.visual_controls,
            artwork_pixels: (*self.artwork_pixels).clone(),
            output,
            ffmpeg,
        };
        self.export_message = None;
        self.export_job = Some(ExportJob::spawn(device, queue, settings));
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("controls")
            .exact_size(244.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(20.0)
                    .stroke(Stroke::new(1.0, FAINT)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("nullframe")
                        .font(FontId::proportional(24.0))
                        .color(TEXT),
                );
                ui.label(RichText::new("music into motion").small().color(MUTED));
                ui.add_space(30.0);

                let load_audio = ui
                    .add_sized(
                        [ui.available_width(), 38.0],
                        egui::Button::new(RichText::new("cargar audio").color(TEXT))
                            .fill(Color32::BLACK)
                            .stroke(Stroke::new(1.0, Color32::from_gray(72))),
                    )
                    .on_hover_text("abrir un archivo WAV, MP3, FLAC u OGG");
                if load_audio.clicked() {
                    self.choose_audio();
                }

                ui.add_space(8.0);
                let load_artwork = ui
                    .add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(RichText::new("cargar carátula").color(TEXT))
                            .fill(Color32::BLACK)
                            .stroke(Stroke::new(1.0, Color32::from_gray(72))),
                    )
                    .on_hover_text("abrir imagen PNG, JPG o WEBP para el vinilo");
                if load_artwork.clicked() {
                    self.choose_artwork();
                }
                self.tutorial_targets
                    .set(Target::LoadAudio, load_audio.rect.union(load_artwork.rect));

                ui.add_space(22.0);
                ui.separator();
                ui.add_space(18.0);

                let visual_top = ui.cursor().min;
                section_label(ui, "visual");
                egui::ComboBox::from_id_salt("visual_selector")
                    .selected_text(self.visual_kind.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for kind in gpu::VisualKind::ALL {
                            ui.selectable_value(&mut self.visual_kind, kind, kind.label());
                        }
                    });
                ui.add_space(12.0);
                section_label(ui, "formato");
                egui::ComboBox::from_id_salt("canvas_format")
                    .selected_text(self.canvas_format.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for format in CanvasFormat::ALL {
                            ui.selectable_value(&mut self.canvas_format, format, format.label());
                        }
                    });
                let [width, height] = self.canvas_format.export_resolution();
                let resolution = ui.label(
                    RichText::new(format!("{width} × {height} · 60 fps"))
                        .small()
                        .color(MUTED),
                );
                self.tutorial_targets.set(
                    Target::VisualFormat,
                    egui::Rect::from_min_max(
                        visual_top,
                        egui::pos2(ui.max_rect().right(), resolution.rect.bottom()),
                    ),
                );
                ui.add_space(22.0);

                if let Some(track) = &self.track {
                    section_label(ui, "archivo");
                    ui.add_space(5.0);
                    ui.label(RichText::new(&track.name).color(TEXT));
                    ui.label(
                        RichText::new(format!(
                            "{:.0} BPM  ·  {} Hz  ·  {} canal{}",
                            track.bpm,
                            track.sample_rate,
                            track.channels,
                            if track.channels == 1 { "" } else { "es" }
                        ))
                        .small()
                        .color(MUTED),
                    );
                    ui.add_space(24.0);
                    meters(ui, &self.analysis);
                } else {
                    section_label(ui, "sin audio");
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("carga un track para comenzar")
                            .color(TEXT)
                            .size(14.0),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("wav · mp3 · flac · ogg").small().color(MUTED));
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    if let Some(error) = &self.audio_error {
                        ui.label(RichText::new(error).small().color(Color32::LIGHT_RED));
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let guide = ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("ver tutorial").small().color(MUTED),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("repetir el recorrido guiado");
                            if guide.clicked() {
                                self.tutorial.restart();
                            }
                        });
                    });
                });
            });
    }

    fn creator_panel(&mut self, ui: &mut egui::Ui) {
        let panel = egui::Panel::right("creator_controls")
            .exact_size(228.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(20.0)
                    .stroke(Stroke::new(1.0, FAINT)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("creator")
                        .font(FontId::proportional(15.0))
                        .color(TEXT),
                );
                ui.label(RichText::new(self.visual_kind.label()).small().color(MUTED));
                ui.add_space(18.0);

                control_slider(
                    ui,
                    "Reacción",
                    &mut self.visual_controls.intensity,
                    0.0..=2.0,
                );
                control_slider(ui, "Velocidad", &mut self.visual_controls.speed, 0.0..=2.5);
                control_slider(
                    ui,
                    "Brillo",
                    &mut self.visual_controls.brightness,
                    0.35..=1.8,
                );
                control_slider(ui, "Paleta", &mut self.visual_controls.hue, 0.0..=1.0);
                control_slider(
                    ui,
                    "Movimiento",
                    &mut self.visual_controls.motion,
                    0.0..=2.0,
                );
                control_slider(ui, "Kick", &mut self.visual_controls.kick, 0.0..=2.5);
                control_slider(ui, "Glow", &mut self.visual_controls.glow, 0.0..=1.5);
                control_slider(
                    ui,
                    self.visual_kind.detail_label(),
                    &mut self.visual_controls.detail,
                    0.0..=1.0,
                );

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("aleatorio").clicked() {
                        self.randomize_seed += 17.31;
                        self.visual_controls.randomize(self.randomize_seed);
                    }
                    if ui.button("reset").clicked() {
                        self.visual_controls.reset();
                    }
                });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    self.export_controls(ui);
                });
            });
        self.tutorial_targets.set(Target::Creator, panel.response.rect);
    }

    fn export_controls(&mut self, ui: &mut egui::Ui) {
        let progress = self.export_job.as_ref().map(|job| job.progress());
        let running = progress
            .as_ref()
            .is_some_and(|progress| !progress.is_finished());

        if let Some(progress) = &progress {
            match progress.stage {
                ExportStage::Rendering | ExportStage::Encoding => {
                    ui.add(
                        egui::ProgressBar::new(progress.fraction())
                            .desired_width(ui.available_width())
                            .fill(ACCENT)
                            .text(
                                RichText::new(format!(
                                    "{} · {}/{}",
                                    progress.message, progress.current_frame, progress.total_frames
                                ))
                                .small()
                                .color(Color32::BLACK),
                            ),
                    );
                }
                ExportStage::Done => {
                    ui.label(RichText::new("exportación lista").small().color(TEXT));
                }
                ExportStage::Failed => {
                    ui.label(
                        RichText::new(&progress.message)
                            .small()
                            .color(Color32::LIGHT_RED),
                    );
                }
            }
        }

        if let Some(message) = &self.export_message {
            ui.label(RichText::new(message).small().color(Color32::LIGHT_RED));
        }

        let can_export = self.track.is_some() && self.ffmpeg.is_some() && !running;
        let button =
            egui::Button::new(RichText::new("exportar mp4").strong().color(Color32::BLACK))
                .fill(if can_export {
                    ACCENT
                } else {
                    Color32::from_gray(40)
                })
                .stroke(Stroke::NONE);
        let export_button = ui
            .add_enabled_ui(can_export, |ui| {
                ui.add_sized([ui.available_width(), 34.0], button)
            })
            .inner;
        self.tutorial_targets.set(Target::Export, export_button.rect);
        if export_button.clicked() {
            self.start_export();
        }

        if self.ffmpeg.is_none() {
            ui.label(
                RichText::new("coloca ffmpeg.exe en tools/ para exportar")
                    .small()
                    .color(MUTED),
            );
        } else {
            ui.label(
                RichText::new("mp4 · 1080p60 · audio original")
                    .small()
                    .color(MUTED),
            );
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let panel = egui::Panel::bottom("transport")
            .exact_size(168.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(16.0)
                    .stroke(Stroke::new(1.0, FAINT)),
            )
            .show(ui, |ui| {
                let Some(track) = self.track.as_ref() else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("selecciona un archivo para comenzar").color(MUTED));
                    });
                    return;
                };

                let duration = track.duration;
                let waveform = track.waveform.clone();
                let mut position = self.playback_position().min(duration);

                ui.horizontal(|ui| {
                    let label = if self.is_playing() {
                        "pausar"
                    } else {
                        "reproducir"
                    };
                    if ui
                        .button(label)
                        .on_hover_text("reproducir o pausar · espacio")
                        .clicked()
                    {
                        self.toggle_playback();
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "{} / {}  ·  snippet {}",
                            format_time(position),
                            format_time(duration),
                            format_time_precise(self.snippet.length()),
                        ))
                        .monospace()
                        .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.snippet.loop_enabled, "loop");
                        if ui.button("inicio").clicked()
                            && let Some(engine) = &self.audio_engine
                            && let Err(error) = engine.seek(self.snippet.start)
                        {
                            self.status = Some(error);
                        }
                        for length in [60.0, 30.0, 15.0] {
                            if ui.button(format!("{length:.0}s")).clicked() {
                                self.snippet.set_length(length, duration);
                            }
                        }
                    });
                });

                ui.add_space(6.0);

                // Interactive waveform: drag edges to move in/out, drag center to slide snippet.
                let waveform_response = paint_waveform(
                    ui,
                    &waveform,
                    position / duration.max(0.001),
                    self.snippet.start / duration.max(0.001),
                    self.snippet.end / duration.max(0.001),
                );
                if let Some(new_pos_norm) = waveform_response.seeked_to {
                    if let Some(engine) = &self.audio_engine
                        && let Err(error) = engine.seek(new_pos_norm * duration)
                    {
                        self.status = Some(error);
                    }
                }
                if let Some(start_norm) = waveform_response.new_start {
                    self.snippet.set_start(start_norm * duration, duration);
                }
                if let Some(end_norm) = waveform_response.new_end {
                    self.snippet.set_end(end_norm * duration, duration);
                }
                if let Some((start_norm, _end_norm)) = waveform_response.moved_range {
                    // Slide the whole snippet keeping its length.
                    let len = self.snippet.length();
                    let new_start = (start_norm * duration).clamp(0.0, (duration - len).max(0.0));
                    self.snippet.start = new_start;
                    self.snippet.end = (new_start + len).min(duration);
                }

                // Playhead slider (fine-grained seeking).
                let response = ui.add(
                    Slider::new(&mut position, 0.0..=duration)
                        .show_value(false)
                        .trailing_fill(true),
                );
                if response.changed()
                    && let Some(engine) = &self.audio_engine
                    && let Err(error) = engine.seek(position)
                {
                    self.status = Some(error);
                }

                // Compact in / out time readout.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("in").small().color(MUTED));
                    ui.label(
                        RichText::new(format_time_precise(self.snippet.start))
                            .monospace()
                            .color(TEXT),
                    );

                    ui.add_space(14.0);
                    ui.label(RichText::new("out").small().color(MUTED));
                    ui.label(
                        RichText::new(format_time_precise(self.snippet.end))
                            .monospace()
                            .color(TEXT),
                    );
                });
            });
        self.tutorial_targets.set(Target::Transport, panel.response.rect);
    }
}

impl eframe::App for NullframeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        self.update_snippet_loop();
        self.update_analysis();
        let frame_delta = ctx.input(|input| input.stable_dt).min(1.0 / 20.0);
        let bpm_factor = self.track.as_ref().map_or(1.0, |t| t.bpm / 125.0);
        let speed_mult = if self.visual_kind == gpu::VisualKind::VinylSpin {
            bpm_factor
        } else {
            1.0
        };
        self.visual_time +=
            frame_delta * self.visual_controls.speed * speed_mult * (0.55 + self.analysis.rms * 0.25);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| {
                self.side_panel(ui);
                self.creator_panel(ui);
                self.transport(ui);
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(Color32::BLACK))
                    .show(ui, |ui| {
                        let available = ui.available_rect_before_wrap();
                        let rect = fit_aspect_rect(available, self.canvas_format.aspect_ratio());
                        self.canvas_rect = rect;
                        ui.allocate_rect(available, egui::Sense::hover());
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.0, Color32::from_rgb(42, 47, 54)),
                            egui::StrokeKind::Inside,
                        );
                        gpu::paint(
                            ui,
                            rect,
                            self.analysis.clone(),
                            self.visual_time,
                            self.visual_kind,
                            self.visual_controls,
                            self.artwork_pixels.clone(),
                        );
                        if let Some(status) = &self.status {
                            egui::Area::new("status".into())
                                .anchor(egui::Align2::CENTER_TOP, [0.0, 18.0])
                                .show(&ctx, |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.label(status);
                                    });
                                });
                        }
                    });
            });

        self.tutorial
            .show(&ctx, &self.tutorial_targets, self.canvas_rect);

        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/InterVariable.ttf")).into(),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "inter".to_owned());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(10.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(11.5, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 7.0);
    style.spacing.slider_width = 118.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::BLACK;
    style.visuals.window_fill = Color32::BLACK;
    style.visuals.extreme_bg_color = Color32::BLACK;
    style.visuals.faint_bg_color = Color32::from_gray(10);
    style.visuals.widgets.noninteractive.bg_fill = Color32::BLACK;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, FAINT);
    style.visuals.widgets.inactive.bg_fill = Color32::BLACK;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(48));
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_gray(174));
    style.visuals.widgets.hovered.bg_fill = Color32::from_gray(14);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(104));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.active.bg_fill = Color32::from_gray(22);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_gray(140));
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.window_stroke = Stroke::new(1.0, FAINT);
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::BLACK);
    style.visuals.hyperlink_color = TEXT;
    style.visuals.override_text_color = Some(TEXT);
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(MUTED));
}

fn meters(ui: &mut egui::Ui, analysis: &AnalysisFrame) {
    section_label(ui, "respuesta");
    for (name, value) in [
        ("graves", analysis.bass),
        ("medios", analysis.mids),
        ("agudos", analysis.treble),
    ] {
        ui.horizontal(|ui| {
            ui.add_sized(
                [56.0, 16.0],
                egui::Label::new(RichText::new(name).small().color(MUTED)),
            );
            ui.add(
                egui::ProgressBar::new((value * 2.2).min(1.0))
                    .desired_width(ui.available_width())
                    .fill(ACCENT),
            );
        });
    }
}

fn control_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) {
    ui.label(RichText::new(label.to_lowercase()).small().color(MUTED));
    ui.add(
        Slider::new(value, range)
            .show_value(true)
            .fixed_decimals(2)
            .trailing_fill(true),
    );
}

fn fit_aspect_rect(available: egui::Rect, aspect_ratio: f32) -> egui::Rect {
    let available_ratio = available.width() / available.height().max(1.0);
    let size = if available_ratio > aspect_ratio {
        egui::vec2(available.height() * aspect_ratio, available.height())
    } else {
        egui::vec2(available.width(), available.width() / aspect_ratio)
    };
    egui::Rect::from_center_size(available.center(), size)
}

/// Output of the interactive waveform widget for a single frame.
struct WaveformResponse {
    /// Normalized position [0,1] to seek to, if the user clicked outside the selection.
    seeked_to: Option<f32>,
    /// New normalized start value when the user drags the in-handle.
    new_start: Option<f32>,
    /// New normalized end value when the user drags the out-handle.
    new_end: Option<f32>,
    /// New (start, end) pair in normalized coords when the user slides the whole snippet.
    moved_range: Option<(f32, f32)>,
}

fn paint_waveform(
    ui: &mut egui::Ui,
    waveform: &[f32],
    progress: f32,
    snippet_start: f32,
    snippet_end: f32,
) -> WaveformResponse {
    // Pixels from a handle boundary that still count as "on the handle".
    const HANDLE_HIT: f32 = 10.0;

    let desired_size = egui::vec2(ui.available_width(), 46.0);
    let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::click_and_drag());
    let rect = response.rect;
    let center_y = rect.center().y;
    let step = rect.width() / waveform.len().max(1) as f32;
    let start_x = egui::lerp(rect.x_range(), snippet_start.clamp(0.0, 1.0));
    let end_x = egui::lerp(rect.x_range(), snippet_end.clamp(0.0, 1.0));

    // --- Determine which zone the pointer is in (for cursor hint) ---
    let pointer_x = response.hover_pos().map(|p| p.x);
    let on_start_handle = pointer_x.is_some_and(|x| (x - start_x).abs() < HANDLE_HIT);
    let on_end_handle = pointer_x.is_some_and(|x| (x - end_x).abs() < HANDLE_HIT);
    let on_center = pointer_x.is_some_and(|x| {
        x > start_x + HANDLE_HIT && x < end_x - HANDLE_HIT
    });

    if on_start_handle || on_end_handle {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if on_center {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    // --- Draw background highlight for the selection ---
    painter.rect_filled(
        egui::Rect::from_x_y_ranges(start_x..=end_x, rect.y_range()),
        2.0,
        Color32::from_rgba_unmultiplied(101, 255, 196, 18),
    );

    // --- Draw waveform bars ---
    for (index, amplitude) in waveform.iter().enumerate() {
        let x = rect.left() + index as f32 * step;
        let height = amplitude.max(0.04) * rect.height() * 0.44;
        let normalized = index as f32 / waveform.len() as f32;
        let color = if normalized <= progress {
            ACCENT
        } else if normalized >= snippet_start && normalized <= snippet_end {
            Color32::from_rgb(82, 108, 101)
        } else {
            Color32::from_rgb(54, 61, 69)
        };
        painter.line_segment(
            [
                egui::pos2(x, center_y - height),
                egui::pos2(x, center_y + height),
            ],
            Stroke::new(step.max(1.0), color),
        );
    }

    // --- Draw in/out handle lines ---
    painter.line_segment(
        [egui::pos2(start_x, rect.top()), egui::pos2(start_x, rect.bottom())],
        Stroke::new(2.0, ACCENT),
    );
    painter.line_segment(
        [egui::pos2(end_x, rect.top()), egui::pos2(end_x, rect.bottom())],
        Stroke::new(2.0, ACCENT),
    );

    // --- Draw handle grip marks (three short horizontal ticks) ---
    for handle_x in [start_x, end_x] {
        for dy in [-5.0_f32, 0.0, 5.0] {
            let y = center_y + dy;
            let side = if handle_x == start_x { 1.0 } else { -1.0 };
            painter.line_segment(
                [egui::pos2(handle_x, y), egui::pos2(handle_x + side * 5.0, y)],
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 255, 255, 140)),
            );
        }
    }

    // --- Draw "IN" / "OUT" labels just outside the handles ---
    let label_color = Color32::from_rgba_unmultiplied(255, 255, 255, 120);
    painter.text(
        egui::pos2(start_x + 4.0, rect.top() + 3.0),
        egui::Align2::LEFT_TOP,
        "IN",
        egui::FontId::monospace(8.5),
        label_color,
    );
    painter.text(
        egui::pos2(end_x - 4.0, rect.top() + 3.0),
        egui::Align2::RIGHT_TOP,
        "OUT",
        egui::FontId::monospace(8.5),
        label_color,
    );

    // --- Interaction logic ---
    let mut out = WaveformResponse {
        seeked_to: None,
        new_start: None,
        new_end: None,
        moved_range: None,
    };

    if response.dragged() || response.clicked() {
        // Identify the drag zone from where the drag started (interact_pointer_pos).
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        if let Some(pos) = pointer {
            let norm = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
            let px = pos.x;

            if (px - start_x).abs() < HANDLE_HIT {
                // Dragging the IN handle.
                out.new_start = Some(norm);
            } else if (px - end_x).abs() < HANDLE_HIT {
                // Dragging the OUT handle.
                out.new_end = Some(norm);
            } else if px > start_x + HANDLE_HIT && px < end_x - HANDLE_HIT {
                if response.dragged() {
                    // Sliding the whole selection: compute delta from last frame.
                    let delta_norm = response.drag_delta().x / rect.width().max(1.0);
                    let new_start = (snippet_start + delta_norm).clamp(0.0, 1.0);
                    let length = snippet_end - snippet_start;
                    let new_end = (new_start + length).clamp(0.0, 1.0);
                    out.moved_range = Some((new_start, new_end));
                }
            } else if response.clicked() {
                // Click outside the selection → seek the playhead.
                out.seeked_to = Some(norm);
            }
        }
    }

    out
}

fn format_time(seconds: f32) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn format_time_precise(seconds: f32) -> String {
    let seconds = seconds.max(0.0);
    let minutes = (seconds / 60.0).floor() as u64;
    format!("{minutes:02}:{:04.1}", seconds % 60.0)
}

#[cfg(test)]
mod tests {
    use super::{fit_aspect_rect, format_time, format_time_precise};

    #[test]
    fn formats_transport_time() {
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(65.2), "01:05");
        assert_eq!(format_time(3599.8), "60:00");
    }

    #[test]
    fn formats_precise_snippet_time() {
        assert_eq!(format_time_precise(65.25), "01:05.2");
    }

    #[test]
    fn fits_portrait_canvas_inside_available_area() {
        let available = eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1000.0, 500.0),
        );
        let result = fit_aspect_rect(available, 9.0 / 16.0);
        assert!((result.height() - 500.0).abs() < 0.01);
        assert!((result.width() - 281.25).abs() < 0.01);
    }
}
