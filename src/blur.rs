//! Background blur for the camera: MediaPipe's selfie segmentation model (landscape,
//! 256×144, Apache 2.0, the ONNX conversion from onnx-community) finds the person in each
//! frame, and everything else is blurred. Runs in pure Rust through tract, on its own thread,
//! so a slow frame never holds up the camera or the encoder.

use std::sync::{Arc, Mutex};

use tract_onnx::prelude::*;

use crate::camera::Frame;

const MODEL: &[u8] = include_bytes!("../assets/models/selfie_segmentation_landscape.onnx");
const IN_W: usize = 256;
const IN_H: usize = 144;
/// The background is blurred at this fraction of the frame size, which also makes the blur
/// cheap: a box blur on the small copy, scaled back up, is soft enough.
const BLUR_SCALE: usize = 8;

type Plan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

pub struct Segmenter {
    plan: Plan,
    /// Whether the model takes channels last (NHWC) or first (NCHW).
    channels_last: bool,
}

impl Segmenter {
    pub fn new() -> Result<Self, String> {
        let err = |e: TractError| format!("Background blur: {e:?}");
        // Every value in the file is declared with a symbolic batch size, which fixing the input
        // to one picture contradicts; drop those declarations so tract works the shapes out
        // from the input.
        let onnx = tract_onnx::onnx();
        let mut proto = onnx.proto_model_for_read(&mut std::io::Cursor::new(MODEL)).map_err(err)?;
        if let Some(graph) = proto.graph.as_mut() {
            graph.value_info.clear();
            for output in graph.output.iter_mut() {
                output.r#type = None;
            }
        }
        let model = onnx.model_for_proto_model(&proto).map_err(err)?;
        let input = model.input_fact(0).map_err(err)?.clone();
        // Check the layout rather than assume it (this conversion is channels first).
        let dims = format!("{:?}", input.shape);
        let last = dims.rsplit(',').next().unwrap_or("").trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let channels_last = last == "3";
        let shape: [usize; 4] = if channels_last { [1, IN_H, IN_W, 3] } else { [1, 3, IN_H, IN_W] };
        let model = model.with_input_fact(0, f32::fact(shape).into()).map_err(err)?;
        let plan = model.into_optimized().map_err(err)?.into_runnable().map_err(err)?;
        Ok(Segmenter { plan, channels_last })
    }

    /// How much each pixel of a `IN_W`×`IN_H` copy of the frame is the person, 0.0..=1.0.
    pub fn mask(&self, frame: &Frame) -> Result<Vec<f32>, String> {
        let small = resize_rgb(frame, IN_W, IN_H);
        let input: Tensor = if self.channels_last {
            tract_ndarray::Array4::from_shape_fn((1, IN_H, IN_W, 3), |(_, y, x, c)| small[(y * IN_W + x) * 3 + c]).into()
        } else {
            tract_ndarray::Array4::from_shape_fn((1, 3, IN_H, IN_W), |(_, c, y, x)| small[(y * IN_W + x) * 3 + c]).into()
        };
        let out = self.plan.run(tvec!(input.into())).map_err(|e| format!("Background blur: {e}"))?;
        let mask = out[0].to_array_view::<f32>().map_err(|e| format!("Background blur: {e}"))?;
        let values: Vec<f32> = mask.iter().copied().collect();
        if values.len() != IN_W * IN_H {
            return Err(format!("Background blur: unexpected mask of {} values", values.len()));
        }
        Ok(values)
    }
}

/// Nearest-neighbour RGB copy of a frame at another size, values 0.0..=1.0.
fn resize_rgb(frame: &Frame, w: usize, h: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let sy = y * frame.height / h;
        for x in 0..w {
            let sx = x * frame.width / w;
            let i = (sy * frame.width + sx) * 4;
            out.extend(frame.rgba[i..i + 3].iter().map(|v| *v as f32 / 255.0));
        }
    }
    out
}

