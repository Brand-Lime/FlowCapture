use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, RgbaImage, imageops::FilterType};
use xcap::Monitor;

use super::traits::ScreenRecorder;
use crate::thread_util::join_thread_with_timeout;

const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(100);
const RECORDER_JOIN_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_FRAME_WIDTH: u32 = 1920;
const JPEG_QUALITY: u8 = 82;

pub struct MacScreenRecorder {
    recording: AtomicBool,
    output_dir: Mutex<Option<PathBuf>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    stop_flag: Arc<AtomicBool>,
}

impl Default for MacScreenRecorder {
    fn default() -> Self {
        Self {
            recording: AtomicBool::new(false),
            output_dir: Mutex::new(None),
            handle: Mutex::new(None),
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ScreenRecorder for MacScreenRecorder {
    fn start(&mut self, output_dir: PathBuf) -> Result<PathBuf> {
        if self.recording.load(Ordering::SeqCst) {
            anyhow::bail!("è già in corso una registrazione");
        }

        std::fs::create_dir_all(&output_dir)?;
        let frames_dir = output_dir.join("frames");
        std::fs::create_dir_all(&frames_dir)?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.stop_flag = stop_flag.clone();
        self.recording.store(true, Ordering::SeqCst);
        *self.output_dir.lock() = Some(output_dir.clone());

        let handle = thread::spawn(move || {
            let mut frame_index = 0u64;
            while !stop_flag.load(Ordering::SeqCst) {
                let frame_started = Instant::now();

                if let Ok(monitors) = Monitor::all() {
                    if let Some(monitor) = monitors.into_iter().next() {
                        let path = frames_dir.join(format!("frame_{frame_index:06}.jpg"));
                        if let Ok(image) = monitor.capture_image() {
                            if save_video_frame(&image, &path).is_ok() {
                                frame_index += 1;
                            }
                        }
                    }
                }

                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }

                let elapsed = frame_started.elapsed();
                if elapsed < TARGET_FRAME_INTERVAL {
                    thread::sleep(TARGET_FRAME_INTERVAL - elapsed);
                }
            }
        });

        *self.handle.lock() = Some(handle);
        Ok(output_dir)
    }

    fn stop(&mut self) -> Result<Option<PathBuf>> {
        if !self.recording.load(Ordering::SeqCst) {
            return Ok(None);
        }

        self.request_stop();
        if let Some(handle) = self.handle.lock().take() {
            if !join_thread_with_timeout(handle, RECORDER_JOIN_TIMEOUT) {
                eprintln!("FlowCapture: video frame capture thread did not stop within timeout");
            }
        }
        self.recording.store(false, Ordering::SeqCst);
        self.output_dir.lock().take();

        Ok(None)
    }

    fn request_stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

fn save_video_frame(image: &RgbaImage, path: &Path) -> Result<()> {
    let mut rgb = image::DynamicImage::ImageRgba8(image.clone()).into_rgb8();
    if rgb.width() > MAX_FRAME_WIDTH {
        let new_height =
            ((rgb.height() as f32 * MAX_FRAME_WIDTH as f32) / rgb.width() as f32).round() as u32;
        rgb = image::imageops::resize(&rgb, MAX_FRAME_WIDTH, new_height.max(1), FilterType::Triangle);
    }

    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    let mut encoder = JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY);
    encoder.encode(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        ExtendedColorType::Rgb8,
    )?;
    Ok(())
}
