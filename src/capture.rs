//! Voice-note and video-note capture built on `waterkit-audio` /
//! `waterkit-camera` / `waterkit-codec` / `waterkit-video-container` +
//! `opus-pure`.
//!
//! WaterUI ships no ready-made capture *view* (no `CameraPreview` widget), so
//! the camera preview is a `GpuContentView`: its `GpuContent` clones the
//! engine's wgpu `Device`/`Queue` into `Arc`s and opens the camera on them —
//! frames stay on the GPU and draw straight through `Frame::view`.
//! Desktop `Camera::recording` is `ControlUnsupported`
//! (water-rs/waterkit#86), so the encode tail is hand-wired: a compute pass
//! converts the camera texture to 360x360 NV12 and only that buffer is mapped
//! back for `waterkit-codec`'s CPU-side `encode_nv12` -> `VideoWriter` mp4.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use arc_swap::ArcSwapOption;
use futures::StreamExt;
use futures::channel::mpsc as fmpsc;
use futures::future::{self, Either};
use waterkit_audio::AudioRecorder;
use waterkit_camera::Camera;
use waterkit_codec::{CodecType, Encoder, EncoderProfile};
use waterkit_video_container::{MuxerCodecType, VideoWriter};
use waterui::Str;
use waterui::binding::Binding;
use waterui::graphics::gpu::{Context as GpuContext, Frame as GpuFrame, GpuContent, RedrawHandle};
// wgpu/bytemuck come through the facade re-exports so their versions can
// never split from the ones Hydrolysis drives (water-rs/waterui#1933).
use waterui::graphics::{bytemuck, wgpu};

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
    let mut enc = OpusEncoder::new(VOICE_SAMPLE_RATE as i32, 1, Application::Voip)
        .map_err(|e| e.to_string())?;
    let head = OpusHead::for_encoder(&enc, input_rate);
    let mut out = Vec::new();
    {
        let mut writer = OggOpusWriter::new(&mut out, head).map_err(|e| e.to_string())?;
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
        let rms =
            (samples[start..end].iter().map(|s| s * s).sum::<f32>() / (end - start) as f32).sqrt();
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
// Video notes — GPU-resident pipeline
// ---------------------------------------------------------------------------

/// Shared state between the video-note sheet's buttons and its `GpuContent`.
pub struct VideoNoteShared {
    /// Surface status line ("opening camera…", error text).
    pub status: Binding<Str>,
    /// Status events the content/capture side publishes; the sheet drains
    /// them on `GpuContentView::on_frame`. Taken once at mount.
    pub(crate) event_rx: Option<mpsc::Receiver<VideoNoteEvent>>,
    /// UI → render-owner recording commands.
    pub(crate) cmd_tx: mpsc::Sender<RecCmd>,
    /// One-shot result from the encoder thread.
    done_rx: Option<mpsc::Receiver<Result<VideoNoteDone, String>>>,
    /// Capture start for the elapsed label.
    started: Instant,
    /// Channel ends for the `GpuContent`; taken once when the sheet mounts
    /// it — every `open_video_note` installs a fresh shared, so a reopened
    /// sheet never shares state with the dropped one.
    pub(crate) inner: Option<VideoNoteInner>,
}

/// Status/error lines the capture + render side publishes; the UI drains
/// them on `on_frame`.
pub(crate) enum VideoNoteEvent {
    Status(String),
}

/// Recording commands the UI sends the render owner, which keeps the
/// encoder's frame sender outright — no shared lock.
pub(crate) enum RecCmd {
    /// Start a session: the given sender feeds `RecMsg`s to the encoder.
    Start(mpsc::Sender<RecMsg>),
    /// Finish the session (enqueues `RecMsg::Finish` after queued frames).
    Stop,
}

/// The `Send` channel ends a `GpuContent` takes ownership of.
pub(crate) struct VideoNoteInner {
    /// Publishes `VideoNoteEvent`s for the UI's `on_frame` drain.
    pub(crate) events: mpsc::Sender<VideoNoteEvent>,
    /// Receives UI recording commands on the render owner.
    pub(crate) cmd_rx: mpsc::Receiver<RecCmd>,
}

/// Result of a finished video-note encode.
pub struct VideoNoteDone {
    pub duration: i32,
    /// JPEG thumbnail of the last recorded frame; `None` when the session
    /// recorded zero frames (a legitimate state — not an encode failure).
    pub thumb: Option<Vec<u8>>,
}

pub(crate) enum RecMsg {
    Frame(Vec<u8>),
    Finish,
}

impl VideoNoteShared {
    fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        Self {
            status: Binding::container(Str::from("opening camera…")),
            event_rx: Some(event_rx),
            cmd_tx,
            done_rx: None,
            started: Instant::now(),
            inner: Some(VideoNoteInner {
                events: event_tx,
                cmd_rx,
            }),
        }
    }

    /// Hand the GPU session's channel ends to the sheet mounting the
    /// `GpuContentView`: `open_video_note` installs a fresh shared before
    /// `video_note_open` flips, and the ends are taken exactly once per
    /// mount. Anything else is a broken invariant, so it fails here rather
    /// than falling back.
    pub(crate) fn take_gpu_session(&mut self) -> (VideoNoteInner, mpsc::Receiver<VideoNoteEvent>) {
        let inner = self
            .inner
            .take()
            .expect("video-note GPU session taken without a fresh shared — open_video_note installs one per open");
        let event_rx = self
            .event_rx
            .take()
            .expect("video-note event drain taken without a fresh shared — open_video_note installs one per open");
        (inner, event_rx)
    }
}

