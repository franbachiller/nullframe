//! Recorrido guiado que se muestra la primera vez que se abre Nullframe.
//!
//! La interfaz registra cada frame el rectángulo de las zonas que el
//! tutorial señala (`TutorialTargets`); el overlay atenúa todo lo demás y
//! coloca una tarjeta junto a la zona activa.

use std::path::PathBuf;

use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    LoadAudio,
    VisualFormat,
    Creator,
    Transport,
    Export,
}

/// Zonas de la interfaz registradas durante el layout del frame actual.
#[derive(Default)]
pub struct TutorialTargets {
    rects: Vec<(Target, Rect)>,
}

impl TutorialTargets {
    pub fn set(&mut self, target: Target, rect: Rect) {
        self.rects.retain(|(existing, _)| *existing != target);
        self.rects.push((target, rect));
    }

    fn get(&self, target: Target) -> Option<Rect> {
        self.rects
            .iter()
            .find(|(existing, _)| *existing == target)
            .map(|(_, rect)| *rect)
    }
}

struct Step {
    target: Option<Target>,
    title: &'static str,
    body: &'static str,
}

const STEPS: &[Step] = &[
    Step {
        target: None,
        title: "bienvenido a nullframe",
        body: "Convierte tu música en visuales para redes. Te mostramos lo esencial en menos de un minuto.",
    },
    Step {
        target: Some(Target::LoadAudio),
        title: "carga tu track",
        body: "WAV, MP3, FLAC u OGG. Nullframe analiza graves, medios, agudos y el BPM al instante. Opcional: sube una carátula para el vinilo.",
    },
    Step {
        target: Some(Target::VisualFormat),
        title: "visual y formato",
        body: "Elige la visual y el formato de destino: 9:16 para reels y stories, 1:1 o 4:5 para el feed. La preview muestra el encuadre real.",
    },
    Step {
        target: Some(Target::Creator),
        title: "ajústalo a tu estilo",
        body: "Cada control reacciona al audio en tiempo real. Usa «aleatorio» para explorar y «reset» para volver al punto de partida.",
    },
    Step {
        target: Some(Target::Transport),
        title: "elige el momento",
        body: "Arrastra los bordes IN / OUT o desliza la selección completa. Presets de 15, 30 y 60 s. Espacio reproduce o pausa.",
    },
    Step {
        target: Some(Target::Export),
        title: "exporta",
        body: "MP4 1080p a 60 fps con el audio original de tu master, listo para subir.",
    },
];

const CARD_WIDTH: f32 = 300.0;
const SPOT_PADDING: f32 = 6.0;
const BACKDROP: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 190);

pub struct Tutorial {
    step: Option<usize>,
}

impl Tutorial {
    /// Arranca activo solo si el usuario todavía no lo ha completado u omitido.
    pub fn load() -> Self {
        let seen = marker_path().is_some_and(|path| path.exists());
        Self {
            step: (!seen).then_some(0),
        }
    }

    pub fn is_active(&self) -> bool {
        self.step.is_some()
    }

    pub fn restart(&mut self) {
        self.step = Some(0);
    }

    fn finish(&mut self) {
        self.step = None;
        if let Some(path) = marker_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, b"1");
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, targets: &TutorialTargets, canvas: Rect) {
        let Some(index) = self.step else {
            return;
        };
        let step = &STEPS[index];
        let screen = ctx.content_rect();
        // Fundido suave al cambiar de paso.
        let fade = ctx.animate_bool_with_time(egui::Id::new(("tutorial_step", index)), true, 0.25);
        let spot = step
            .target
            .and_then(|target| targets.get(target))
            .map(|rect| rect.expand(SPOT_PADDING).intersect(screen));

        let mut action = None;
        egui::Area::new(egui::Id::new("tutorial_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // Capa que bloquea la interacción con la interfaz de fondo.
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
                let painter = ui.painter();
                paint_backdrop(painter, screen, spot);
                if let Some(spot) = spot {
                    painter.rect_stroke(
                        spot,
                        6.0,
                        Stroke::new(1.0, Color32::from_white_alpha((200.0 * fade) as u8)),
                        egui::StrokeKind::Outside,
                    );
                }

                let card_pos = card_position(spot, canvas, screen);
                let offset = Vec2::new(0.0, (1.0 - fade) * 8.0);
                egui::Area::new(egui::Id::new("tutorial_card"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(card_pos + offset)
                    .show(ctx, |ui| {
                        ui.set_opacity(fade);
                        action = card(ui, index, step);
                    });
            });

        ctx.input(|input| {
            if input.key_pressed(egui::Key::Escape) {
                action = Some(Action::Skip);
            } else if input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::ArrowRight) {
                action = Some(Action::Next);
            } else if input.key_pressed(egui::Key::ArrowLeft) {
                action = Some(Action::Back);
            }
        });

        match action {
            Some(Action::Next) if index + 1 < STEPS.len() => self.step = Some(index + 1),
            Some(Action::Next | Action::Skip) => self.finish(),
            Some(Action::Back) => self.step = Some(index.saturating_sub(1)),
            None => {}
        }
    }
}

