//! Voice-note and video-note capture built on `waterkit-audio` /
//! `waterkit-camera` / `waterkit-codec` / `waterkit-video-container` +
//! `opus-pure`.
//!
//! WaterUI ships no ready-made capture *view* (no `CameraPreview` widget), so
//! the camera preview here is hand-wired: frames are read back over a private
//! wgpu device into RGBA and pushed into a `reactive_image` view. Desktop
//! `Camera::recording` is `ControlUnsupported`, so video-note files are
//! assembled by hand: RGBA -> NV12 -> waterkit-codec H.264 -> VideoWriter mp4.
//!
//! `Camera::frames()` borrows the camera, so the camera lives on a dedicated
//! thread that owns both it and the frame stream; the UI talks to it through
//! channels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use waterkit_audio::AudioRecorder;
use waterkit_camera::{Camera, CameraConfig, PixelFormat};
use waterkit_codec::{CodecType, Encoder};
use waterkit_video_container::VideoWriter;
use futures_lite::StreamExt;

/// Voice notes record at 48 kHz mono and encode Opus frames of 20 ms.
const VOICE_SAMPLE_RATE: u32 = 48_000;
const VOICE_FRAME: usize = 960;
/// Video notes are square; Telegram caps the diameter at 640.
pub(crate) const VIDEO_NOTE_SIZE: u32 = 360;
const VIDEO_FPS: u32 = 30;

// ---------------------------------------------------------------------------
// Voice notes
// ---------------------------------------------------------------------------

/// A live voice recording: the recorder plus the task draining its stream.
pub struct VoiceCapture {
    recorder: AudioRecorder,
    samples: Arc<Mutex<Vec<f32>>>,
    started: Instant,
}

/// Snapshot of a finished voice recording.
pub struct VoiceTake {
    /// Bytes of the OGG/Opus file.
    pub data: Vec<u8>,
    /// Whole seconds of audio.
    pub duration: i32,
    /// Packed 5-bit waveform, base64-encoded for TDLib's `bytes` JSON field.
    pub waveform: String,
}

/// Start recording from the default microphone.
///
/// Errors are returned verbatim (`DeviceNotFound`/`OpenFailed` on a machine
/// with no input device) so the UI can show exactly what the hardware did.
pub fn start_voice() -> Result<VoiceCapture, String> {
    let mut recorder = AudioRecorder::new()
        .sample_rate(VOICE_SAMPLE_RATE)
        .channels(1)
        .build()
        .map_err(|e| e.to_string())?;
    pollster::block_on(recorder.start()).map_err(|e| e.to_string())?;
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        // `stream()` is `'static` (`use<>`): spawn a plain collector thread so
        // capture survives independently of any view lifecycle.
        let sink = samples.clone();
        let mut stream = Box::pin(recorder.stream());
        std::thread::spawn(move || {
            while let Some(buf) = futures_lite::future::block_on(stream.next()) {
                sink.lock().unwrap().extend_from_slice(buf.samples());
            }
        });
    }
    Ok(VoiceCapture {
        recorder,
        samples,
        started: Instant::now(),
    })
}

impl VoiceCapture {
    /// Stop the recorder and encode captured samples to OGG/Opus bytes.
    pub fn finish(mut self) -> Result<VoiceTake, String> {
        pollster::block_on(self.recorder.stop()).map_err(|e| e.to_string())?;
        let samples = std::mem::take(&mut *self.samples.lock().unwrap());
        let duration = (samples.len() as u64 / u64::from(VOICE_SAMPLE_RATE)) as i32;
        let data = encode_opus_ogg(&samples, VOICE_SAMPLE_RATE)?;
        let waveform = encode_waveform(&samples, VOICE_SAMPLE_RATE);
        Ok(VoiceTake {
            data,
            duration,
            waveform,
        })
    }

