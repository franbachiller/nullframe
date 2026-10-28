use std::{f32::consts::TAU, sync::Arc};

use rustfft::{Fft, FftPlanner, num_complex::Complex};

use crate::audio::AudioTrack;

pub const SPECTRUM_BANDS: usize = 72;
const FFT_SIZE: usize = 2048;

#[derive(Clone)]
pub struct AnalysisFrame {
    pub spectrum: [f32; SPECTRUM_BANDS],
    pub bass: f32,
    pub mids: f32,
    pub treble: f32,
    pub rms: f32,
    pub transient: f32,
}

impl Default for AnalysisFrame {
    fn default() -> Self {
        Self {
            spectrum: [0.0; SPECTRUM_BANDS],
            bass: 0.0,
            mids: 0.0,
            treble: 0.0,
            rms: 0.0,
            transient: 0.0,
        }
    }
}

pub struct AudioAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    input: Vec<Complex<f32>>,
    smoothed: AnalysisFrame,
}

impl AudioAnalyzer {
    pub fn new() -> Self {
        let mut planner = FftPlanner::new();
        Self {
            fft: planner.plan_fft_forward(FFT_SIZE),
            input: vec![Complex::default(); FFT_SIZE],
            smoothed: AnalysisFrame::default(),
        }
    }

    pub fn analyze(&mut self, track: &AudioTrack, position: f32) -> &AnalysisFrame {
        let channels = usize::from(track.channels);
        let center_frame = (position * track.sample_rate as f32) as usize;
        let start_frame = center_frame.saturating_sub(FFT_SIZE / 2);
        let total_frames = track.samples.len() / channels;
        let mut rms_sum = 0.0;

        for index in 0..FFT_SIZE {
            let frame = start_frame + index;
            let mono = if frame < total_frames {
                let offset = frame * channels;
                track.samples[offset..offset + channels]
                    .iter()
                    .copied()
                    .sum::<f32>()
                    / channels as f32
            } else {
                0.0
            };
            let window = 0.5 - 0.5 * (TAU * index as f32 / FFT_SIZE as f32).cos();
            self.input[index] = Complex::new(mono * window, 0.0);
            rms_sum += mono * mono;
        }

        self.fft.process(&mut self.input);

        let mut next = AnalysisFrame::default();
        next.rms = (rms_sum / FFT_SIZE as f32).sqrt().min(1.0);
        next.spectrum = spectrum_bands(&self.input, track.sample_rate);
        next.bass = band_average(&next.spectrum, 0, 18);
        next.mids = band_average(&next.spectrum, 18, 48);
        next.treble = band_average(&next.spectrum, 48, SPECTRUM_BANDS);
        next.transient = ((next.rms - self.smoothed.rms).max(0.0) * 12.0).min(1.0);

        smooth_frame(&mut self.smoothed, &next);
        &self.smoothed
    }

    pub fn reset(&mut self) {
        self.smoothed = AnalysisFrame::default();
    }
}

fn spectrum_bands(fft_output: &[Complex<f32>], sample_rate: u32) -> [f32; SPECTRUM_BANDS] {
    let mut bands = [0.0; SPECTRUM_BANDS];
    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
    let min_hz: f32 = 35.0;
    let max_hz = 18_000.0_f32.min(sample_rate as f32 * 0.5);
    let frequency_ratio = max_hz / min_hz;

    for (band, value) in bands.iter_mut().enumerate() {
        let low_hz = min_hz * frequency_ratio.powf(band as f32 / SPECTRUM_BANDS as f32);
        let high_hz = min_hz * frequency_ratio.powf((band + 1) as f32 / SPECTRUM_BANDS as f32);
        let low_bin = (low_hz / bin_hz).floor() as usize;
        let high_bin = ((high_hz / bin_hz).ceil() as usize)
            .max(low_bin + 1)
            .min(FFT_SIZE / 2);
        let magnitude = fft_output[low_bin..high_bin]
            .iter()
            .map(|bin| bin.norm())
            .sum::<f32>()
            / (high_bin - low_bin) as f32
            / FFT_SIZE as f32;
        *value = (magnitude * 18.0).sqrt().min(1.0);
    }

    bands
}

fn band_average(spectrum: &[f32], start: usize, end: usize) -> f32 {
    spectrum[start..end].iter().sum::<f32>() / (end - start) as f32
}

fn smooth_frame(current: &mut AnalysisFrame, next: &AnalysisFrame) {
    for (current_band, next_band) in current.spectrum.iter_mut().zip(next.spectrum) {
        let speed = if next_band > *current_band {
            0.52
        } else {
            0.16
        };
        *current_band += (next_band - *current_band) * speed;
    }
    current.bass += (next.bass - current.bass) * 0.3;
    current.mids += (next.mids - current.mids) * 0.3;
    current.treble += (next.treble - current.treble) * 0.3;
    current.rms += (next.rms - current.rms) * 0.35;
    current.transient += (next.transient - current.transient) * 0.45;
}

#[cfg(test)]
mod tests {
    use rustfft::num_complex::Complex;

    use super::{SPECTRUM_BANDS, band_average, spectrum_bands};

    #[test]
    fn silent_fft_produces_silent_spectrum() {
        let fft = vec![Complex::default(); 2048];
        let spectrum = spectrum_bands(&fft, 48_000);

        assert!(spectrum.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn band_average_uses_requested_range() {
        let mut spectrum = [0.0; SPECTRUM_BANDS];
        spectrum[4..8].fill(0.5);

        assert_eq!(band_average(&spectrum, 4, 8), 0.5);
    }
}