// ---------------------------------------------------------------------------
// Video notes — GPU-resident pipeline
// ---------------------------------------------------------------------------
// waterui has no ready-made camera-preview view, so the sheet mounts a
// `GpuContentView` whose `GpuContent` clones the engine's wgpu Device/Queue
// into `Arc`s (both are `Clone` — the same approach as
// `examples/waterkit_camera_filters`) and hands them to
// `Camera::open_default` on a dedicated thread (`Camera` is not `Send`, so
// it cannot live inside the `Send`-bound content; textures, which are `Send`,
// cross back over a channel). Camera frames stay on the GPU the whole way
// and are drawn straight onto the layer through `Frame::view`.
//
// Recording is the one place a readback happens: `waterkit-codec`'s
// `Encoder::encode_nv12` takes CPU memory (VA-API import), so a compute pass
// center-crops + downscales the camera texture to 360x360 NV12 inside a
// storage buffer and only that plane pair (194 KB/frame vs 1.9 MB RGBA) is
// mapped back and fed to the encoder. Desktop `Camera::recording()` is
// `ControlUnsupported` — water-rs/waterkit#86 — so the mux/encode tail is
// hand-wired until it lands.

/// `GpuContent` rendering camera frames and feeding an H.264 encoder.
pub struct VideoNoteGpu {
    /// Camera textures — a single-slot handoff: the producer replaces the
    /// pending texture, so the newest always wins, at most one waits
    /// unprocessed, and GPU textures never queue up.
    frame_slot: Arc<ArcSwapOption<wgpu::Texture>>,
    latest: Option<Arc<wgpu::Texture>>,
    pipeline: Option<wgpu::RenderPipeline>,
    compute: Option<wgpu::ComputePipeline>,
    render_bgl: Option<wgpu::BindGroupLayout>,
    compute_bgl: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    crop_buf: Option<wgpu::Buffer>,
    params_buf: Option<wgpu::Buffer>,
    nv12_buf: Option<wgpu::Buffer>,
    staging: Option<wgpu::Buffer>,
    /// UI recording commands — render-thread owned.
    cmd_rx: mpsc::Receiver<RecCmd>,
    /// Active encoder session's frame sink — `Some` = recording.
    rec_tx: Option<mpsc::Sender<RecMsg>>,
    /// `map_async` results return here, keeping the map lifecycle
    /// render-owned (busy until unmap, errors surfaced to the UI).
    map_done_tx: mpsc::Sender<Result<(), String>>,
    map_done_rx: mpsc::Receiver<Result<(), String>>,
    map_in_flight: bool,
    /// Status events for the UI's `on_frame` drain.
    event_tx: mpsc::Sender<VideoNoteEvent>,
    /// Host wake handle — every producer event (frame, map result, status)
    /// requests a redraw instead of a continuous fake frame.
    redraw: Option<RedrawHandle>,
    /// Sheet-close cancellation raced against camera open/stream.
    cancel_tx: fmpsc::Sender<()>,
    cancel_rx: Option<fmpsc::Receiver<()>>,
}

