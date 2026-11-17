//! The printer's camera. Creality's optional camera, and the usual Klipper set-up, serve
//! an MJPEG stream (a never-ending HTTP response of JPEG pictures). We try the common
//! addresses in turn and keep the newest picture.

use super::Wake;
use eframe::egui::ColorImage;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// (port, path) pairs tried in order.
pub const STREAM_PATHS: [(u16, &str); 4] = [
    (8080, "/?action=stream"),
    (80, "/webcam/?action=stream"),
    (4408, "/webcam/?action=stream"),
    (4409, "/webcam/?action=stream"),
];
const MAX_BUFFER: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum CameraState {
    Looking,
    Streaming(String),
    NotFound,
}

pub struct CameraFeed {
    pub state: Arc<Mutex<CameraState>>,
    /// Frame number and picture.
    pub frame: Arc<Mutex<Option<(u64, ColorImage)>>>,
    stop: Arc<AtomicBool>,
}

impl CameraFeed {
    pub fn start(host: String, wake: Wake) -> Self {
        let state = Arc::new(Mutex::new(CameraState::Looking));
        let frame = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, fr, sp) = (state.clone(), frame.clone(), stop.clone());
        std::thread::spawn(move || {
            for (port, path) in STREAM_PATHS {
                if sp.load(Ordering::Relaxed) {
                    return;
                }
                let Ok((stream, leftover)) = open_stream(&host, port, path) else {
                    continue;
                };
                set(
                    &st,
                    CameraState::Streaming(format!("http://{host}:{port}{path}")),
                );
                wake();
                read_frames(stream, leftover, &fr, &sp, &wake);
                break;
            }
            // No camera answered, or its stream ended: the page offers "Try again".
            if !sp.load(Ordering::Relaxed) {
                set(&st, CameraState::NotFound);
                wake();
            }
        });
        Self { state, frame, stop }
    }

    pub fn state(&self) -> CameraState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(CameraState::NotFound)
    }
}

impl Drop for CameraFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn set(state: &Mutex<CameraState>, s: CameraState) {
    if let Ok(mut g) = state.lock() {
        *g = s;
    }
}

/// Sends a GET and checks for a 200 multipart answer. Returns the socket and any body
/// bytes that arrived with the headers.
fn open_stream(host: &str, port: u16, path: &str) -> Result<(TcpStream, Vec<u8>), String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(1500))
        .map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {host}:{port}\r\nUser-Agent: PrintCraft\r\nAccept: */*\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = s.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 16 * 1024 {
            return Err("headers too long".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let ok = head.starts_with("http/1.") && head.split_whitespace().nth(1) == Some("200");
    if !ok || !head.contains("multipart/x-mixed-replace") {
        return Err("not a camera stream".into());
    }
    Ok((s, buf[header_end..].to_vec()))
}

fn read_frames(
    mut s: TcpStream,
    mut buf: Vec<u8>,
    frame: &Mutex<Option<(u64, ColorImage)>>,
    stop: &AtomicBool,
    wake: &Wake,
) {
    let mut chunk = vec![0u8; 64 * 1024];
    let mut count = 0u64;
    while !stop.load(Ordering::Relaxed) {
        while let Some(jpeg) = take_jpeg(&mut buf) {
            if let Ok(img) = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                count += 1;
                if let Ok(mut f) = frame.lock() {
                    *f = Some((
                        count,
                        ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    ));
                }
                wake();
            }
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.len() > MAX_BUFFER {
            buf.clear();
        }
    }
}

/// Removes and returns the first complete JPEG (FF D8 … FF D9) in `buf`, dropping what
/// came before it (multipart headers and boundaries).
pub fn take_jpeg(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    let end = buf[start + 2..]
        .windows(2)
        .position(|w| w == [0xFF, 0xD9])?
        + start
        + 4;
    let jpeg = buf[start..end].to_vec();
    buf.drain(..end);
    Some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_mjpeg_stream_into_pictures() {
        let mut buf = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
        buf.extend_from_slice(&[0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]);
        buf.extend_from_slice(b"\r\n--frame\r\n\r\n");
        buf.extend_from_slice(&[0xFF, 0xD8, 9]);
        let first = take_jpeg(&mut buf).unwrap();
        assert_eq!(first, vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]);
        assert!(
            take_jpeg(&mut buf).is_none(),
            "second picture isn't complete yet"
        );
        buf.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(
            take_jpeg(&mut buf).unwrap(),
            vec![0xFF, 0xD8, 9, 0xFF, 0xD9]
        );
    }
}
