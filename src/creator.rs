#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasFormat {
    #[default]
    Landscape,
    Portrait,
    Square,
    Feed,
}

impl CanvasFormat {
    pub const ALL: [Self; 4] = [Self::Landscape, Self::Portrait, Self::Square, Self::Feed];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Landscape => "16:9 · Horizontal",
            Self::Portrait => "9:16 · Reels / Stories",
            Self::Square => "1:1 · Cuadrado",
            Self::Feed => "4:5 · Feed",
        }
    }

    pub const fn aspect_ratio(self) -> f32 {
        match self {
            Self::Landscape => 16.0 / 9.0,
            Self::Portrait => 9.0 / 16.0,
            Self::Square => 1.0,
            Self::Feed => 4.0 / 5.0,
        }
    }

    pub const fn export_resolution(self) -> [u32; 2] {
        match self {
            Self::Landscape => [1920, 1080],
            Self::Portrait => [1080, 1920],
            Self::Square => [1080, 1080],
            Self::Feed => [1080, 1350],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VisualControls {
    pub intensity: f32,
    pub speed: f32,
    pub brightness: f32,
    pub hue: f32,
    pub motion: f32,
    pub kick: f32,
    pub glow: f32,
    pub detail: f32,
}

impl Default for VisualControls {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            speed: 1.0,
            brightness: 1.0,
            hue: 0.5,
            motion: 1.0,
            kick: 1.0,
            glow: 0.65,
            detail: 0.55,
        }
    }
}

impl VisualControls {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn randomize(&mut self, seed: f32) {
        self.intensity = 0.65 + hash(seed + 1.0) * 0.85;
        self.speed = 0.45 + hash(seed + 2.0) * 1.45;
        self.brightness = 0.72 + hash(seed + 3.0) * 0.58;
        self.hue = hash(seed + 4.0);
        self.motion = 0.45 + hash(seed + 5.0) * 1.35;
        self.kick = 0.45 + hash(seed + 6.0) * 1.55;
        self.glow = 0.25 + hash(seed + 7.0) * 0.75;
        self.detail = 0.20 + hash(seed + 8.0) * 0.80;
    }

    pub fn uniform_values(self) -> [[f32; 4]; 2] {
        [
            [self.intensity, self.speed, self.brightness, self.hue],
            [self.motion, self.kick, self.glow, self.detail],
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SnippetRange {
    pub start: f32,
    pub end: f32,
    pub loop_enabled: bool,
}

impl Default for SnippetRange {
    fn default() -> Self {
        Self {
            start: 0.0,
            end: 0.0,
            loop_enabled: true,
        }
    }
}

impl SnippetRange {
    pub const MIN_LENGTH: f32 = 0.25;

    pub fn reset(&mut self, track_duration: f32) {
        self.start = 0.0;
        self.end = track_duration.min(15.0);
        self.loop_enabled = true;
    }

    pub fn length(self) -> f32 {
        (self.end - self.start).max(0.0)
    }

    pub fn set_length(&mut self, length: f32, track_duration: f32) {
        let requested = length.clamp(Self::MIN_LENGTH, track_duration.max(Self::MIN_LENGTH));
        self.end = (self.start + requested).min(track_duration);
        self.start = (self.end - requested).max(0.0);
    }

    pub fn set_start(&mut self, start: f32, track_duration: f32) {
        self.start = start.clamp(0.0, (self.end - Self::MIN_LENGTH).max(0.0));
        self.clamp(track_duration);
    }

    pub fn set_end(&mut self, end: f32, track_duration: f32) {
        self.end = end.clamp(
            (self.start + Self::MIN_LENGTH).min(track_duration),
            track_duration,
        );
        self.clamp(track_duration);
    }

    pub fn clamp(&mut self, track_duration: f32) {
        self.start = self.start.clamp(0.0, track_duration);
        self.end = self.end.clamp(self.start, track_duration);
        if track_duration >= Self::MIN_LENGTH && self.length() < Self::MIN_LENGTH {
            self.end = (self.start + Self::MIN_LENGTH).min(track_duration);
            self.start = (self.end - Self::MIN_LENGTH).max(0.0);
        }
    }
}

fn hash(value: f32) -> f32 {
    (value.sin() * 43_758.547).fract().abs()
}

#[cfg(test)]
mod tests {
    use super::{CanvasFormat, SnippetRange, VisualControls};

    #[test]
    fn format_dimensions_match_aspect_ratio() {
        for format in CanvasFormat::ALL {
            let [width, height] = format.export_resolution();
            let ratio = width as f32 / height as f32;
            assert!((ratio - format.aspect_ratio()).abs() < 0.001);
        }
    }

    #[test]
    fn snippet_length_stays_inside_track() {
        let mut snippet = SnippetRange {
            start: 95.0,
            end: 100.0,
            loop_enabled: true,
        };
        snippet.set_length(15.0, 100.0);
        assert_eq!(snippet.start, 85.0);
        assert_eq!(snippet.end, 100.0);
    }

    #[test]
    fn randomized_controls_stay_in_supported_ranges() {
        let mut controls = VisualControls::default();
        controls.randomize(42.0);
        assert!((0.5..=1.5).contains(&controls.intensity));
        assert!((0.25..=2.0).contains(&controls.speed));
        assert!((0.5..=1.5).contains(&controls.brightness));
        assert!((0.0..=1.0).contains(&controls.hue));
    }
}