/// Shader: fullscreen triangle sampling the centered square crop.
const PREVIEW_WGSL: &str = r#"
struct Crop { src_w: f32, src_h: f32, _p0: f32, _p1: f32 };
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> crop: Crop;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1., -1.), vec2<f32>(3., -1.), vec2<f32>(-1., 3.));
    var o: VOut;
    let xy = p[i];
    o.pos = vec4<f32>(xy, 0., 1.);
    o.uv = vec2<f32>((xy.x + 1.) * 0.5, (1. - xy.y) * 0.5);
    return o;
}

@fragment fn fs(in: VOut) -> @location(0) vec4<f32> {
    let m = min(crop.src_w, crop.src_h);
    let lo = vec2<f32>((crop.src_w - m) * 0.5 / crop.src_w,
                      (crop.src_h - m) * 0.5 / crop.src_h);
    let hi = vec2<f32>(m / crop.src_w, m / crop.src_h);
    return textureSample(tex, smp, lo + in.uv * hi);
}
"#;

/// Compute shader: center-crop + downscale to a square, RGBA -> NV12
/// (BT.601 limited). Each invocation writes a 4x2 block: two 4-pixel Y words
/// plus one interleaved UV word (U0 V0 U1 V1).
const NV12_WGSL: &str = r#"
struct Params { src_w: u32, src_h: u32, out: u32, _p: u32 };
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<uniform> p: Params;

fn y8(c: vec3<f32>) -> u32 {
    return u32(clamp(16.5 + 65.481 * c.r + 128.553 * c.g + 24.966 * c.b,
                     0.0, 255.0));
}
fn u8c(c: vec3<f32>) -> u32 {
    return u32(clamp(128.5 - 37.797 * c.r - 74.203 * c.g + 112.0 * c.b,
                     0.0, 255.0));
}
fn v8(c: vec3<f32>) -> u32 {
    return u32(clamp(128.5 + 112.0 * c.r - 93.786 * c.g - 18.214 * c.b,
                     0.0, 255.0));
}

fn sample_at(px: u32, py: u32) -> vec3<f32> {
    let side = min(p.src_w, p.src_h);
    let ox = (p.src_w - side) / 2u;
    let oy = (p.src_h - side) / 2u;
    let sx = min(ox + (px * side + p.out / 2u) / p.out, p.src_w - 1u);
    let sy = min(oy + (py * side + p.out / 2u) / p.out, p.src_h - 1u);
    return textureLoad(src_tex, vec2<u32>(sx, sy), 0).rgb;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bx = gid.x * 4u;
    let by = gid.y * 2u;
    if (bx >= p.out || by >= p.out) { return; }

    var w0 = y8(sample_at(bx,      by));
    w0 |= y8(sample_at(bx + 1u, by)) << 8;
    w0 |= y8(sample_at(bx + 2u, by)) << 16;
    w0 |= y8(sample_at(bx + 3u, by)) << 24;
    dst[(by * p.out + bx) / 4u] = w0;

    var w1 = y8(sample_at(bx,      by + 1u));
    w1 |= y8(sample_at(bx + 1u, by + 1u)) << 8;
    w1 |= y8(sample_at(bx + 2u, by + 1u)) << 16;
    w1 |= y8(sample_at(bx + 3u, by + 1u)) << 24;
    dst[((by + 1u) * p.out + bx) / 4u] = w1;

    let c0 = (sample_at(bx, by) + sample_at(bx + 1u, by)
            + sample_at(bx, by + 1u) + sample_at(bx + 1u, by + 1u)) * 0.25;
    let c1 = (sample_at(bx + 2u, by) + sample_at(bx + 3u, by)
            + sample_at(bx + 2u, by + 1u) + sample_at(bx + 3u, by + 1u)) * 0.25;
    let uv = u8c(c0) | (v8(c0) << 8) | (u8c(c1) << 16) | (v8(c1) << 24);
    dst[(p.out * p.out + (by / 2u) * p.out + bx) / 4u] = uv;
}
"#;

