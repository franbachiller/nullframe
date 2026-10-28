//! Instalador y desinstalador de Nullframe.
//!
//! - Con payload anexado: instala.
//! - Sin payload (copiado como `uninstall.exe`) o con `--uninstall`: desinstala.
//! - `--pack <salida.exe> <archivos…>`: genera el instalador final.
//! - `--silent [--dir <carpeta>]`: sin interfaz (pruebas y automatización).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod background;
mod payload;
mod system;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke,
    Vec2,
};

use payload::Payload;
use system::{Progress, SharedProgress};

const WINDOW: Vec2 = Vec2::new(660.0, 420.0);
const MARGIN: f32 = 40.0;
const TEXT: Color32 = Color32::from_rgb(242, 242, 242);
const MUTED: Color32 = Color32::from_rgb(128, 128, 132);
const FAINT: Color32 = Color32::from_rgb(52, 52, 56);
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .map(PathBuf::from)
    };

    if args.first().map(String::as_str) == Some("--pack") {
        let files: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
        let result = args
            .get(1)
            .ok_or_else(|| "uso: --pack <salida.exe> <archivos…>".to_owned())
            .and_then(|out| payload::pack(Path::new(out), &files).map_err(|e| e.to_string()));
        exit_with(result);
    }

    let payload = Payload::open_current().ok().flatten();
    let uninstalling = flag("--uninstall") || payload.is_none();
    let mode = if uninstalling {
        Mode::Uninstall {
            dir: uninstall_dir(),
        }
    } else {
        Mode::Install {
            payload: Arc::new(payload.expect("payload")),
        }
    };

    if flag("--silent") {
        let progress = SharedProgress::default();
        let result = match &mode {
            Mode::Install { payload } => {
                let dir = value("--dir").unwrap_or_else(system::default_install_dir);
                system::install(payload, &dir, false, &progress)
            }
            Mode::Uninstall { dir } => system::uninstall(dir, &progress).inspect(|()| {
                if running_from(dir) {
                    system::schedule_folder_removal(dir);
                }
            }),
        };
        exit_with(result);
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(if uninstalling {
            "Desinstalar Nullframe"
        } else {
            "Instalar Nullframe"
        })
        .with_inner_size(WINDOW)
        .with_resizable(false)
        .with_decorations(false)
        .with_maximize_button(false);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "nullframe-setup",
        options,
        Box::new(|cc| Ok(Box::new(Setup::new(cc, mode)))),
    );
}

fn exit_with(result: Result<(), String>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1)
        }
    }
}

/// El desinstalador vive en la carpeta de instalación; si se ejecuta desde
/// otro lugar, se usa la ruta registrada.
fn uninstall_dir() -> PathBuf {
    let own = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    own.filter(|dir| dir.join(system::APP_EXE).exists())
        .or_else(system::installed_dir)
        .unwrap_or_else(system::default_install_dir)
}

fn running_from(dir: &Path) -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent == dir))
        .unwrap_or(false)
}

