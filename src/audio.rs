use std::{
    fs::File,
    num::{NonZeroU16, NonZeroU32},
    path::Path,
    sync::Arc,
    time::Duration,
};

use rodio::{
    Decoder, DeviceSinkBuilder, Player, Source, buffer::SamplesBuffer, stream::MixerDeviceSink,
};

pub struct AudioTrack {
    pub name: String,
    pub samples: Arc<[f32]>,
    pub waveform: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
    pub duration: f32,
    pub bpm: f32,
}

impl AudioTrack {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|error| format!("No se pudo abrir el archivo: {error}"))?;
        let decoder = Decoder::try_from(file)
            .map_err(|error| format!("Formato de audio no compatible: {error}"))?;
        let channels = decoder.channels().get();
        let sample_rate = decoder.sample_rate().get();
        let samples: Vec<f32> = decoder.collect();

        if samples.is_empty() {
            return Err("El archivo no contiene muestras de audio.".to_owned());
        }

        let frame_count = samples.len() / usize::from(channels);
        let duration = frame_count as f32 / sample_rate as f32;
        let waveform = build_waveform(&samples, usize::from(channels), 720);
        let bpm = detect_bpm(&samples, usize::from(channels), sample_rate);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Audio")
            .to_owned();

        Ok(Self {
            name,
            samples: samples.into(),
            waveform,
            channels,
            sample_rate,
            duration,
            bpm,
        })
    }
}

pub struct AudioEngine {
    player: Player,
    _device: MixerDeviceSink,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let device = DeviceSinkBuilder::open_default_sink()
            .map_err(|error| format!("No se pudo abrir la salida de audio: {error}"))?;
        let player = Player::connect_new(device.mixer());
        Ok(Self {
            player,
            _device: device,
        })
    }

    pub fn set_track(&self, track: &AudioTrack) {
        self.player.clear();
        let source = SamplesBuffer::new(
            NonZeroU16::new(track.channels).expect("audio channels are non-zero"),
            NonZeroU32::new(track.sample_rate).expect("sample rate is non-zero"),
            track.samples.to_vec(),
        );
        self.player.append(source);
        self.player.pause();
    }

    pub fn toggle_playback(&self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn is_playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    pub fn position(&self) -> f32 {
        self.player.get_pos().as_secs_f32()
    }

    pub fn seek(&self, seconds: f32) -> Result<(), String> {
        self.player
            .try_seek(Duration::from_secs_f32(seconds.max(0.0)))
            .map_err(|error| format!("No se pudo mover la reproducción: {error}"))
    }
}

fn build_waveform(samples: &[f32], channels: usize, point_count: usize) -> Vec<f32> {
    let frame_count = samples.len() / channels;
    let frames_per_point = (frame_count / point_count).max(1);

    (0..point_count)
        .map(|point| {
            let start_frame = point * frames_per_point;
            let end_frame = ((point + 1) * frames_per_point).min(frame_count);
            if start_frame >= end_frame {
                return 0.0;
            }

            let peak = (start_frame..end_frame)
                .flat_map(|frame| {
                    let start = frame * channels;
                    &samples[start..start + channels]
                })
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max);
            peak.min(1.0)
        })
        .collect()
}

pub fn detect_bpm(samples: &[f32], channels: usize, sample_rate: u32) -> f32 {
    let frame_count = samples.len() / channels;
    if frame_count < (sample_rate as usize * 2) {
        return 125.0;
    }

    let hop_size = (sample_rate as usize / 100).max(1);
    let total_hops = frame_count / hop_size;
    let mut energy = Vec::with_capacity(total_hops);

    for hop in 0..total_hops {
        let start = hop * hop_size * channels;
        let end = ((hop + 1) * hop_size * channels).min(samples.len());
        let sum_sq: f32 = samples[start..end].iter().map(|s| s * s).sum();
        let rms = (sum_sq / (end - start) as f32).sqrt();
        energy.push(rms);
    }

    let mut onset = vec![0.0_f32; total_hops];
    for i in 1..total_hops {
        let diff = energy[i] - energy[i - 1];
        if diff > 0.0 {
            onset[i] = diff;
        }
    }

    let hop_rate = sample_rate as f32 / hop_size as f32;
    let min_bpm = 70.0_f32;
    let max_bpm = 165.0_f32;

    let min_lag = (hop_rate * 60.0 / max_bpm) as usize;
    let max_lag = (hop_rate * 60.0 / min_bpm) as usize;

    let mut best_bpm = 125.0_f32;
    let mut max_corr = 0.0_f32;

    let search_hops = total_hops.min(3000);
    if search_hops <= max_lag {
        return 125.0;
    }

    for lag in min_lag..=max_lag {
        let mut corr = 0.0_f32;
        for i in 0..(search_hops - lag) {
            corr += onset[i] * onset[i + lag];
        }
        if corr > max_corr {
            max_corr = corr;
            best_bpm = (60.0 * hop_rate) / lag as f32;
        }
    }

    best_bpm.round()
}

#[cfg(test)]
mod tests {
    use super::{build_waveform, detect_bpm};

    #[test]
    fn waveform_preserves_peaks_from_all_channels() {
        let samples = [0.1, -0.8, 0.3, 0.2, -0.5, 0.4, 0.2, -0.1];
        let waveform = build_waveform(&samples, 2, 2);

        assert_eq!(waveform, vec![0.8, 0.5]);
    }

    #[test]
    fn waveform_handles_short_audio() {
        let waveform = build_waveform(&[0.25, -0.5], 1, 4);

        assert_eq!(waveform, vec![0.25, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn detect_bpm_returns_default_for_short_samples() {
        let samples = vec![0.1_f32; 1000];
        assert_eq!(detect_bpm(&samples, 1, 44100), 125.0);
    }
}