enum Action {
    Next,
    Back,
    Skip,
}

fn card(ui: &mut egui::Ui, index: usize, step: &Step) -> Option<Action> {
    let mut action = None;
    let last = index + 1 == STEPS.len();
    egui::Frame::new()
        .fill(Color32::from_gray(8))
        .stroke(Stroke::new(1.0, Color32::from_gray(52)))
        .corner_radius(8.0)
        .inner_margin(20.0)
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(200),
        })
        .show(ui, |ui| {
            ui.set_width(CARD_WIDTH - 40.0);
            ui.label(
                RichText::new(format!("{:02} / {:02}", index + 1, STEPS.len()))
                    .font(FontId::monospace(10.0))
                    .color(Color32::from_gray(110)),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(step.title)
                    .font(FontId::proportional(17.0))
                    .color(Color32::WHITE),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(step.body)
                    .size(12.5)
                    .color(Color32::from_gray(170)),
            );
            ui.add_space(16.0);
            progress_line(ui, index);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if !last && link(ui, "omitir").clicked() {
                    action = Some(Action::Skip);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match index {
                        0 => "comenzar",
                        _ if last => "empezar a crear",
                        _ => "siguiente",
                    };
                    let next = egui::Button::new(
                        RichText::new(label).strong().color(Color32::BLACK),
                    )
                    .fill(Color32::WHITE)
                    .corner_radius(4.0);
                    if ui.add(next).clicked() {
                        action = Some(Action::Next);
                    }
                    if index > 0 && link(ui, "atrás").clicked() {
                        action = Some(Action::Back);
                    }
                });
            });
        });
    action
}

fn link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(11.5).color(Color32::from_gray(120)))
            .frame(false),
    )
}

/// Barra segmentada que indica el avance del recorrido.
fn progress_line(ui: &mut egui::Ui, index: usize) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), egui::Sense::hover());
    let gap = 4.0;
    let segment = (rect.width() - gap * (STEPS.len() - 1) as f32) / STEPS.len() as f32;
    for i in 0..STEPS.len() {
        let x = rect.left() + i as f32 * (segment + gap);
        let color = if i <= index {
            Color32::WHITE
        } else {
            Color32::from_gray(40)
        };
        ui.painter().rect_filled(
            Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(segment, rect.height())),
            1.0,
            color,
        );
    }
}

/// Oscurece la pantalla dejando un hueco nítido sobre la zona resaltada.
fn paint_backdrop(painter: &egui::Painter, screen: Rect, spot: Option<Rect>) {
    let Some(spot) = spot else {
        painter.rect_filled(screen, 0.0, BACKDROP);
        return;
    };
    let bands = [
        Rect::from_min_max(screen.min, Pos2::new(screen.right(), spot.top())),
        Rect::from_min_max(Pos2::new(screen.left(), spot.bottom()), screen.max),
        Rect::from_min_max(
            Pos2::new(screen.left(), spot.top()),
            Pos2::new(spot.left(), spot.bottom()),
        ),
        Rect::from_min_max(
            Pos2::new(spot.right(), spot.top()),
            Pos2::new(screen.right(), spot.bottom()),
        ),
    ];
    for band in bands {
        if band.is_positive() {
            painter.rect_filled(band, 0.0, BACKDROP);
        }
    }
}

/// Coloca la tarjeta al lado con más espacio libre de la zona resaltada.
fn card_position(spot: Option<Rect>, canvas: Rect, screen: Rect) -> Pos2 {
    const ESTIMATED_HEIGHT: f32 = 210.0;
    const GAP: f32 = 18.0;
    let size = Vec2::new(CARD_WIDTH, ESTIMATED_HEIGHT);
    let Some(spot) = spot else {
        return canvas.center() - size / 2.0;
    };

    let candidates = [
        (spot.right() + GAP, spot.top()),
        (spot.left() - GAP - size.x, spot.top()),
        (spot.left(), spot.top() - GAP - size.y),
        (spot.left(), spot.bottom() + GAP),
    ];
    let fits = |(x, y): (f32, f32)| {
        screen.shrink(8.0).contains_rect(Rect::from_min_size(Pos2::new(x, y), size))
    };
    let (x, y) = candidates
        .into_iter()
        .find(|&candidate| fits(candidate))
        .unwrap_or((canvas.center().x - size.x / 2.0, canvas.center().y - size.y / 2.0));
    let max = screen.max - size - Vec2::splat(8.0);
    Pos2::new(x.clamp(8.0, max.x.max(8.0)), y.clamp(8.0, max.y.max(8.0)))
}

/// `%APPDATA%\Nullframe\tutorial_visto` (o `~/.config/nullframe/...`).
fn marker_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(|dir| PathBuf::from(dir).join("Nullframe"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/nullframe"))
        })?;
    Some(base.join("tutorial_visto"))
}