/// The frame with everything outside the person blurred.
pub fn apply(frame: &Frame, mask: &[f32]) -> Frame {
    let (w, h) = (frame.width, frame.height);
    let (bw, bh) = ((w / BLUR_SCALE).max(1), (h / BLUR_SCALE).max(1));
    // A small copy, box-blurred a few times: a soft background.
    let mut small = vec![0f32; bw * bh * 3];
    for y in 0..bh {
        for x in 0..bw {
            let (mut acc, mut n) = ([0f32; 3], 0f32);
            for sy in (y * h / bh)..((y + 1) * h / bh).min(h) {
                for sx in (x * w / bw)..((x + 1) * w / bw).min(w) {
                    let i = (sy * w + sx) * 4;
                    for c in 0..3 {
                        acc[c] += frame.rgba[i + c] as f32;
                    }
                    n += 1.0;
                }
            }
            for c in 0..3 {
                small[(y * bw + x) * 3 + c] = acc[c] / n.max(1.0);
            }
        }
    }
    for _ in 0..3 {
        small = box_blur(&small, bw, bh, 2);
    }
    let sample = |buf: &[f32], bw: usize, bh: usize, fx: f32, fy: f32, c: usize| -> f32 {
        let x0 = (fx.floor().max(0.0) as usize).min(bw - 1);
        let y0 = (fy.floor().max(0.0) as usize).min(bh - 1);
        let (x1, y1) = ((x0 + 1).min(bw - 1), (y0 + 1).min(bh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| buf[(y * bw + x) * 3 + c];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx.clamp(0.0, 1.0);
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx.clamp(0.0, 1.0);
        top + (bottom - top) * ty.clamp(0.0, 1.0)
    };
    let mask_at = |x: usize, y: usize| -> f32 {
        let fx = (x as f32 + 0.5) * IN_W as f32 / w as f32 - 0.5;
        let fy = (y as f32 + 0.5) * IN_H as f32 / h as f32 - 0.5;
        let x0 = (fx.floor().max(0.0) as usize).min(IN_W - 1);
        let y0 = (fy.floor().max(0.0) as usize).min(IN_H - 1);
        let (x1, y1) = ((x0 + 1).min(IN_W - 1), (y0 + 1).min(IN_H - 1));
        let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
        let m = |x: usize, y: usize| mask[y * IN_W + x];
        let top = m(x0, y0) + (m(x1, y0) - m(x0, y0)) * tx;
        let bottom = m(x0, y1) + (m(x1, y1) - m(x0, y1)) * tx;
        // Sharpen the soft edge a little so hair and shoulders don't smear.
        ((top + (bottom - top) * ty - 0.25) * 2.0).clamp(0.0, 1.0)
    };
    let mut rgba = frame.rgba.clone();
    for y in 0..h {
        let fy = (y as f32 + 0.5) * bh as f32 / h as f32 - 0.5;
        for x in 0..w {
            let person = mask_at(x, y);
            if person >= 0.999 {
                continue;
            }
            let fx = (x as f32 + 0.5) * bw as f32 / w as f32 - 0.5;
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let bg = sample(&small, bw, bh, fx, fy, c);
                rgba[i + c] = (frame.rgba[i + c] as f32 * person + bg * (1.0 - person)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Frame { width: w, height: h, rgba, seq: frame.seq, source: frame.source }
}

fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; src.len()];
    let mut out = vec![0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            for c in 0..3 {
                let sum: f32 = (lo..=hi).map(|xx| src[(y * w + xx) * 3 + c]).sum();
                tmp[(y * w + x) * 3 + c] = sum / (hi - lo + 1) as f32;
            }
        }
    }
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            for c in 0..3 {
                let sum: f32 = (lo..=hi).map(|yy| tmp[(yy * w + x) * 3 + c]).sum();
                out[(y * w + x) * 3 + c] = sum / (hi - lo + 1) as f32;
            }
        }
    }
    out
}

/// Blurs the camera's frames in the background while `on` is set: reads `raw`, writes `out`.
/// The mask is worked out on every frame the model has time for and reused in between.
pub fn run(raw: crate::camera::Latest, out: crate::camera::Latest, on: Arc<std::sync::atomic::AtomicBool>, stop: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    std::thread::Builder::new()
        .name("blur".into())
        .spawn(move || {
            let mut segmenter: Option<Segmenter> = None;
            let mut last_seq = 0u64;
            let mask: Arc<Mutex<Option<Vec<f32>>>> = Default::default();
            while !stop.load(Ordering::Relaxed) {
                let frame = raw.lock().ok().and_then(|f| f.clone());
                let Some(frame) = frame.filter(|f| f.seq != last_seq) else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                last_seq = frame.seq;
                let processed = if on.load(Ordering::Relaxed) {
                    if segmenter.is_none() {
                        match Segmenter::new() {
                            Ok(s) => segmenter = Some(s),
                            Err(e) => {
                                crate::call::call_log(&e);
                                on.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                    match segmenter.as_ref().map(|s| s.mask(&frame)) {
                        Some(Ok(m)) => {
                            let blurred = apply(&frame, &m);
                            if let Ok(mut slot) = mask.lock() {
                                *slot = Some(m);
                            }
                            Arc::new(blurred)
                        }
                        _ => frame,
                    }
                } else {
                    frame
                };
                if let Ok(mut slot) = out.lock() {
                    *slot = Some(processed);
                }
            }
        })
        .expect("blur thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_loads_and_masks() {
        let s = Segmenter::new().expect("model loads");
        let frame = Frame { width: 640, height: 480, rgba: vec![128; 640 * 480 * 4], seq: 1, source: None };
        let started = std::time::Instant::now();
        let mask = s.mask(&frame).expect("mask");
        eprintln!("mask in {:?}, channels last {}", started.elapsed(), s.channels_last);
        assert_eq!(mask.len(), IN_W * IN_H);
        assert!(mask.iter().all(|v| (0.0..=1.0).contains(v)));
        let out = apply(&frame, &mask);
        assert_eq!(out.rgba.len(), frame.rgba.len());
    }
}
