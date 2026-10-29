use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
use serde::Serialize;

use crate::storage::Database;

const FFMPEG_ENV: &str = env!("FLOWCAPTURE_FFMPEG");

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoReadyEvent {
    pub session_id: String,
    pub video_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullVideoReadyEvent {
    pub session_id: String,
    pub full_video_path: String,
}

pub fn resolve_ffmpeg() -> Option<PathBuf> {
    for candidate in sidecar_ffmpeg_paths() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if !FFMPEG_ENV.is_empty() {
        let bundled = PathBuf::from(FFMPEG_ENV);
        if bundled.is_file() {
            return Some(bundled);
        }
    }

    if crate::process_util::background_command("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Some(PathBuf::from("ffmpeg"));
    }

    for candidate in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }

    None
}

fn sidecar_ffmpeg_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in [
                "ffmpeg",
                // Installed Windows builds (Tauri bundle, Velopack) ship the sidecar as ffmpeg.exe.
                "ffmpeg.exe",
                "ffmpeg-aarch64-apple-darwin",
                "ffmpeg-x86_64-apple-darwin",
                "ffmpeg-x86_64-pc-windows-msvc.exe",
                "ffmpeg-aarch64-unknown-linux-gnu",
                "ffmpeg-x86_64-unknown-linux-gnu",
            ] {
                paths.push(dir.join(name));
            }
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        if let Ok(target) = std::env::var("TARGET") {
            paths.push(
                PathBuf::from(manifest_dir)
                    .join("binaries")
                    .join(format!("ffmpeg-{target}")),
            );
        }
    }

    paths
}

/// Videos being encoded or finalized after a recording: closing the app now would lose them
/// (and they would never reach the sessions folder).
static ACTIVE_VIDEO_JOBS: AtomicUsize = AtomicUsize::new(0);

/// Counts a video job from its creation (before the thread starts) to its end.
struct VideoJob;

impl VideoJob {
    fn start() -> Self {
        ACTIVE_VIDEO_JOBS.fetch_add(1, Ordering::SeqCst);
        VideoJob
    }
}

