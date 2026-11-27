//! H.264 for video calls, the codec Teams' media processor takes (constrained baseline,
//! packetization mode 1). Encoding and decoding run on their own threads with OpenH264;
//! RTP packets are reassembled into access units here.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, Profile, UsageType};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};

use crate::camera::{Frame, Latest};

pub const FPS: u32 = 15;
const BITRATE: u32 = 600_000;
/// A keyframe this often, so a receiver that missed one recovers without asking.
const KEYFRAME_EVERY: Duration = Duration::from_secs(3);

/// What the media processor asks of a stream we send, in a `vdclc` message on the data
/// channel: the largest picture (in 16×16 macroblocks), bitrate and frame rate the people
/// watching it can take, and whether they need a keyframe now. In a meeting with several
/// people it asks for a small picture; a larger one isn't forwarded to them at all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SendLimit {
    pub max_fs: Option<u32>,
    pub max_br_kbps: Option<u32>,
    pub max_fps: Option<u32>,
    pub keyframe: bool,
}

/// Limits by the stream id they're for (`sourceId`).
pub type SendLimits = Arc<Mutex<HashMap<u64, SendLimit>>>;

impl SendLimit {
    /// One `fmtParamsList` entry: `KeyFrame=1;max-br=208;max-fps=1500.000000;max-fs=405;...`
    /// (`max-fps` in hundredths).
    pub fn parse(params: &str) -> SendLimit {
        let mut limit = SendLimit::default();
        for (key, value) in params.split(';').filter_map(|kv| kv.split_once('=')) {
            let number = value.trim().parse::<f64>().ok();
            match key.trim() {
                "KeyFrame" => limit.keyframe = value.trim() == "1",
                "max-br" => limit.max_br_kbps = number.map(|n| n as u32),
                "max-fs" => limit.max_fs = number.map(|n| n as u32),
                "max-fps" => limit.max_fps = number.map(|n| (n / 100.0).round() as u32).filter(|f| *f > 0),
                _ => {}
            }
        }
        limit
    }

    /// The size to send a `w`×`h` picture at: the same shape, in whole macroblocks, no more
    /// of them than asked for, and never larger than the picture.
    pub fn size_for(&self, w: usize, h: usize) -> (usize, usize) {
        let Some(max) = self.max_fs.filter(|m| *m > 0) else { return (w, h) };
        let blocks = |w: usize, h: usize| w.div_ceil(16) * h.div_ceil(16);
        if blocks(w, h) <= max as usize {
            return (w, h);
        }
        let scale = ((max as f64 * 256.0) / (w * h) as f64).sqrt();
        let (mut sw, mut sh) = (((w as f64 * scale) as usize / 16) * 16, ((h as f64 * scale) as usize / 16) * 16);
        while blocks(sw, sh) > max as usize && sw > 16 && sh > 16 {
            sw -= 16;
            sh = (sh as f64 - 16.0 * h as f64 / w as f64).max(16.0) as usize / 2 * 2;
        }
        (sw.max(16), sh.max(16))
    }
}

/// Encodes the newest camera frame up to FPS times a second while `on`, handing each encoded
/// frame (Annex B) to `out`, within what the media processor asks of `stream` in `limits`.
/// Stops when `out` closes.
pub fn encode_loop(latest: Latest, on: Arc<AtomicBool>, out: tokio::sync::mpsc::UnboundedSender<Vec<u8>>, limits: Option<(SendLimits, u64)>) {
    encode_with(latest, on, out, FPS, BITRATE, UsageType::CameraVideoRealTime, limits);
}

/// The shared screen: fewer frames, more bits for sharp text, and the encoder's screen mode,
/// within what the media processor asks of `stream` in `limits`.
pub fn encode_screen_loop(latest: Latest, on: Arc<AtomicBool>, out: tokio::sync::mpsc::UnboundedSender<Vec<u8>>, limits: Option<(SendLimits, u64)>) {
    encode_with(latest, on, out, crate::screen::FPS, SCREEN_BITRATE, UsageType::ScreenContentRealTime, limits);
}

/// The shared screen's bitrate when Teams doesn't say otherwise; it allowed about 3 Mbit/s for
/// a 1080p share (2026-11-24).
const SCREEN_BITRATE: u32 = 3_000_000;

/// Lowest bitrate the encoder is set to, whatever is asked: below it the picture falls apart.
const MIN_BITRATE: u32 = 90_000;