impl VideoNoteGpu {
    /// Wrap the sheet's channel ends into a `GpuContent`.
    pub fn new(inner: VideoNoteInner) -> Self {
        let (map_done_tx, map_done_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = fmpsc::channel(1);
        Self {
            frame_slot: Arc::new(ArcSwapOption::new(None)),
            latest: None,
            pipeline: None,
            compute: None,
            render_bgl: None,
            compute_bgl: None,
            sampler: None,
            crop_buf: None,
            params_buf: None,
            nv12_buf: None,
            staging: None,
            cmd_rx: inner.cmd_rx,
            rec_tx: None,
            map_done_tx,
            map_done_rx,
            map_in_flight: false,
            event_tx: inner.events,
            redraw: None,
            cancel_tx,
            cancel_rx: Some(cancel_rx),
        }
    }

    /// Publish a status line to the UI and wake the host to display it.
    fn send_status(&self, text: String) {
        let _ = self.event_tx.send(VideoNoteEvent::Status(text));
        if let Some(redraw) = self.redraw.as_ref() {
            redraw.request_redraw();
        }
    }

    fn build(&mut self, gpu: &GpuContext<'_>) {
        let device = gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("watergram-preview"),
            source: wgpu::ShaderSource::Wgsl(PREVIEW_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("watergram-preview-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("watergram-preview-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watergram-preview-pipe"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.render_bgl = Some(bgl);
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("watergram-preview-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        }));
        self.crop_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watergram-crop-uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // NV12 compute path.
        let cshader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("watergram-nv12"),
            source: wgpu::ShaderSource::Wgsl(NV12_WGSL.into()),
        });
        let cbgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("watergram-nv12-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let cpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("watergram-nv12-pl"),
            bind_group_layouts: &[Some(&cbgl)],
            immediate_size: 0,
        });
        self.compute = Some(
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("watergram-nv12-pipe"),
                layout: Some(&cpl),
                module: &cshader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            }),
        );
        self.compute_bgl = Some(cbgl);
        self.params_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watergram-nv12-params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let nv12_size = (VIDEO_NOTE_SIZE * VIDEO_NOTE_SIZE * 3 / 2) as u64;
        self.nv12_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watergram-nv12"),
            size: nv12_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.staging = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watergram-nv12-staging"),
            size: nv12_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    /// Take the newest camera texture, if one is pending.
    fn pull_frame(&mut self) {
        if let Some(texture) = self.frame_slot.swap(None) {
            self.latest = Some(texture);
        }
    }