impl Drop for VideoJob {
    fn drop(&mut self) {
        ACTIVE_VIDEO_JOBS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn active_video_jobs() -> usize {
    ACTIVE_VIDEO_JOBS.load(Ordering::SeqCst)
}

pub fn spawn_session_video_encode(
    db: Arc<Database>,
    app: AppHandle,
    session_id: String,
    duration_secs: i64,
) {
    let job = VideoJob::start();
    thread::spawn(move || {
        let _job = job;
        let session_dir = db.session_dir(&session_id);
        match encode_session_video(&session_dir, duration_secs) {
            Ok(Some(path)) => {
                let video_path = path.to_string_lossy().to_string();
                if db
                    .update_session_video_path(&session_id, &video_path)
                    .is_ok()
                {
                    let _ = app.emit(
                        "video-ready",
                        VideoReadyEvent {
                            session_id,
                            video_path,
                        },
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("session video encode failed for {session_id}: {err:#}");
            }
        }
    });
}

pub fn spawn_session_full_video_finalize(
    db: Arc<Database>,
    app: AppHandle,
    session_id: String,
    audio_path: Option<PathBuf>,
) {
    let job = VideoJob::start();
    thread::spawn(move || {
        let _job = job;
        let session_dir = db.session_dir(&session_id);
        let temp_video = session_dir.join("video/recording_hd_temp.mp4");
        let final_video = session_dir.join("video/recording_hd.mp4");
        if !temp_video.is_file() && !final_video.is_file() {
            return;
        }

        let audio = audio_path.or_else(|| {
            let record_audio = db
                .get_setting("record_audio")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true);

            if record_audio {
                for _ in 0..15 {
                    if let Some(p) = find_session_audio(&session_dir) {
                        return Some(p);
                    }
                    thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            find_session_audio(&session_dir)
        });

        match crate::recorder::full_video::finalize_hd_video(&session_dir, audio.as_deref()) {
            Ok(Some(path)) => {
                let full_video_path = path.to_string_lossy().to_string();
                if db
                    .update_session_full_video_path(&session_id, &full_video_path)
                    .is_ok()
                {
                    let _ = app.emit(
                        "full-video-ready",
                        FullVideoReadyEvent {
                            session_id,
                            full_video_path,
                        },
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("session full video finalize failed for {session_id}: {err:#}");
            }
        }
    });
}

pub fn encode_session_video(session_dir: &Path, duration_secs: i64) -> Result<Option<PathBuf>> {
    let output = session_dir.join("video/recording.mp4");
    let frames_dir = session_dir.join("video/frames");
    if output.is_file() && output.metadata().is_ok_and(|meta| meta.len() > 1024) {
        // Leftover frames from sessions encoded by older versions.
        let _ = std::fs::remove_dir_all(&frames_dir);
        return Ok(Some(output));
    }

    if !frames_dir.is_dir() {
        return Ok(None);
    }

    let audio_file = find_session_audio(session_dir);
    let duration = duration_secs.max(1) as f64;
    if encode_frames_to_mp4(&frames_dir, audio_file.as_deref(), &output, duration)? {
        // The PNG frames are only the encoder input: once the MP4 exists they are dead weight
        // (hundreds of full-resolution images per session).
        let _ = std::fs::remove_dir_all(&frames_dir);
        Ok(Some(output))
    } else {
        Ok(None)
    }
}

pub fn find_session_audio(session_dir: &Path) -> Option<PathBuf> {
    let audio_dir = session_dir.join("audio");
    for ext in ["webm", "wav", "mp3", "ogg", "m4a"] {
        let path = audio_dir.join(format!("recording.{ext}"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn encode_frames_to_mp4(
    frames_dir: &Path,
    audio_path: Option<&Path>,
    output_path: &Path,
    duration_secs: f64,
) -> Result<bool> {
    let (frame_count, input_pattern) = detect_frame_sequence(frames_dir)?;
    if frame_count == 0 {
        return Ok(false);
    }

    let Some(ffmpeg) = resolve_ffmpeg() else {
        return Ok(false);
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let fps = (frame_count as f64 / duration_secs.max(1.0)).clamp(1.0, 30.0);
    let fps_arg = format!("{fps:.3}");

    let mut cmd = crate::process_util::background_command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-start_number",
        "0",
        "-framerate",
        &fps_arg,
        "-i",
        &input_pattern,
    ]);

    if let Some(audio) = audio_path {
        cmd.args([
            "-i",
            &audio.to_string_lossy(),
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-shortest",
        ]);
    }

    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
        &output_path.to_string_lossy(),
    ]);

    let status = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .context("failed to run ffmpeg")?;

    if !status.success() {
        return Ok(false);
    }

    Ok(output_path.is_file() && output_path.metadata().is_ok_and(|meta| meta.len() > 1024))
}

fn detect_frame_sequence(frames_dir: &Path) -> Result<(usize, String)> {
    for ext in ["jpg", "jpeg", "png"] {
        let first = frames_dir.join(format!("frame_000000.{ext}"));
        if first.is_file() {
            let count = count_numbered_frames(frames_dir, ext);
            if count > 0 {
                return Ok((
                    count,
                    frames_dir
                        .join(format!("frame_%06d.{ext}"))
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
        }
    }

    Ok((0, String::new()))
}

fn count_numbered_frames(frames_dir: &Path, extension: &str) -> usize {
    (0..)
        .map(|index| frames_dir.join(format!("frame_{index:06}.{extension}")))
        .take_while(|path| path.is_file())
        .count()
}

pub fn resolve_session_mp4(session_dir: &Path, video_path: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = video_path {
        let candidate = PathBuf::from(path);
        if candidate.extension().is_some_and(|ext| ext == "mp4") && candidate.is_file() {
            return Some(candidate);
        }
    }

    let default_mp4 = session_dir.join("video/recording.mp4");
    if default_mp4.is_file() {
        return Some(default_mp4);
    }

    None
}

pub fn ensure_session_mp4(
    session_dir: &Path,
    video_path: Option<&str>,
    duration_secs: i64,
) -> Result<Option<PathBuf>> {
    if let Some(path) = resolve_session_mp4(session_dir, video_path) {
        return Ok(Some(path));
    }

    encode_session_video(session_dir, duration_secs)
}