    /// Seconds elapsed so far.
    pub fn elapsed(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// Encode mono f32 samples into an Ogg Opus container.
fn encode_opus_ogg(samples: &[f32], input_rate: u32) -> Result<Vec<u8>, String> {
    use opus_pure::{Application, OggOpusWriter, OpusEncoder, OpusHead};
    let mut enc =
        OpusEncoder::new(VOICE_SAMPLE_RATE as i32, 1, Application::Voip)
            .map_err(|e| e.to_string())?;
    let head = OpusHead::for_encoder(&enc, input_rate);
    let mut out = Vec::new();
    {
        let mut writer =
            OggOpusWriter::new(&mut out, head).map_err(|e| e.to_string())?;
        let mut packet = [0u8; opus_pure::MAX_PACKET_BYTES];
        for frame in samples.chunks(VOICE_FRAME) {
            if frame.len() < VOICE_FRAME {
                break;
            }
            let n = enc
                .encode(frame, VOICE_FRAME, &mut packet)
                .map_err(|e| e.to_string())?;
            writer
                .write_packet_with_duration(&packet[..n], VOICE_FRAME as u32)
                .map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// 100-bar 5-bit waveform (TDLib `bytes` ⇒ base64 string).
fn encode_waveform(samples: &[f32], _rate: u32) -> String {
    use base64::Engine;
    const BARS: usize = 100;
    if samples.is_empty() {
        return String::new();
    }
    let per_bar = (samples.len() / BARS).max(1);
    let mut bytes: Vec<u8> = Vec::with_capacity(BARS * 5 / 8 + 1);
    let mut acc: u64 = 0;
    let mut acc_bits = 0u32;
    for i in 0..BARS {
        let start = i * per_bar;
        let end = (start + per_bar).min(samples.len());
        let rms = (samples[start..end]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / (end - start) as f32)
            .sqrt();
        let v = (rms * 64.0).clamp(0.0, 31.0) as u8;
        acc |= u64::from(v) << acc_bits;
        acc_bits += 5;
        while acc_bits >= 8 {
            bytes.push((acc & 0xff) as u8);
            acc >>= 8;
            acc_bits -= 8;
        }
    }
    if acc_bits > 0 {
        bytes.push((acc & 0xff) as u8);
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------
// Video notes
// ---------------------------------------------------------------------------

enum CamCmd {
    StartRecording,
    /// Finish encoding and reply with the mp4 path + duration + jpeg thumb.
    StopAndEncode {
        path: std::path::PathBuf,
        reply: mpsc::Sender<Result<(i32, Vec<u8>), String>>,
    },
    Close,
}

/// Frames travel from the camera thread to the UI as square-cropped RGBA.
pub struct VideoNoteCapture {
    cmd_tx: mpsc::Sender<CamCmd>,
    preview_rx: mpsc::Receiver<Vec<u8>>,
    #[allow(dead_code)] // exposed for tests asserting the square size
    pub size: u32,
    started: Instant,
    recording: bool,
}

/// Open the default camera; spawn its frame pump thread.
///
/// Returns `Err` with the exact reason (`CameraError::NotFound` on machines
/// with no webcam) so callers can surface it verbatim.
pub fn open_camera() -> Result<VideoNoteCapture, String> {
    if Camera::list().map_err(|e| e.to_string())?.is_empty() {
        return Err("no camera found".into());
    }
    let (cmd_tx, cmd_rx) = mpsc::channel::<CamCmd>();
    let (prev_tx, prev_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || camera_thread(cmd_rx, prev_tx));
    Ok(VideoNoteCapture {
        cmd_tx,
        preview_rx: prev_rx,
        size: VIDEO_NOTE_SIZE,
        started: Instant::now(),
        recording: false,
    })
}

impl VideoNoteCapture {
    /// Begin collecting frames for the recording.
    pub fn start_recording(&mut self) {
        self.started = Instant::now();
        self.recording = true;
        let _ = self.cmd_tx.send(CamCmd::StartRecording);
    }

    /// Seconds elapsed in the current recording.
    pub fn elapsed(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Latest square-cropped preview frame (RGBA `size`×`size`), if queued.
    pub fn next_preview(&self) -> Option<Vec<u8>> {
        let mut last = None;
        while let Ok(f) = self.preview_rx.try_recv() {
            last = Some(f);
        }
        last
    }

    /// Stop recording, encode, and return `(duration_secs, thumbnail_jpeg)`.
    /// The mp4 file is written at `path`.
    pub fn finish(&self, path: &std::path::Path) -> Result<(i32, Vec<u8>), String> {
        let (tx, rx) = mpsc::channel();
        let _ = self.cmd_tx.send(CamCmd::StopAndEncode {
            path: path.to_path_buf(),
            reply: tx,
        });
        rx.recv()
            .unwrap_or_else(|_| Err("camera thread exited".into()))
    }
}

impl Drop for VideoNoteCapture {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(CamCmd::Close);
    }
}

/// Create a wgpu device used solely for camera frames + CPU readback.
fn request_gpu() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(
        &wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            ..Default::default()
        },
    ))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("watergram-camera"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .map_err(|e| format!("device request failed: {e}"))
}

fn camera_thread(cmd_rx: mpsc::Receiver<CamCmd>, prev_tx: mpsc::Sender<Vec<u8>>) {
    let (device, queue) = match request_gpu() {
        Ok(dq) => dq,
        Err(_) => return,
    };
    let device = std::sync::Arc::new(device);
    let queue = std::sync::Arc::new(queue);
    let config = CameraConfig {
        format: PixelFormat::Rgba8,
        ..CameraConfig::default()
    };
    let camera = match pollster::block_on(Camera::open(
        &Camera::list().ok().and_then(|c| c.first().map(|i| i.id.clone()))
            .unwrap_or_default(),
        config,
        device.clone(),
        queue.clone(),
    )) {
        Ok(c) => c,
        Err(_) => return,
    };
    let (w, h) = (camera.resolution().width, camera.resolution().height);
    let mut stream = std::pin::pin!(camera.frames());
    let recording = AtomicBool::new(false);
    let mut recorded: Vec<(Vec<u8>, Duration)> = Vec::new();
    let mut last_frame: Option<(Vec<u8>, Duration)> = None;
    loop {
        // Drain pending commands first (non-blocking).
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                CamCmd::StartRecording => {
                    recorded.clear();
                    recording.store(true, Ordering::Relaxed);
                }
                CamCmd::StopAndEncode { path, reply } => {
                    recording.store(false, Ordering::Relaxed);
                    let res = finish_video_note(&recorded, &path);
                    let thumb = match last_frame.as_ref() {
                        Some((f, _)) => rgba_to_jpeg(f, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE),
                        None => Err("no preview frame for thumbnail".into()),
                    };
                    let _ = reply.send(res.and_then(|d| thumb.map(|t| (d, t))));
                    return; // session over; camera drops here
                }
                CamCmd::Close => return,
            }
        }
        // Wait for the next frame; map commands while blocked would need a
        // select, so rely on camera frame rate to wake us (~30fps).
        let frame = match futures_lite::future::block_on(stream.next()) {
            Some(f) => f,
            None => return,
        };
        if let Some(rgba) = readback_rgba(&device, &queue, &frame) {
            let sq = crop_square_rgba(&rgba, frame.width(), frame.height(), VIDEO_NOTE_SIZE);
            if recording.load(Ordering::Relaxed) {
                recorded.push((sq.clone(), frame.timestamp()));
            }
            last_frame = Some((sq.clone(), frame.timestamp()));
            let _ = prev_tx.send(sq);
        }
        let _ = (w, h);
    }
}

fn finish_video_note(
    frames: &[(Vec<u8>, Duration)],
    path: &std::path::Path,
) -> Result<i32, String> {
    if frames.is_empty() {
        return Err("no frames captured".into());
    }
    let duration = (frames.len() as u64 / u64::from(VIDEO_FPS)).max(1) as i32;
    encode_video_note(frames, path)?;
    Ok(duration)
}

/// Read a camera frame texture back to RGBA bytes on the CPU.
fn readback_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    frame: &waterkit_camera::Frame,
) -> Option<Vec<u8>> {
    let (w, h) = (frame.width(), frame.height());
    let bpp = 4u32;
    let unpadded = w * bpp;
    let padded = unpadded.div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cam-readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        frame.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(2)),
        })
        .ok()?;
    rx.recv().ok()?.ok()?;
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((unpadded * h) as usize);
    for row in 0..h {
        let s = (row * padded) as usize;
        out.extend_from_slice(&data[s..s + unpadded as usize]);
    }
    drop(data);
    buf.unmap();
    Some(out)
}