    /// Convert the latest texture to NV12 on the GPU and map it back once.
    /// The single readback exists only because `Encoder::encode_nv12` wants
    /// CPU bytes — everything before it stays in VRAM.
    fn pump_recording(&mut self, frame: &GpuFrame) {
        let (Some(texture), Some(compute), Some(cbgl), Some(nv12), Some(staging)) = (
            self.latest.as_ref(),
            self.compute.as_ref(),
            self.compute_bgl.as_ref(),
            self.nv12_buf.as_ref(),
            self.staging.as_ref(),
        ) else {
            return;
        };
        if self.rec_tx.is_none() || self.map_in_flight {
            return;
        }
        let params: [u32; 4] = [texture.width(), texture.height(), VIDEO_NOTE_SIZE, 0];
        frame.queue.write_buffer(
            self.params_buf.as_ref().unwrap(),
            0,
            bytemuck::cast_slice(&params),
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watergram-nv12-bind"),
            layout: cbgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: nv12.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buf.as_ref().unwrap().as_entire_binding(),
                },
            ],
        });
        let mut enc = frame
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("watergram-nv12-enc"),
            });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("watergram-nv12-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(compute);
            pass.set_bind_group(0, &bind, &[]);
            // Each invocation covers a 4x2 block: ceil blocks per axis.
            pass.dispatch_workgroups(
                (VIDEO_NOTE_SIZE / 4).div_ceil(8),
                (VIDEO_NOTE_SIZE / 2).div_ceil(8),
                1,
            );
        }
        enc.copy_buffer_to_buffer(nv12, 0, staging, 0, nv12.size());
        frame.queue.submit([enc.finish()]);

        // The map stays in-flight until `drain_map_done` unmaps it — the
        // callback reports back over a channel so render state stays the
        // single owner instead of racing two atomics.
        self.map_in_flight = true;
        let done = self.map_done_tx.clone();
        let redraw = self.redraw.clone().unwrap();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |res| {
                let _ = done.send(res.map_err(|e| format!("{e}")));
                redraw.request_redraw();
            });
    }

    /// Apply UI recording commands; the render owner holds the encoder
    /// sender outright.
    fn drain_cmds(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                RecCmd::Start(tx) => self.rec_tx = Some(tx),
                RecCmd::Stop => {
                    if let Some(tx) = self.rec_tx.take() {
                        let _ = tx.send(RecMsg::Finish);
                    }
                }
            }
        }
    }

    /// Resolve the one outstanding map: success with an unreadable range is
    /// a device-contract violation and is surfaced, `map_async` errors go to
    /// the UI status channel, and the buffer is busy until actually unmapped.
    fn drain_map_done(&mut self) {
        let Ok(res) = self.map_done_rx.try_recv() else {
            return;
        };
        self.map_in_flight = false;
        let Some(staging) = self.staging.as_ref() else {
            return;
        };
        match res {
            Ok(()) => match staging.slice(..).get_mapped_range() {
                Ok(view) => {
                    let data = view.to_vec();
                    drop(view);
                    staging.unmap();
                    if let Some(tx) = self.rec_tx.as_ref() {
                        let _ = tx.send(RecMsg::Frame(data));
                    }
                }
                Err(err) => {
                    staging.unmap();
                    self.send_status(format!("video note readback: {err}"));
                }
            },
            Err(err) => self.send_status(format!("video note map: {err}")),
        }
    }

    /// Render the latest camera texture into the surface.
    fn draw(&mut self, frame: &mut GpuFrame) {
        let (Some(texture), Some(pipeline), Some(bgl), Some(sampler), Some(crop)) = (
            self.latest.as_ref(),
            self.pipeline.as_ref(),
            self.render_bgl.as_ref(),
            self.sampler.as_ref(),
            self.crop_buf.as_ref(),
        ) else {
            return;
        };
        let dims: [f32; 4] = [texture.width() as f32, texture.height() as f32, 0.0, 0.0];
        frame
            .queue
            .write_buffer(crop, 0, bytemuck::cast_slice(&dims));
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watergram-preview-bind"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: crop.as_entire_binding(),
                },
            ],
        });
        let mut enc = frame
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("watergram-preview-enc"),
            });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watergram-preview-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        frame.queue.submit([enc.finish()]);
    }
}
impl GpuContent for VideoNoteGpu {
    fn setup(&mut self, gpu: &GpuContext<'_>) {
        self.build(gpu);
        // Clone the surface's device/queue into Arcs — same pattern as
        // `examples/waterkit_camera_filters`; the camera then produces
        // textures on the very device that draws them. `setup` is sync and
        // `Camera` is not `Send`, so open + stream polling run on a dedicated
        // owner thread that forwards textures; a sheet-close cancel wins the
        // race even when `open_default`/`next` are still pending, and only
        // textures (which are `Send`) cross.
        let device = Arc::new(gpu.device.clone());
        let queue = Arc::new(gpu.queue.clone());
        self.redraw = Some(gpu.redraw.clone());
        let redraw = gpu.redraw.clone();
        let events = self.event_tx.clone();
        let mut cancel = self.cancel_rx.take().unwrap();
        let slot = self.frame_slot.clone();
        std::thread::spawn(move || {
            futures_lite::future::block_on(async move {
                let status = |text: String| {
                    let _ = events.send(VideoNoteEvent::Status(text));
                    redraw.request_redraw();
                };
                let camera = match future::select(
                    Box::pin(Camera::open_default(device, queue)),
                    cancel.next(),
                )
                .await
                {
                    Either::Left((Ok(camera), _)) => camera,
                    Either::Left((Err(e), _)) => {
                        status(format!("camera: {e}"));
                        return;
                    }
                    Either::Right(_) => return,
                };
                status(String::new());
                let mut stream = Box::pin(camera.frames());
                loop {
                    match future::select(stream.as_mut().next(), cancel.next()).await {
                        Either::Left((Some(frame), _)) => {
                            // Replace the pending texture: the render side
                            // always draws the newest; the producer never
                            // blocks and at most one texture waits.
                            slot.store(Some(Arc::new(frame.into_texture())));
                            redraw.request_redraw();
                        }
                        Either::Left((None, _)) => {
                            status("camera stream ended".into());
                            return;
                        }
                        Either::Right(_) => return,
                    }
                }
            });
        });
    }

