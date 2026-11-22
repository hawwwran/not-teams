//! Microphone and speaker for calls. Everything above this module works in 20 ms frames of
//! 48 kHz mono `i16` (960 samples, one Opus frame); the default devices run at whatever
//! rate and channel count they like, so both directions downmix/upmix and resample.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

pub const RATE: u32 = 48_000;
pub const FRAME: usize = 960;
/// Playback keeps at most this much queued; older audio is dropped so delay can't build up.
const MAX_QUEUED: usize = FRAME * 10;

/// Loudness meters for the UI: peak of the last buffer, 0.0..=1.0, stored as f32 bits.
#[derive(Default)]
pub struct Levels {
    mic: AtomicU32,
    speaker: AtomicU32,
}

impl Levels {
    pub fn mic(&self) -> f32 {
        f32::from_bits(self.mic.load(Ordering::Relaxed))
    }
    pub fn speaker(&self) -> f32 {
        f32::from_bits(self.speaker.load(Ordering::Relaxed))
    }
}

/// Decoded far-end audio waiting to be played, 48 kHz mono.
pub type Playback = Arc<Mutex<VecDeque<i16>>>;

pub fn push_playback(queue: &Playback, samples: &[i16]) {
    let Ok(mut q) = queue.lock() else { return };
    q.extend(samples);
    let excess = q.len().saturating_sub(MAX_QUEUED);
    q.drain(..excess);
}

/// Open streams; dropping this stops both devices. cpal streams aren't `Send`, so keep it
/// on the thread that opened it.
pub struct Devices {
    _input: cpal::Stream,
    _output: cpal::Stream,
    /// Switches a Bluetooth headset back to its listening profile when the call's audio
    /// closes; dropped after the streams (fields drop in order).
    _profile: Option<crate::devices::ProfileGuard>,
}

pub fn open(
    mic_frames: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    playback: Playback,
    muted: Arc<AtomicBool>,
    levels: Arc<Levels>,
    choice: &crate::devices::DeviceChoice,
) -> Result<Devices, String> {
    let (resolved, profile) = crate::devices::prepare(choice);
    let choice = &resolved;
    let host = cpal::default_host();
    // A chosen device that's gone (unplugged headset) falls back to the default. PipeWire
    // choices open the default too and are routed once the streams run.
    let chosen = |id: &str| {
        (!crate::devices::routed(id)).then(|| id.parse::<cpal::DeviceId>().ok().and_then(|id| host.device_by_id(&id))).flatten()
    };
    let input_dev = chosen(&choice.microphone).or_else(|| host.default_input_device()).ok_or("No microphone found.")?;
    let output_dev = chosen(&choice.speaker).or_else(|| host.default_output_device()).ok_or("No speaker or headphones found.")?;
    let in_cfg = input_dev.default_input_config().map_err(|e| format!("Microphone: {e}"))?;
    let out_cfg = output_dev.default_output_config().map_err(|e| format!("Speaker: {e}"))?;

    let input = match in_cfg.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&input_dev, in_cfg.config(), mic_frames, muted, levels.clone()),
        SampleFormat::I16 => build_input::<i16>(&input_dev, in_cfg.config(), mic_frames, muted, levels.clone()),
        SampleFormat::I32 => build_input::<i32>(&input_dev, in_cfg.config(), mic_frames, muted, levels.clone()),
        SampleFormat::U16 => build_input::<u16>(&input_dev, in_cfg.config(), mic_frames, muted, levels.clone()),
        f => return Err(format!("Microphone sample format {f} isn't supported.")),
    }?;
    let output = match out_cfg.sample_format() {
        SampleFormat::F32 => build_output::<f32>(&output_dev, out_cfg.config(), playback, levels),
        SampleFormat::I16 => build_output::<i16>(&output_dev, out_cfg.config(), playback, levels),
        SampleFormat::I32 => build_output::<i32>(&output_dev, out_cfg.config(), playback, levels),
        SampleFormat::U16 => build_output::<u16>(&output_dev, out_cfg.config(), playback, levels),
        f => return Err(format!("Speaker sample format {f} isn't supported.")),
    }?;
    input.play().map_err(|e| format!("Microphone: {e}"))?;
    output.play().map_err(|e| format!("Speaker: {e}"))?;
    if crate::devices::routed(&choice.microphone) && crate::devices::routed(&choice.speaker) && (!choice.microphone.is_empty() || !choice.speaker.is_empty()) {
        let choice = choice.clone();
        std::thread::spawn(move || {
            // The streams' PipeWire nodes appear shortly after they start.
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if crate::devices::route_streams(&choice) >= 2 {
                    break;
                }
            }
        });
    }
    Ok(Devices {
        _input: input,
        _output: output,
        _profile: profile,
    })
}