fn encode_with(
    latest: Latest,
    on: Arc<AtomicBool>,
    out: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    base_fps: u32,
    base_bitrate: u32,
    usage: UsageType,
    limits: Option<(SendLimits, u64)>,
) {
    std::thread::Builder::new()
        .name("video-encode".into())
        .spawn(move || {
            let make = |bitrate: u32, fps: u32| {
                let config = EncoderConfig::new()
                    .bitrate(BitRate::from_bps(bitrate))
                    .max_frame_rate(FrameRate::from_hz(fps as f32))
                    .usage_type(usage)
                    .profile(Profile::Baseline);
                Encoder::with_api_config(crate::codec::api()?, config).ok()
            };
            // Made once there's something to send; the codec may still be downloading when
            // the call starts, so a missing one is tried again each second.
            let mut encoder: Option<Encoder> = None;
            let mut tried = Instant::now() - Duration::from_secs(1);
            // (bitrate, fps, width, height) the encoder was made for.
            let mut made = (0, 0, 0usize, 0usize);
            let mut last_seq = 0;
            let mut last_key = Instant::now() - KEYFRAME_EVERY;
            let mut was_on = false;
            loop {
                let limit = limits.as_ref().and_then(|(map, stream)| {
                    let mut map = map.lock().ok()?;
                    let l = map.get_mut(stream)?;
                    let copy = *l;
                    l.keyframe = false;
                    Some(copy)
                });
                let limit = limit.unwrap_or_default();
                let fps = limit.max_fps.map_or(base_fps, |f| f.clamp(1, base_fps));
                let bitrate = limit.max_br_kbps.map_or(base_bitrate, |k| (k * 1000).clamp(MIN_BITRATE, base_bitrate));
                std::thread::sleep(Duration::from_millis(1000 / fps as u64));
                if out.is_closed() {
                    return;
                }
                let now_on = on.load(Ordering::Relaxed);
                if !now_on {
                    was_on = false;
                    continue;
                }
                let Some(frame) = latest.lock().ok().and_then(|f| f.clone()) else { continue };
                if frame.seq == last_seq {
                    continue;
                }
                last_seq = frame.seq;
                let (w, h) = limit.size_for(frame.width, frame.height);
                if encoder.is_none() || made != (bitrate, fps, w, h) {
                    if encoder.is_none() && tried.elapsed() < Duration::from_secs(1) {
                        continue;
                    }
                    tried = Instant::now();
                    let Some(e) = make(bitrate, fps) else {
                        encoder = None;
                        continue;
                    };
                    if made.2 != 0 {
                        crate::call::call_log(&format!("sending video at {w}x{h}, {fps} fps, {} kbit/s", bitrate / 1000));
                    }
                    encoder = Some(e);
                    made = (bitrate, fps, w, h);
                    was_on = false;
                }
                let Some(encoder) = encoder.as_mut() else { continue };
                // A fresh start (camera just turned on), a receiver asking, or the regular
                // interval: keyframe.
                if !was_on || limit.keyframe || last_key.elapsed() >= KEYFRAME_EVERY {
                    encoder.force_intra_frame();
                    last_key = Instant::now();
                }
                was_on = true;
                let scaled;
                let frame: &Frame = if (w, h) == (frame.width, frame.height) {
                    &frame
                } else {
                    scaled = scale(&frame, w, h);
                    &scaled
                };
                let yuv = YUVBuffer::from_rgba8_source(RgbaSliceU8::new(&frame.rgba, (frame.width, frame.height)));
                if let Ok(bits) = encoder.encode(&yuv) {
                    let data = bits.to_vec();
                    if !data.is_empty() {
                        let _ = out.send(data);
                    }
                }
            }
        })
        .expect("video encode thread");
}

