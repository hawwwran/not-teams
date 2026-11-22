//! G.711 (PCMU μ-law and PCMA A-law) at 8 kHz. Incoming Teams calls are offered the codec
//! set of Teams' desktop clients (SILK, SATIN, G.722, G.711...), without Opus; G.711 is the
//! one a WebRTC stack can carry that we can code ourselves. 20 ms = 160 samples = 160 bytes.

pub const RATE: u32 = 8_000;
pub const FRAME: usize = 160;

const BIAS: i32 = 0x84;
const CLIP: i32 = 32635;

pub fn ulaw_encode(sample: i16) -> u8 {
    let mut s = sample as i32;
    let sign = if s < 0 { 0x80 } else { 0 };
    if s < 0 {
        s = -s;
    }
    s = s.min(CLIP) + BIAS;
    let mut exponent = 7;
    let mut mask = 0x4000;
    while exponent > 0 && s & mask == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = (s >> (exponent + 3)) & 0x0F;
    !(sign | (exponent << 4) | mantissa) as u8
}

pub fn ulaw_decode(byte: u8) -> i16 {
    let u = !byte as i32;
    let sign = u & 0x80;
    let exponent = (u >> 4) & 0x07;
    let mantissa = u & 0x0F;
    let magnitude = (((mantissa << 3) + BIAS) << exponent) - BIAS;
    (if sign != 0 { -magnitude } else { magnitude }) as i16
}

pub fn alaw_decode(byte: u8) -> i16 {
    let a = (byte ^ 0x55) as i32;
    let sign = a & 0x80;
    let exponent = (a >> 4) & 0x07;
    let mantissa = a & 0x0F;
    let magnitude = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    (if sign != 0 { magnitude } else { -magnitude }) as i16
}

/// 48 kHz → 8 kHz by averaging each group of six samples (a crude but adequate low-pass for
/// speech) and μ-law encoding.
pub fn encode_frame_48k(frame: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME);
    out.extend(frame
        .chunks(6)
        .map(|c| ulaw_encode((c.iter().map(|&s| s as i32).sum::<i32>() / c.len() as i32) as i16)));
    out
}

/// G.711 payload → 48 kHz samples, linearly interpolated; `last` carries the previous
/// packet's final sample so the seams stay smooth.
pub fn decode_to_48k(payload: &[u8], alaw: bool, last: &mut i16) -> Vec<i16> {
    let mut out = Vec::with_capacity(payload.len() * 6);
    for &b in payload {
        let s = if alaw { alaw_decode(b) } else { ulaw_decode(b) };
        for k in 1..=6 {
            out.push((*last as i32 + (s as i32 - *last as i32) * k / 6) as i16);
        }
        *last = s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulaw_round_trip_is_close() {
        for s in [-32000i16, -5000, -100, 0, 100, 5000, 32000] {
            let back = ulaw_decode(ulaw_encode(s));
            assert!((back as i32 - s as i32).abs() <= (s as i32).abs() / 16 + 8, "{s} -> {back}");
        }
    }

    #[test]
    fn known_codes() {
        assert_eq!(ulaw_encode(0), 0xFF);
        assert_eq!(ulaw_decode(0xFF), 0);
        assert_eq!(alaw_decode(0xD5), 8);
    }

    #[test]
    fn frame_sizes() {
        assert_eq!(encode_frame_48k(&[0; 960]).len(), FRAME);
        let mut last = 0;
        assert_eq!(decode_to_48k(&[0xFF; FRAME], false, &mut last).len(), 960);
    }
}