fn build_input<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    frames: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    muted: Arc<AtomicBool>,
    levels: Arc<Levels>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let mut resampler = PushResampler::new(config.sample_rate, RATE);
    let mut mono: Vec<f32> = Vec::new();
    let mut resampled = Vec::new();
    let mut pending: Vec<i16> = Vec::with_capacity(FRAME * 2);
    dev.build_input_stream(
        config,
        move |data: &[T], _| {
            mono.clear();
            mono.extend(data.chunks(channels).map(|c| c.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32));
            let peak = mono.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0);
            let silent = muted.load(Ordering::Relaxed);
            levels.mic.store(if silent { 0f32 } else { peak }.to_bits(), Ordering::Relaxed);
            resampled.clear();
            resampler.process(&mono, &mut resampled);
            pending.extend(resampled.iter().map(|s| if silent { 0 } else { to_i16(*s) }));
            while pending.len() >= FRAME {
                let frame: Vec<i16> = pending.drain(..FRAME).collect();
                let _ = frames.send(frame);
            }
        },
        |e| eprintln!("microphone: {e}"),
        None,
    )
    .map_err(|e| format!("Microphone: {e}"))
}

fn build_output<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    playback: Playback,
    levels: Arc<Levels>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let step = RATE as f64 / config.sample_rate as f64;
    let (mut pos, mut prev, mut cur) = (1.0f64, 0f32, 0f32);
    dev.build_output_stream(
        config,
        move |out: &mut [T], _| {
            let mut q = playback.lock().ok();
            let mut peak = 0f32;
            for frame in out.chunks_mut(channels) {
                while pos >= 1.0 {
                    prev = cur;
                    cur = q.as_mut().and_then(|q| q.pop_front()).map(|s| s as f32 / 32768.0).unwrap_or(0.0);
                    pos -= 1.0;
                }
                let v = prev + (cur - prev) * pos as f32;
                pos += step;
                peak = peak.max(v.abs());
                for s in frame.iter_mut() {
                    *s = T::from_sample(v);
                }
            }
            levels.speaker.store(peak.min(1.0).to_bits(), Ordering::Relaxed);
        },
        |e| eprintln!("speaker: {e}"),
        None,
    )
    .map_err(|e| format!("Speaker: {e}"))
}

fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Linear resampling of a pushed stream. Positions index the sequence `prev, input...`, so
/// interpolation can span the boundary between two callbacks.
struct PushResampler {
    step: f64,
    pos: f64,
    prev: f32,
}

impl PushResampler {
    fn new(from: u32, to: u32) -> Self {
        PushResampler {
            step: from as f64 / to as f64,
            pos: 1.0,
            prev: 0.0,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len();
        if n == 0 {
            return;
        }
        let at = |i: usize| if i == 0 { self.prev } else { input[i - 1] };
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            out.push(at(i) + (at(i + 1) - at(i)) * f);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.prev = input[n - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_rate() {
        let mut r = PushResampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.process(&[0.5; 441], &mut out);
        }
        // 1 s at 44.1 kHz in → about 1 s at 48 kHz out.
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
        assert!(out[10..].iter().all(|v| (v - 0.5).abs() < 1e-6));
    }
}
