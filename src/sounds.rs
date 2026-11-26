//! The sounds NotTeams plays for a new message, a call ringing us and a meeting starting, each
//! one of six the person picks in Settings. They're synthesised here (sine tones with
//! envelopes), so there's no recording and no licence to respect; they're written once as WAV
//! files in the cache and played with the desktop's player (PipeWire's `pw-play`, or
//! `paplay`/`aplay`), without waiting.

use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

const RATE: u32 = 48_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    Chime,
    Bell,
    Pop,
    Marimba,
    Pulse,
    Alarm,
}

pub const ALL: [Tone; 6] = [Tone::Chime, Tone::Bell, Tone::Pop, Tone::Marimba, Tone::Pulse, Tone::Alarm];

/// What a sound is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Message,
    Call,
    MeetingStarted,
}

/// The tone for each event, kept in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub message: Tone,
    pub call: Tone,
    pub meeting: Tone,
}

impl Default for Choice {
    fn default() -> Self {
        Choice { message: Tone::Pop, call: Tone::Marimba, meeting: Tone::Alarm }
    }
}

impl Choice {
    pub fn tone(&self, event: Event) -> Tone {
        match event {
            Event::Message => self.message,
            Event::Call => self.call,
            Event::MeetingStarted => self.meeting,
        }
    }

    pub fn slot(&mut self, event: Event) -> &mut Tone {
        match event {
            Event::Message => &mut self.message,
            Event::Call => &mut self.call,
            Event::MeetingStarted => &mut self.meeting,
        }
    }
}

impl Tone {
    pub fn label(self) -> &'static str {
        match self {
            Tone::Chime => "Chime",
            Tone::Bell => "Bell",
            Tone::Pop => "Pop",
            Tone::Marimba => "Marimba",
            Tone::Pulse => "Pulse",
            Tone::Alarm => "Alarm",
        }
    }

    /// The sound as 48 kHz mono samples, at most about three seconds (a ringing call repeats
    /// it every 3.5 s).
    fn samples(self) -> Vec<f32> {
        let mut out = Vec::new();
        match self {
            Tone::Chime => {
                for (n, f) in [1046.5, 1318.5, 1568.0].iter().enumerate() {
                    add(&mut out, n as f32 * 0.18, 1.2, |t| note(t, *f, &[(1.0, 1.0), (2.0, 0.3)], 4.5));
                }
            }
            Tone::Bell => add(&mut out, 0.0, 2.2, |t| note(t, 880.0, &[(1.0, 1.0), (2.0, 0.5), (2.76, 0.35), (5.4, 0.2)], 2.0)),
            Tone::Pop => {
                add(&mut out, 0.0, 0.09, |t| sweep(t, 0.09, 520.0, 780.0));
                add(&mut out, 0.12, 0.11, |t| sweep(t, 0.11, 780.0, 1170.0));
            }
            Tone::Marimba => {
                for (start, f) in [(0.0, 784.0), (0.15, 1174.7), (0.6, 784.0), (0.75, 1174.7)] {
                    add(&mut out, start, 0.5, |t| note(t, f, &[(1.0, 1.0), (3.9, 0.25)], 8.0));
                }
            }
            Tone::Pulse => {
                for n in 0..4 {
                    add(&mut out, n as f32 * 0.15, 0.08, |t| beep(t, 0.08, 1000.0));
                }
            }
            Tone::Alarm => {
                for n in 0..12 {
                    let start = n as f32 * 0.17 + if n >= 6 { 0.35 } else { 0.0 };
                    let f = if n % 2 == 0 { 1318.5 } else { 987.8 };
                    add(&mut out, start, 0.15, |t| beep(t, 0.15, f));
                }
            }
        }
        let peak = out.iter().fold(0f32, |m, s| m.max(s.abs())).max(1e-6);
        out.iter().map(|s| s / peak * 0.7).collect()
    }
}

/// Mixes `len` seconds of `f(t)` into `out` from `start` seconds.
fn add(out: &mut Vec<f32>, start: f32, len: f32, f: impl Fn(f32) -> f32) {
    let first = (start * RATE as f32) as usize;
    let count = (len * RATE as f32) as usize;
    if out.len() < first + count {
        out.resize(first + count, 0.0);
    }
    for i in 0..count {
        out[first + i] += f(i as f32 / RATE as f32);
    }
}

/// A struck note: partials (frequency ratio, level), a quick attack and an exponential decay.
fn note(t: f32, f: f32, partials: &[(f32, f32)], decay: f32) -> f32 {
    let attack = (t / 0.004).min(1.0);
    let body: f32 = partials.iter().map(|(ratio, level)| (TAU * f * ratio * t).sin() * level * (-t * decay * ratio.sqrt()).exp()).sum();
    body * attack
}

/// A short soft blip whose pitch slides from `from` to `to`.
fn sweep(t: f32, len: f32, from: f32, to: f32) -> f32 {
    let k = (to - from) / len;
    let phase = TAU * (from * t + k * t * t / 2.0);
    phase.sin() * (std::f32::consts::PI * t / len).sin()
}

/// An electronic beep with a little edge (a third harmonic), faded in and out against clicks.
fn beep(t: f32, len: f32, f: f32) -> f32 {
    let fade = (t / 0.008).min(1.0).min((len - t) / 0.008).max(0.0);
    ((TAU * f * t).sin() + 0.3 * (TAU * 3.0 * f * t).sin()) * fade
}

/// The sound as a 16-bit mono WAV file.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

/// Plays the tone chosen in Settings for `event`, without waiting.
pub fn play_for(event: Event) {
    std::thread::spawn(move || play_now(crate::settings::Settings::load().sounds.tone(event)));
}

/// Plays a tone without waiting (for trying one in Settings).
pub fn play(tone: Tone) {
    std::thread::spawn(move || play_now(tone));
}

fn play_now(tone: Tone) {
    // Written once; the version in the name makes a changed sound be written again. Written
    // beside and renamed, so a second sound starting meanwhile never plays half a file.
    let path = crate::brand::cache_dir().join("sounds").join(format!("{}-v1.wav", tone.label().to_lowercase()));
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = path.with_extension(format!("{:?}.part", std::thread::current().id()).replace(['(', ')'], ""));
        if std::fs::write(&part, wav(&tone.samples())).is_err() || std::fs::rename(&part, &path).is_err() {
            let _ = std::fs::remove_file(&part);
            return;
        }
    }
    for player in ["pw-play", "paplay", "aplay"] {
        let played = std::process::Command::new(player)
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if played {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tone_is_short_audible_and_unclipped() {
        for tone in ALL {
            let s = tone.samples();
            let secs = s.len() as f32 / RATE as f32;
            assert!((0.15..=3.0).contains(&secs), "{tone:?} lasts {secs} s");
            let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!((0.69..=0.71).contains(&peak), "{tone:?} peaks at {peak}");
        }
    }

    #[test]
    fn wav_header_matches_the_data() {
        let w = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
    }
}