fn window_icon() -> Option<egui::IconData> {
    let icon = image::load_from_memory(include_bytes!("../../assets/icon.png")).ok()?;
    let rgba = icon.to_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

enum Mode {
    Install { payload: Arc<Payload> },
    Uninstall { dir: PathBuf },
}

#[derive(Clone, PartialEq)]
enum Screen {
    Welcome,
    Working,
    Done,
    Failed(String),
}

struct Setup {
    mode: Mode,
    screen: Screen,
    dir: PathBuf,
    updating: bool,
    desktop_shortcut: bool,
    launch_after: bool,
    progress: SharedProgress,
    started: Instant,
}

impl Setup {
    fn new(cc: &eframe::CreationContext<'_>, mode: Mode) -> Self {
        configure_style(&cc.egui_ctx);
        if let Some(render_state) = &cc.wgpu_render_state {
            background::init(render_state);
        }
        let dir = match &mode {
            Mode::Install { .. } => system::installed_dir().unwrap_or_else(system::default_install_dir),
            Mode::Uninstall { dir } => dir.clone(),
        };
        Self {
            updating: matches!(mode, Mode::Install { .. }) && dir.join(system::APP_EXE).exists(),
            mode,
            screen: Screen::Welcome,
            dir,
            desktop_shortcut: true,
            launch_after: true,
            progress: SharedProgress::default(),
            started: Instant::now(),
        }
    }

    fn installing(&self) -> bool {
        matches!(self.mode, Mode::Install { .. })
    }

    fn start(&mut self) {
        self.progress = Arc::new(Mutex::new(Progress::default()));
        self.screen = Screen::Working;
        let progress = Arc::clone(&self.progress);
        let dir = self.dir.clone();
        match &self.mode {
            Mode::Install { payload } => {
                let payload = Arc::clone(payload);
                let desktop = self.desktop_shortcut;
                thread::spawn(move || {
                    let result = system::install(&payload, &dir, desktop, &progress);
                    progress.lock().unwrap().result = Some(result);
                });
            }
            Mode::Uninstall { .. } => {
                thread::spawn(move || {
                    let result = system::uninstall(&dir, &progress);
                    progress.lock().unwrap().result = Some(result);
                });
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context) {
        match &self.mode {
            Mode::Install { .. } if self.launch_after => system::launch_app(&self.dir),
            Mode::Uninstall { dir } if running_from(dir) => system::schedule_folder_removal(dir),
            _ => {}
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn choose_dir(&mut self) {
        let start = self.dir.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(picked) = rfd::FileDialog::new().set_directory(start).pick_folder() {
            self.dir = if picked.ends_with(system::APP_NAME) {
                picked
            } else {
                picked.join(system::APP_NAME)
            };
            self.updating = self.dir.join(system::APP_EXE).exists();
        }
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.screen == Screen::Working {
            let result = self.progress.lock().unwrap().result.clone();
            match result {
                Some(Ok(())) => self.screen = Screen::Done,
                Some(Err(error)) => self.screen = Screen::Failed(error),
                None => {}
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| {
                let full = ui.max_rect();
                let working = self.screen == Screen::Working;
                let energy = ctx.animate_bool_with_time(egui::Id::new("energy"), working, 0.8);
                background::paint(ui, full, self.started.elapsed().as_secs_f32(), energy);
                paint_scrim(ui.painter(), full);
                ui.painter()
                    .rect_stroke(full, 0.0, Stroke::new(1.0, FAINT), egui::StrokeKind::Inside);

                self.title_bar(ui, full);
                let content = Rect::from_min_max(
                    Pos2::new(full.left() + MARGIN, full.top() + 96.0),
                    Pos2::new(full.right() - MARGIN, full.bottom() - MARGIN),
                );
                match self.screen.clone() {
                    Screen::Welcome => self.welcome(ui, content),
                    Screen::Working => self.working(ui, content),
                    Screen::Done => self.done(ui, content),
                    Screen::Failed(error) => self.failed(ui, content, &error),
                }
            });

        ctx.request_repaint();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

impl Setup {
    fn title_bar(&mut self, ui: &mut egui::Ui, full: Rect) {
        let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), 44.0));
        let drag = ui.interact(bar, egui::Id::new("title_bar"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let painter = ui.painter();
        let wordmark = painter.text(
            Pos2::new(full.left() + MARGIN, bar.center().y),
            Align2::LEFT_CENTER,
            "nullframe",
            FontId::proportional(15.0),
            TEXT,
        );
        painter.text(
            Pos2::new(wordmark.right() + 8.0, bar.center().y + 1.0),
            Align2::LEFT_CENTER,
            if self.installing() { "setup" } else { "uninstall" },
            FontId::monospace(10.5),
            MUTED,
        );

        if self.screen == Screen::Working {
            return;
        }
        let close = Rect::from_center_size(
            Pos2::new(full.right() - 26.0, bar.center().y),
            Vec2::splat(28.0),
        );
        let response = ui.interact(close, egui::Id::new("close"), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(close, 4.0, Color32::from_white_alpha(18));
        }
        let color = if response.hovered() { TEXT } else { MUTED };
        let r = 4.5;
        let c = close.center();
        for (a, b) in [
            (Pos2::new(c.x - r, c.y - r), Pos2::new(c.x + r, c.y + r)),
            (Pos2::new(c.x - r, c.y + r), Pos2::new(c.x + r, c.y - r)),
        ] {
            ui.painter().line_segment([a, b], Stroke::new(1.2, color));
        }
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if response.clicked() || escape {
            if let (Screen::Done, Mode::Uninstall { dir }) = (&self.screen, &self.mode)
                && running_from(dir)
            {
                system::schedule_folder_removal(dir);
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui, content: Rect) {
        let (eyebrow, title, body, action) = if self.installing() {
            (
                format!("INSTALADOR · v{VERSION}"),
                if self.updating {
                    "Actualiza Nullframe."
                } else {
                    "Music into motion."
                },
                "Convierte tu música en visuales generativos para reels, stories y feed. Se instala solo para tu usuario, sin permisos de administrador.",
                if self.updating { "actualizar" } else { "instalar" },
            )
        } else {
            (
                "DESINSTALADOR".to_owned(),
                "Desinstalar Nullframe.",
                "Se eliminarán la app y sus accesos directos. Tus videos exportados no se tocan.",
                "desinstalar",
            )
        };
        heading(ui, content, &eyebrow, title, body);

        let footer_top = content.bottom() - 38.0;
        if self.installing() {
            let options_top = footer_top - 74.0;
            let painter = ui.painter();
            painter.text(
                Pos2::new(content.left(), options_top),
                Align2::LEFT_TOP,
                "destino",
                FontId::proportional(11.0),
                MUTED,
            );
            let path = elide(&self.dir.display().to_string(), 58);
            let path_rect = painter.text(
                Pos2::new(content.left(), options_top + 17.0),
                Align2::LEFT_TOP,
                path,
                FontId::monospace(11.5),
                TEXT,
            );
            let change = text_button(
                ui,
                Pos2::new(path_rect.right() + 12.0, path_rect.center().y),
                "cambiar",
                "change_dir",
            );
            if change {
                self.choose_dir();
            }
            checkbox(
                ui,
                Pos2::new(content.left(), options_top + 48.0),
                &mut self.desktop_shortcut,
                "acceso directo en el escritorio",
                "desktop",
            );
        }

        let primary = primary_button(
            ui,
            Rect::from_min_max(Pos2::new(content.right() - 132.0, footer_top), content.right_bottom()),
            action,
        );
        let cancel = text_button(
            ui,
            Pos2::new(content.right() - 132.0 - 22.0 - 44.0, footer_top + 19.0),
            "cancelar",
            "cancel",
        );
        if primary || ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.start();
        } else if cancel {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn working(&mut self, ui: &mut egui::Ui, content: Rect) {
        let state = self.progress.lock().unwrap().clone();
        let fraction = ui.ctx().animate_value_with_time(
            egui::Id::new("progress"),
            state.fraction,
            0.2,
        );
        let (title, eyebrow) = if self.installing() {
            ("Instalando…", "UN MOMENTO")
        } else {
            ("Desinstalando…", "UN MOMENTO")
        };
        heading(ui, content, eyebrow, title, "");

        let line_y = content.bottom() - 6.0;
        let painter = ui.painter();
        painter.text(
            Pos2::new(content.left(), line_y - 16.0),
            Align2::LEFT_BOTTOM,
            &state.detail,
            FontId::proportional(12.0),
            MUTED,
        );
        painter.text(
            Pos2::new(content.right(), line_y - 16.0),
            Align2::RIGHT_BOTTOM,
            format!("{:>3.0}%", fraction * 100.0),
            FontId::monospace(12.0),
            TEXT,
        );
        let track = Rect::from_min_max(
            Pos2::new(content.left(), line_y - 1.0),
            Pos2::new(content.right(), line_y + 1.0),
        );
        painter.rect_filled(track, 1.0, FAINT);
        let filled = Rect::from_min_max(
            track.min,
            Pos2::new(egui::lerp(track.x_range(), fraction), track.max.y),
        );
        painter.rect_filled(filled, 1.0, TEXT);
    }

    fn done(&mut self, ui: &mut egui::Ui, content: Rect) {
        let footer_top = content.bottom() - 38.0;
        let action = if self.installing() {
            heading(
                ui,
                content,
                "INSTALACIÓN COMPLETA",
                "Listo.",
                "Nullframe ya está en tu menú Inicio. Carga un track y empieza a crear.",
            );
            checkbox(
                ui,
                Pos2::new(content.left(), footer_top + 12.0),
                &mut self.launch_after,
                "abrir Nullframe ahora",
                "launch",
            );
            "finalizar"
        } else {
            heading(
                ui,
                content,
                "DESINSTALACIÓN COMPLETA",
                "Hasta pronto.",
                "Nullframe se ha desinstalado de este equipo.",
            );
            "cerrar"
        };
        let primary = primary_button(
            ui,
            Rect::from_min_max(Pos2::new(content.right() - 132.0, footer_top), content.right_bottom()),
            action,
        );
        if primary || ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            let ctx = ui.ctx().clone();
            self.finish(&ctx);
        }
    }

    fn failed(&mut self, ui: &mut egui::Ui, content: Rect, error: &str) {
        heading(ui, content, "ERROR", "Algo salió mal.", error);
        let footer_top = content.bottom() - 38.0;
        let retry = primary_button(
            ui,
            Rect::from_min_max(Pos2::new(content.right() - 132.0, footer_top), content.right_bottom()),
            "reintentar",
        );
        let close = text_button(
            ui,
            Pos2::new(content.right() - 132.0 - 22.0 - 36.0, footer_top + 19.0),
            "cerrar",
            "close_failed",
        );
        if retry {
            self.screen = Screen::Welcome;
        } else if close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn heading(ui: &mut egui::Ui, content: Rect, eyebrow: &str, title: &str, body: &str) {
    let column = Rect::from_min_size(content.min, Vec2::new(380.0, content.height()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
        ui.label(
            RichText::new(eyebrow)
                .font(FontId::monospace(10.5))
                .color(MUTED),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(title)
                .font(FontId::proportional(34.0))
                .color(TEXT),
        );
        if !body.is_empty() {
            ui.add_space(8.0);
            ui.label(
                RichText::new(body)
                    .font(FontId::proportional(13.5))
                    .color(Color32::from_gray(170)),
            );
        }
    });
}

fn primary_button(ui: &mut egui::Ui, rect: Rect, label: &str) -> bool {
    let response = ui.interact(rect, egui::Id::new(("primary", label)), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let fill = Color32::from_gray((255.0 - hover * 30.0) as u8);
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(13.0),
        Color32::BLACK,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn text_button(ui: &mut egui::Ui, left_center: Pos2, label: &str, id: &str) -> bool {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, MUTED);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - galley.size().y / 2.0),
        galley.size(),
    )
    .expand(4.0);
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let color = if response.hovered() { TEXT } else { MUTED };
    ui.painter().text(
        Pos2::new(left_center.x, left_center.y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        color,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 4.0, rect.bottom() - 3.0),
                Pos2::new(rect.right() - 4.0, rect.bottom() - 3.0),
            ],
            Stroke::new(1.0, Color32::from_white_alpha(60)),
        );
    }
    response.clicked()
}

fn checkbox(ui: &mut egui::Ui, left_top: Pos2, value: &mut bool, label: &str, id: &str) {
    let box_rect = Rect::from_min_size(left_top, Vec2::splat(14.0));
    let text_rect = ui.painter().text(
        Pos2::new(box_rect.right() + 10.0, box_rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        Color32::from_gray(200),
    );
    let response = ui.interact(box_rect.union(text_rect).expand(4.0), egui::Id::new(id), Sense::click());
    if response.clicked() {
        *value = !*value;
    }
    let painter = ui.painter();
    if *value {
        painter.rect_filled(box_rect, 3.0, TEXT);
        let c = box_rect.center();
        painter.line_segment(
            [Pos2::new(c.x - 3.5, c.y + 0.2), Pos2::new(c.x - 1.0, c.y + 2.8)],
            Stroke::new(1.6, Color32::BLACK),
        );
        painter.line_segment(
            [Pos2::new(c.x - 1.0, c.y + 2.8), Pos2::new(c.x + 3.8, c.y - 2.6)],
            Stroke::new(1.6, Color32::BLACK),
        );
    } else {
        let stroke = if response.hovered() { MUTED } else { FAINT };
        painter.rect_stroke(box_rect, 3.0, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Degradado negro de izquierda a derecha para que el texto se lea sobre el fondo animado.
fn paint_scrim(painter: &egui::Painter, rect: Rect) {
    let mut mesh = egui::Mesh::default();
    let left = Color32::from_black_alpha(235);
    let right = Color32::from_black_alpha(70);
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

fn elide(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_owned();
    }
    let keep = max - 1;
    let head: String = chars[..keep / 3].iter().collect();
    let tail: String = chars[chars.len() - (keep - keep / 3)..].iter().collect();
    format!("{head}…{tail}")
}

fn configure_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/InterVariable.ttf")).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::BLACK;
    style.visuals.override_text_color = Some(TEXT);
    style.spacing.item_spacing = Vec2::new(8.0, 4.0);
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
}