/// A smaller copy of a frame: each pixel the average of the ones it covers.
fn scale(frame: &Frame, w: usize, h: usize) -> Frame {
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let (y0, y1) = (y * frame.height / h, ((y + 1) * frame.height / h).max(y * frame.height / h + 1).min(frame.height));
        for x in 0..w {
            let (x0, x1) = (x * frame.width / w, ((x + 1) * frame.width / w).max(x * frame.width / w + 1).min(frame.width));
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * frame.width + sx) * 4;
                    for c in 0..4 {
                        acc[c] += frame.rgba[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * w + x) * 4;
            for c in 0..4 {
                rgba[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    Frame { width: w, height: h, rgba, seq: frame.seq, source: frame.source }
}

/// Decodes access units, each with its sender's source id when known, into `latest`, for
/// the window to show. Stops when `units` closes.
pub fn decode_loop(units: mpsc::Receiver<(Vec<u8>, Option<u64>)>, latest: Latest) {
    std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            let new_decoder = || Decoder::with_api_config(crate::codec::api()?, DecoderConfig::new()).ok();
            // As for encoding: a codec that arrives during the call is picked up then.
            let mut decoder = None;
            let mut tried = Instant::now() - Duration::from_secs(1);
            let mut seq = 0u64;
            let mut last_source = None;
            for (unit, source) in units {
                // Someone else's stream: start clean rather than build on the last one's frames.
                if source.is_some() && source != last_source {
                    if last_source.is_some() {
                        decoder = None;
                        // The switch comes with a keyframe; don't drop it waiting out a retry.
                        tried = Instant::now() - Duration::from_secs(1);
                    }
                    last_source = source;
                }
                if decoder.is_none() && tried.elapsed() >= Duration::from_secs(1) {
                    decoder = new_decoder();
                    if decoder.is_none() {
                        tried = Instant::now();
                    }
                }
                let Some(dec) = decoder.as_mut() else { continue };
                let Ok(Some(yuv)) = dec.decode(&unit) else { continue };
                let (width, height) = yuv.dimensions();
                let mut rgba = vec![0u8; width * height * 4];
                yuv.write_rgba8(&mut rgba);
                seq += 1;
                if let Ok(mut slot) = latest.lock() {
                    *slot = Some(Arc::new(Frame { width, height, rgba, seq, source }));
                }
            }
        })
        .expect("video decode thread");
}

/// Reassembles H.264 RTP payloads (RFC 6184: single NAL units, STAP-A, FU-A) into
/// Annex B access units, one per marker bit.
#[derive(Default)]
pub struct Depacketizer {
    unit: Vec<u8>,
    fragment: Vec<u8>,
}

const START: [u8; 4] = [0, 0, 0, 1];

impl Depacketizer {
    pub fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        let Some(&first) = payload.first() else { return None };
        match first & 0x1F {
            1..=23 => {
                self.unit.extend_from_slice(&START);
                self.unit.extend_from_slice(payload);
            }
            24 => {
                let mut rest = &payload[1..];
                while rest.len() > 2 {
                    let size = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                    if rest.len() < 2 + size {
                        break;
                    }
                    self.unit.extend_from_slice(&START);
                    self.unit.extend_from_slice(&rest[2..2 + size]);
                    rest = &rest[2 + size..];
                }
            }
            28 if payload.len() > 2 => {
                let header = payload[1];
                if header & 0x80 != 0 {
                    self.fragment.clear();
                    self.fragment.push((first & 0xE0) | (header & 0x1F));
                }
                if !self.fragment.is_empty() {
                    self.fragment.extend_from_slice(&payload[2..]);
                }
                if header & 0x40 != 0 && !self.fragment.is_empty() {
                    self.unit.extend_from_slice(&START);
                    self.unit.append(&mut self.fragment);
                }
            }
            _ => {}
        }
        if marker && !self.unit.is_empty() {
            return Some(std::mem::take(&mut self.unit));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_and_aggregates_reassemble() {
        let mut d = Depacketizer::default();
        // STAP-A carrying two NALs (SPS 0x67, PPS 0x68), then an IDR (0x65) in two FU-A parts.
        assert!(d.push(&[24, 0, 2, 0x67, 1, 0, 2, 0x68, 2], false).is_none());
        assert!(d.push(&[28, 0x85, 0xAA], false).is_none());
        let unit = d.push(&[28, 0x45, 0xBB], true).unwrap();
        assert_eq!(unit, vec![0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x05, 0xAA, 0xBB]);
    }

    #[test]
    fn follows_what_the_media_processor_asks() {
        let l = SendLimit::parse("KeyFrame=1;max-br=208;max-fps=1500.000000;max-fs=405;max-mbps=6075;sequence-number=1;ssrc=4256372932");
        assert_eq!((l.max_fs, l.max_br_kbps, l.max_fps, l.keyframe), (Some(405), Some(208), Some(15), true));
        let (w, h) = l.size_for(640, 480);
        assert!(w.div_ceil(16) * h.div_ceil(16) <= 405 && w % 16 == 0, "{w}x{h}");
        assert!((w as f64 / h as f64 - 4.0 / 3.0).abs() < 0.1, "keeps the shape: {w}x{h}");
        let (w, h) = SendLimit::parse("max-fs=240").size_for(640, 480);
        assert!(w.div_ceil(16) * h.div_ceil(16) <= 240, "{w}x{h}");
        assert_eq!(SendLimit::default().size_for(640, 480), (640, 480));
        assert_eq!(SendLimit::parse("max-fs=8160").size_for(640, 480), (640, 480));
        let f = scale(&Frame { width: 640, height: 480, rgba: vec![200; 640 * 480 * 4], seq: 1, source: None }, w, h);
        assert_eq!(f.rgba.len(), w * h * 4);
        assert!(f.rgba.iter().all(|v| *v == 200));
    }

    #[test]
    #[ignore = "needs Cisco's codec, which NotTeams downloads on its first start"]
    fn encodes_and_decodes_a_frame() {
        let config = EncoderConfig::new().bitrate(BitRate::from_bps(BITRATE)).profile(Profile::Baseline);
        let mut enc = Encoder::with_api_config(crate::codec::api().expect("the downloaded codec"), config).unwrap();
        let rgba = vec![120u8; 64 * 48 * 4];
        let yuv = YUVBuffer::from_rgba8_source(RgbaSliceU8::new(&rgba, (64, 48)));
        let bits = enc.encode(&yuv).unwrap().to_vec();
        let mut dec = Decoder::with_api_config(crate::codec::api().unwrap(), DecoderConfig::new()).unwrap();
        let out = dec.decode(&bits).unwrap().expect("a picture");
        assert_eq!(out.dimensions(), (64, 48));
    }
}
