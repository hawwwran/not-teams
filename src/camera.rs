//! The camera for video calls, through video4linux. A thread keeps the latest frame as
//! RGBA; the preview and the encoder each take it when they're ready, so neither can hold
//! up the other.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
const FPS: u32 = 15;

/// One picture, RGBA, with a counter so readers can tell a new frame from the last one.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub seq: u64,
    /// Whose picture it is: the sender's source id (the RTP CSRC the media processor puts
    /// on others' video), when known.
    pub source: Option<u64>,
}

/// The latest frame of a video source (the camera, or the other side's decoded video).
pub type Latest = Arc<Mutex<Option<Arc<Frame>>>>;

pub struct Camera {
    pub latest: Latest,
    stop: Arc<AtomicBool>,
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Opens the first camera and starts capturing. Fails quickly when there is none or it's
/// busy, so the call can go on without video.
/// `path` is the chosen camera (`/dev/video2`); empty for the first one.
pub fn open(latest: Latest, path: &str) -> Result<Camera, String> {
    if crate::brand::env("FAKE_CAMERA").is_some() {
        return Ok(test_pattern(latest));
    }
    let dev = if path.is_empty() { Device::new(0) } else { Device::with_path(path).or_else(|_| Device::new(0)) }
        .map_err(|e| format!("No camera found ({e})."))?;
    let mut fmt = dev.format().map_err(|e| format!("Camera: {e}"))?;
    fmt.width = WIDTH;
    fmt.height = HEIGHT;
    fmt.fourcc = FourCC::new(b"YUYV");
    let fmt = match dev.set_format(&fmt) {
        Ok(f) if f.fourcc == FourCC::new(b"YUYV") => f,
        _ => {
            let mut m = dev.format().map_err(|e| format!("Camera: {e}"))?;
            m.width = WIDTH;
            m.height = HEIGHT;
            m.fourcc = FourCC::new(b"MJPG");
            dev.set_format(&m).map_err(|e| format!("Camera: {e}"))?
        }
    };
    let _ = dev.set_params(&v4l::video::capture::Parameters::with_fps(FPS));
    let stop = Arc::new(AtomicBool::new(false));
    let (out, flag) = (latest.clone(), stop.clone());
    let (width, height, fourcc) = (fmt.width as usize, fmt.height as usize, fmt.fourcc);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("camera".into())
        .spawn(move || {
            let mut stream = match Stream::with_buffers(&dev, Type::VideoCapture, 4) {
                Ok(s) => {
                    let _ = ready_tx.send(Ok(()));
                    s
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("The camera is busy or unavailable ({e}).")));
                    return;
                }
            };
            let mut seq = 0u64;
            while !flag.load(Ordering::Relaxed) {
                let Ok((buf, _)) = stream.next() else { continue };
                let rgba = if fourcc == FourCC::new(b"YUYV") {
                    yuyv_to_rgba(buf, width, height)
                } else {
                    match image::load_from_memory_with_format(buf, image::ImageFormat::Jpeg) {
                        Ok(img) => img.to_rgba8().into_raw(),
                        Err(_) => continue,
                    }
                };
                if rgba.len() != width * height * 4 {
                    continue;
                }
                seq += 1;
                let (w, h, rgba) = widescreen(width, height, rgba);
                if let Ok(mut slot) = out.lock() {
                    *slot = Some(Arc::new(Frame { width: w, height: h, rgba, seq, source: None }));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "Camera thread failed.".to_string())??;
    Ok(Camera { latest, stop })
}

/// The middle 16:9 band of a taller picture (a 4:3 camera's 640×480 becomes 640×360). Teams
/// shows cameras as 16:9, and a 4:3 picture came out stretched on the others' screens.
fn widescreen(width: usize, height: usize, rgba: Vec<u8>) -> (usize, usize, Vec<u8>) {
    let h = (width * 9 / 16) & !1;
    if h == 0 || h >= height {
        return (width, height, rgba);
    }
    let top = ((height - h) / 2) * width * 4;
    (width, h, rgba[top..top + h * width * 4].to_vec())
}

/// Moving colour bars instead of the camera (FAKE_CAMERA), for testing video without
/// filming anyone.
fn test_pattern(latest: Latest) -> Camera {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let out = latest.clone();
    std::thread::spawn(move || {
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        let bars: [[u8; 3]; 7] = [[192, 192, 192], [192, 192, 0], [0, 192, 192], [0, 192, 0], [192, 0, 192], [192, 0, 0], [0, 0, 192]];
        let mut seq = 0u64;
        while !flag.load(Ordering::Relaxed) {
            seq += 1;
            let shift = (seq as usize * 8) % w;
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let c = if y > h * 3 / 4 { [(x * 255 / w) as u8; 3] } else { bars[((x + shift) % w) * 7 / w] };
                    rgba.extend([c[0], c[1], c[2], 255]);
                }
            }
            let (w2, h2, rgba) = widescreen(w, h, rgba);
            if let Ok(mut slot) = out.lock() {
                *slot = Some(Arc::new(Frame { width: w2, height: h2, rgba, seq, source: None }));
            }
            std::thread::sleep(std::time::Duration::from_millis(1000 / FPS as u64));
        }
    });
    Camera { latest, stop }
}

fn yuyv_to_rgba(buf: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for px in buf.chunks_exact(4).take(width * height / 2) {
        let (y0, u, y1, v) = (px[0] as f32, px[1] as f32 - 128.0, px[2] as f32, px[3] as f32 - 128.0);
        for y in [y0, y1] {
            let r = y + 1.402 * v;
            let g = y - 0.344 * u - 0.714 * v;
            let b = y + 1.772 * u;
            out.extend([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn cameras_are_sent_widescreen() {
        let (w, h, px) = super::widescreen(640, 480, vec![7; 640 * 480 * 4]);
        assert_eq!((w, h, px.len()), (640, 360, 640 * 360 * 4));
        let (w, h, _) = super::widescreen(1280, 720, vec![0; 1280 * 720 * 4]);
        assert_eq!((w, h), (1280, 720), "already 16:9");
    }

    #[test]
    fn grey_yuyv_is_grey() {
        let rgba = super::yuyv_to_rgba(&[128, 128, 128, 128], 2, 1);
        assert_eq!(rgba, vec![128, 128, 128, 255, 128, 128, 128, 255]);
    }
}