    fn render(&mut self, frame: &mut GpuFrame<'_>) {
        self.drain_cmds();
        self.pull_frame();
        self.drain_map_done();
        self.pump_recording(frame);
        self.draw(frame);
        // Service map callbacks; a pending map requests the next frame so
        // its completion can drain, everything else is woken by events.
        let _ = frame.device.poll(wgpu::PollType::Poll);
        if self.map_in_flight {
            frame.request_redraw();
        }
    }
}

impl Drop for VideoNoteGpu {
    /// Sheet closed / view dropped — cancel wins the camera thread's race
    /// even mid-`open_default` or mid-`next`.
    fn drop(&mut self) {
        let _ = self.cancel_tx.try_send(());
    }
}

/// New shared state for a freshly opened video-note sheet.
pub(crate) fn new_video_note_shared() -> Rc<RefCell<VideoNoteShared>> {
    Rc::new(RefCell::new(VideoNoteShared::new()))
}

/// Start recording: spawn the encoder thread and hand its frame sink to
/// the render owner as a `RecCmd` session.
pub(crate) fn video_note_start_recording(shared: &Rc<RefCell<VideoNoteShared>>, path: PathBuf) {
    let (tx, rx) = mpsc::channel::<RecMsg>();
    let (dtx, drx) = mpsc::channel::<Result<VideoNoteDone, String>>();
    std::thread::spawn(move || recorder_thread(rx, dtx, path));
    let mut sh = shared.borrow_mut();
    sh.done_rx = Some(drx);
    sh.started = Instant::now();
    let _ = sh.cmd_tx.send(RecCmd::Start(tx));
}

/// Stop recording: enqueue `Finish` after the queued frames and return the
/// encoder's result receiver (callers poll it, never block the UI).
pub(crate) fn video_note_stop_recording(
    shared: &Rc<RefCell<VideoNoteShared>>,
) -> Option<mpsc::Receiver<Result<VideoNoteDone, String>>> {
    let mut sh = shared.borrow_mut();
    let _ = sh.cmd_tx.send(RecCmd::Stop);
    sh.done_rx.take()
}

/// Seconds elapsed since recording started.
pub(crate) fn video_note_elapsed(shared: &Rc<RefCell<VideoNoteShared>>) -> i32 {
    shared.borrow().started.elapsed().as_secs() as i32
}