/// Center-crop to square and nearest-neighbor downscale to `out` px.
fn crop_square_rgba(rgba: &[u8], w: u32, h: u32, out: u32) -> Vec<u8> {
    let side = w.min(h);
    let x0 = (w - side) / 2;
    let y0 = (h - side) / 2;
    let mut dst = Vec::with_capacity((out * out * 4) as usize);
    for y in 0..out {
        let sy = y0 + y * side / out;
        for x in 0..out {
            let sx = x0 + x * side / out;
            let i = ((sy * w + sx) * 4) as usize;
            dst.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    dst
}

/// RGBA -> NV12 (BT.601 limited range), for `Encoder::encode_nv12`.
fn rgba_to_nv12(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let wu = w as usize;
    let hu = h as usize;
    let mut y = Vec::with_capacity(wu * hu);
    let mut uv = Vec::with_capacity(wu * hu / 2);
    for j in 0..hu {
        for i in 0..wu {
            let p = (j * wu + i) * 4;
            let (r, g, b) = (
                rgba[p] as f32,
                rgba[p + 1] as f32,
                rgba[p + 2] as f32,
            );
            let yy = 0.257f32.mul_add(r, 0.504f32.mul_add(g, 0.098f32.mul_add(b, 16.0)));
            y.push(yy.clamp(16.0, 235.0) as u8);
        }
    }
    for j in (0..hu).step_by(2) {
        for i in (0..wu).step_by(2) {
            let p = (j * wu + i) * 4;
            let (r, g, b) = (
                rgba[p] as f32,
                rgba[p + 1] as f32,
                rgba[p + 2] as f32,
            );
            let u =
                (-0.148f32).mul_add(r, (-0.291f32).mul_add(g, 0.439f32.mul_add(b, 128.0)));
            let v = 0.439f32.mul_add(
                r,
                (-0.368f32).mul_add(g, (-0.071f32).mul_add(b, 128.0)),
            );
            uv.push(u.clamp(16.0, 240.0) as u8);
            uv.push(v.clamp(16.0, 240.0) as u8);
        }
    }
    let mut out = y;
    out.extend_from_slice(&uv);
    out
}

/// Encode square RGBA frames (VIDEO_NOTE_SIZE) to an H.264 mp4 at `path`.
fn encode_video_note(
    frames: &[(Vec<u8>, Duration)],
    path: &std::path::Path,
) -> Result<(), String> {
    let d = VIDEO_NOTE_SIZE;
    let mut encoder = Encoder::new(CodecType::H264, d, d).map_err(|e| e.to_string())?;
    let mut writer = VideoWriter::new(
        path,
        d,
        d,
        VIDEO_FPS,
        waterkit_video_container::MuxerCodecType::H264,
    )
    .map_err(|e| e.to_string())?;
    if let Some(cfg) = encoder.codec_config() {
        writer.set_codec_config(cfg);
    }
    for (i, (rgba, _ts)) in frames.iter().enumerate() {
        let nv12 = rgba_to_nv12(rgba, d, d);
        let mut stream = encoder.encode_nv12(&nv12);
        let chunk: Vec<u8> = {
            let mut v = Vec::new();
            for b in stream.by_ref().flatten() {
                v.extend_from_slice(&b);
            }
            v
        };
        if !chunk.is_empty() {
            writer
                .write_sample(&chunk, i == 0)
                .map_err(|e| e.to_string())?;
        }
    }
    writer.finish().map_err(|e| e.to_string())
}

fn rgba_to_jpeg(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or_else(|| "thumbnail buffer size mismatch".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Number of cameras the host reports (0 on a machine with no webcam).
#[allow(dead_code)] // used by the waterui test in lib.rs
pub(crate) fn camera_count() -> usize {
    Camera::list().map(|c| c.len()).unwrap_or(0)
}

/// Whether a default audio input device is present.
#[allow(dead_code)] // used by the waterui test in lib.rs
pub(crate) fn has_microphone() -> bool {
    AudioRecorder::list_devices()
        .map(|d| !d.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// NV12 planes: luma w*h then a quarter-sized interleaved UV plane.
    #[test]
    fn nv12_layout() {
        let rgba = vec![128u8; 16 * 16 * 4];
        let nv12 = rgba_to_nv12(&rgba, 16, 16);
        assert_eq!(nv12.len(), 16 * 16 + (16 * 16) / 2);
        // 0.5 grey → Y should sit near the midpoint of BT.601 limited range.
        let y = nv12[0];
        assert!((90..=170).contains(&y), "Y={y}");
    }

    /// Center-crop produces a square, nearest-neighbour downscale keeps size.
    #[test]
    fn crop_square() {
        let rgba = vec![7u8; 8 * 4 * 4];
        let sq = crop_square_rgba(&rgba, 8, 4, 2);
        assert_eq!(sq.len(), 2 * 2 * 4);
        assert_eq!(sq[0], 7);
    }

    /// Waveform is base64 of 100 5-bit bars packed into bytes.
    #[test]
    fn waveform_encodes_100_bars() {
        let samples: Vec<f32> = (0..4800)
            .map(|i| (i as f32 / 480.0).sin() * 0.5)
            .collect();
        let w = encode_waveform(&samples, VOICE_SAMPLE_RATE);
        assert!(!w.is_empty());
        let raw = base64::prelude::BASE64_STANDARD.decode(&w).unwrap();
        assert_eq!(raw.len(), 500usize.div_ceil(8));
    }

    /// Opus/Ogg encoder produces a non-empty Ogg stream starting at "OggS".
    #[test]
    fn opus_ogg_container() {
        let samples: Vec<f32> = (0..960 * 10)
            .map(|i| (i as f32 / 96.0).sin() * 0.3)
            .collect();
        let ogg = encode_opus_ogg(&samples, VOICE_SAMPLE_RATE).unwrap();
        assert!(ogg.len() > 100);
        assert_eq!(&ogg[..4], b"OggS");
    }
}