/// Encoder thread: NV12 frames in, mp4 + thumbnail out.
fn recorder_thread(
    rx: mpsc::Receiver<RecMsg>,
    done: mpsc::Sender<Result<VideoNoteDone, String>>,
    path: PathBuf,
) {
    let run = || -> Result<VideoNoteDone, String> {
        let mut enc = Encoder::new(
            CodecType::H264,
            VIDEO_NOTE_SIZE,
            VIDEO_NOTE_SIZE,
            EncoderProfile::Realtime,
        )
        .map_err(|e| e.to_string())?;
        let mut writer = VideoWriter::new(
            &path,
            VIDEO_NOTE_SIZE,
            VIDEO_NOTE_SIZE,
            VIDEO_FPS,
            MuxerCodecType::H264,
        )
        .map_err(|e| e.to_string())?;
        if let Some(cfg) = enc.codec_config() {
            writer.set_codec_config(cfg);
        }
        let mut frames = 0u32;
        let mut last: Option<Vec<u8>> = None;
        while let Ok(msg) = rx.recv() {
            match msg {
                RecMsg::Frame(nv12) => {
                    frames += 1;
                    let mut stream = enc.encode_nv12(&nv12);
                    for part in stream.by_ref() {
                        let chunk = part.map_err(|e| e.to_string())?;
                        writer
                            .write_sample(&chunk, frames == 1)
                            .map_err(|e| e.to_string())?;
                    }
                    last = Some(nv12);
                }
                RecMsg::Finish => break,
            }
        }
        writer.finish().map_err(|e| e.to_string())?;
        // Zero recorded frames is a legitimate session — no thumbnail.
        // A JPEG encode failure propagates instead of collapsing into an
        // empty thumbnail.
        let thumb = match last.as_deref() {
            Some(nv) => {
                let rgba = nv12_to_rgba(nv, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE);
                Some(rgba_to_jpeg(&rgba, VIDEO_NOTE_SIZE, VIDEO_NOTE_SIZE)?)
            }
            None => None,
        };
        Ok(VideoNoteDone {
            duration: (frames / VIDEO_FPS) as i32,
            thumb,
        })
    };
    let _ = done.send(run());
}

// ---------------------------------------------------------------------------
// Helpers shared by capture paths
// ---------------------------------------------------------------------------

/// Convert an NV12 buffer back to RGBA (BT.601 limited) for thumbnails.
fn nv12_to_rgba(nv12: &[u8], w: u32, h: u32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let yv = nv12[y * w + x] as f32;
            let uv = w * h + (y / 2) * w + (x / 2) * 2;
            let u = nv12[uv] as f32 - 128.0;
            let v = nv12[uv + 1] as f32 - 128.0;
            let c = (yv - 16.0).max(0.0) * 1.164;
            let i = (y * w + x) * 4;
            out[i] = (c + 1.596 * v).clamp(0.0, 255.0) as u8;
            out[i + 1] = (c - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8;
            out[i + 2] = (c + 2.017 * u).clamp(0.0, 255.0) as u8;
            out[i + 3] = 255;
        }
    }
    out
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

    /// NV12 -> RGBA thumbnail path produces a full-size opaque frame.
    #[test]
    fn nv12_to_rgba_roundtrip_shape() {
        let w = 8;
        let h = 8;
        let mut nv12 = vec![126u8; w * h];
        nv12.extend_from_slice(&vec![128u8; w * h / 2]);
        let rgba = nv12_to_rgba(&nv12, w as u32, h as u32);
        assert_eq!(rgba.len(), w * h * 4);
        assert_eq!(rgba[3], 255);
        let (r, g, b) = (rgba[0] as i32, rgba[1] as i32, rgba[2] as i32);
        assert!((r - g).abs() < 8 && (g - b).abs() < 8, "rgb={r},{g},{b}");
    }

    /// Waveform is base64 of 100 5-bit bars packed into bytes.
    #[test]
    fn waveform_encodes_100_bars() {
        let samples: Vec<f32> = (0..4800).map(|i| (i as f32 / 480.0).sin() * 0.5).collect();
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
