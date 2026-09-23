# Dogfooding notes — WaterUI / hydrolysis

Findings collected while building a real Telegram client (Watergram) on
`waterui` 0.5.2 / `hydrolysis` 0.3.1 / `water` CLI 0.4.3.

## Lints (water-rs/lints)

- **README documents a tag that does not exist.** The "Using" section says
  `libraries = [{ git = "...", tag = "v0.1.0" }]` but the repo has no git tags
  at all (default branch is `dev`). `cargo dylint` fails to resolve it —
  had to pin `rev = "<sha>"` instead. Either cut the tag or change the README
  to use `rev`/`branch`.
- **`if_else_view` false-positives on `if`/`else` that produces *data*, not
  views.** `Str`, `String`, tuples of `Str`, `Color`, filled `Shape`s all
  implement `View`, so every `let x = if cond { a } else { b }` producing one
  is flagged — even when `cond` is a **plain `bool`** (a struct field), where
  the suggested `when(cond, || ..)` cannot compile: `when` requires
  `impl IntoComputed<bool>` and `bool` does not implement it. 15 warnings in
  this project, all of this form; I suppressed them with
  `#[allow(if_else_view)]`. The lint should skip conditions that are not
  signal-y (no `Binding`/`Computed` receiver, no `.get()`) or verify the
  `when(cond, ..)` rewrite actually type-checks.
- **`binding_parameter_by_value` silently breaks on edition 2024 if applied
  naively.** `fn note_line(note: &Binding<Str>) -> impl View` captures the
  `&'a` lifetime in the opaque return type (edition-2024 lifetime capture
  rules), so `note_line(&store.auth_note)` fails E0597. The correct form is
  `-> impl View + use<>` — the lint's help should emit it, otherwise its own
  suggestion does not compile.
- **`manual_text_map` suggests dropping `.clone()` inside `map` even when it
  is required.** `me.map(|m| m.name)` works only because `Str` is `Copy`;
  for `String` fields the suggestion doesn't compile. Minor, but the lint
  checks "feeds a `text!` placeholder" by name — it can't see the field type.

## Framework / API gaps

- **`waterkit-codec` transitively requires libva ≥ 2.20 on Linux but nothing
  documents or enforces that.** A scaffolded hydrolysis app fails to compile on
  stock Ubuntu 22.04 (system libva 2.14): `cros-libva 0.0.12` regenerates
  bindgen bindings against installed headers, yet `src/buffer/av1.rs`
  unconditionally references `refresh_frame_flags` / `bit_depth_minus8` /
  `mono_chrome` on `_VAEncPictureParameterBufferAV1` — fields that only exist
  in libva ≥ 2.20. Only `hierarchical_level_plus1` is cfg-gated
  (`libva_1_19_or_higher`); the newer fields are not. `cargo update` cannot
  escape it because `cros-codecs 0.0.6` pins `cros-libva ^0.0.12` (0.0.x caret
  = exact). Fix on this machine: build libva 2.22 from source into /usr/local
  and set `PKG_CONFIG_PATH` in `.cargo/config.toml`.
  Suggestion: gate the AV1 encode params on the same `libva_1_2x_or_higher`
  cfgs the crate already generates, or bump cros-codecs to a compatible
  cros-libva and document the minimum system version.
- **Scaffold comment claims the `media` feature avoids needing VA-API on
  Linux — that is false.** `waterui`'s `assets` feature (on by default in the
  generated Cargo.toml) pulls `waterui-assets → waterui-media → waterui-image
  → waterkit-codec` which unconditionally depends on `cros-codecs` on
  `cfg(target_os = "linux")` — there is no feature flag to disable the vaapi
  path. Even with `media` off, the base build still needs libva dev headers of
  a sufficiently new version.

## Semantic testing (waterui-testing)

- **`List` folds every row's contents into a single `ListItem` a11y node —
  inner controls and labels are unreachable.** Correcting the earlier note
  ("children do not appear at all"): `List::for_each`/`ListItem` rows DO
  show up in `#[waterui::test]` mounts — but each row is emitted as exactly
  one `ListItem` node whose label is the concatenation of its inner texts
  (a bubble becomes `"Alice first message 12:00"`), with `children: []`
  and only a `Focus` action. Nothing inside the row is addressable: no
  text labels (tests must switch to `label_contains` on the row), and —
  worse — no buttons: the ✗ resend control inside a failed bubble is
  invisible to a11y automation and, by extension, to real screen readers.
  Settings screens were rewritten as `scroll(vstack(..))` to stay testable;
  for chat rows the fold is acceptable, but any interactive element inside a
  `ListItem` silently loses a11y actions.
- **The semantic runtime has no theme tokens installed.** Mounting a view
  that calls `.foreground(Error)` (or any theme color) under
  `#[waterui::test]` panics: `color token `Error` is not installed in the
  environment`. Every test needs
  `#[waterui::test(theme = hydrolysis_m3::Material3::defaults())]` and a
  `UiBuilder<Styled<Material3>>` param. A built-in default theme would make
  the common case one attribute shorter.
- **`#[state]` extractors need `.state(&store)` on the *mounted* view.**
  `.action(|store: Store| ..)` fails with `Environment state not found at
  position 0` unless the view passed to `ui.mount` carries
  `.state(&store)`. Reasonable, but undocumented in testing docs — the error
  message points nowhere near `.state()`.
- **`ElementRef::set_text` takes `&mut SemanticApp` as its first arg** —
  `app.query().label("API ID").single().set_text(&mut app, "…")` — which is
  awkward to read and easy to get wrong; an `app.set_text(&ref, "…")` or a
  method on SemanticApp would be nicer.
- **Intermittent `malloc_consolidate(): unaligned fastbin chunk detected`
  under parallel `cargo test`.** 11 `#[waterui::test]` tests; one parallel
  run aborted with heap corruption, three subsequent identical runs passed
  (also fine `--test-threads=1`). Likely a race in the semantic runtime or in
  tdlib-rs's allocator linkage (the binary links tdjson even in unit tests).

## tdlib-rs 1.4 (dependency feedback, not WaterUI)

- **Nothing works until a background thread pumps `receive()`** — every
  `functions::*` future resolves through the shared `td_receive` queue, and
  spontaneous updates only start after the client's *first request*. A
  client created and then `receive()`-polled without sending a request sits
  silent forever (the doc comment does say this, but an `Update` for
  `authorizationStateWaitTdlibParameters` never arriving is surprising).
- **Telegram disabled self-service test accounts (`99966XYYYY`) in 2024**
  (tdlib/td#3083) — the magic phone-number logins no longer work; live e2e
  now needs a real account registered on the test DCs.

## CLI / DX

- **`Modifier::line_limit` ordering trap.** `.line_limit(N)` returns a
  different type than `.muted()`/`.foreground()` chains in some positions —
  order it *before* color modifiers or the chain fails to unify. No error
  message explains the ordering requirement.
- **`water` CLI scaffold (`water create`) generates `Cargo.toml` with a
  misleading `media` feature comment** (see above) and no `.cargo/config.toml`
  hook for `PKG_CONFIG_PATH` — Linux users hit the libva wall on first build.
- **`water run --backend hydrolysis` does not forward the repo's
  `.cargo/config.toml` `[build] rustflags` to the managed backend build.**
  The managed crate lives under
  `~/.water/build_cache/<repo-path>/managed_backends/hydrolysis`, so cargo
  config discovery starts there and never sees the app repo's config; even
  `$HOME/.cargo/config.toml` is not applied (likely a `CARGO_HOME`/`--config`
  override). Any project whose native deps need extra link flags — e.g.
  tdlib-rs' prebuilt libtdjson needing `__isoc23_strtol`/`__libcpp_verbose_abort`
  shims — fails at link with `undefined symbol`. Workaround: prefix the
  command with `RUSTFLAGS="..."` (env var *does* reach the build). Repro:
  add `-l dylib=foo` to `<repo>/.cargo/config.toml`, run `water run`, watch
  `cargo rustc` stderr lack the flag.
- **`water run` produces a dist dir whose `libwaterui_dylib.so` is renamed
  (hash stripped) but the launcher binary links against the hashed name.**
  Launch fails instantly: `error while loading shared libraries:
  libwaterui_dylib-<hash>.so: cannot open shared object file`. RUNPATH is
  `$ORIGIN`, so a `ln -s libwaterui_dylib.so libwaterui_dylib-<hash>.so`
  inside `dist/linux/debug/` fixes it — water should either copy the hashed
  filename or pass `-Wl,-soname`/rewrite the dep name.
- **`waterui::task::spawn_local` called during view construction panics
  (`Local executor not set`) on hydrolysis** — the local executor is only
  installed once the backend runs the mounted view, so async init work must
  live *inside* `.task(...)`/`.on_appear`, not in `fn main()`/Store::new.
  executor-core's docs don't say when the executor becomes valid; a sentence
  in `task::spawn_local` docs would have saved a crash-debug cycle.
- **`tdlib_rs::receive()` returns `None` on tdjson's ~2s poll timeout** — a
  `while let Some(...) = receive()` pump exits on the first idle gap, the
  channel closes, and the app's update drain dies permanently (frozen
  "Connecting…" while TDLib keeps working). The pump must `loop` and skip
  `None`s. Verified live: with the fix the credentials→phone transition
  proceeds; without it, `authorizationStateWaitPhoneNumber` never reaches
  the UI. Not a WaterUI bug, but every nami/`spawn_local`-style host will
  hit this — worth a doc line in tdlib-rs.
- **hydrolysis winit windows are locked to layout size**: `platform.rs`
  forwards min=max size limits to `set_{min,max}_inner_size`, so windows
  can't be resized and collapse to tiny strips on small-content screens
  (the Loading screen renders at 123x54; `xprop` confirms
  min=max=123x54). Maximize is undone on every screen change.
- **`water run`'s shared-dylib freshness check false-positives after a
  failed/interrupted build**: "no dep-info was found beside it" for
  `libwaterui_dylib.so` blocks every rebuild until
  `water gc build-cache --shared-target` (12 GB wipe + full rebuild).
  The check should repair the single unit instead of requiring a gc.
- **No a11y bus on the hydrolysis Linux build** (accesskit-atspi not
  wired?): the window exposes nothing to AT-SPI, so UI automation has to
  drive raw coordinates. Real users on Orca get nothing either.
- **GPU-less VMs can render via `WATER_HYDROLYSIS_FORCE_FALLBACK_ADAPTER=1`**
  (llvmpipe compute actually works — the Material 3 auth screens draw
  correctly, just slowly). Worth documenting as the standard way to run
  hydrolysis in CI/headless instead of "diagnostics only".
- **Parallel `#[waterui::test]` mounts corrupt the heap — root-caused via
  ThreadSanitizer** (waterui-testing 0.5.1 + hydrolysis-m3 0.3.1 + fontique
  via hydrolysis test path + xcap 0.9.8/waterkit-screen 0.1.4):
  `cargo test --lib` (default parallelism) intermittently aborts with
  `malloc_consolidate(): unaligned fastbin chunk detected` /
  `double free or corruption` (~1-in-10 on this machine, was ~2-in-3 with
  more mount-heavy tests). ASan (nightly `-Zsanitizer=address`,
  `-Zbuild-std`) reproduces nothing — 8/8 clean runs — because its allocator
  replaces the glibc arena the bug corrupts. **TSan
  (`-Zsanitizer=thread -Zbuild-std`) identifies the races directly**:

  1. `fontique::collection::System::new` → `Collection::new`, called from
     `hydrolysis::runner::fonts::deterministic_test_fonts` inside
     `SemanticRuntime::new_for_tests` (via
     `waterui_testing::app::UiBuilder::mount_semantic`). Two test threads
     mounting concurrently race inside libfontconfig on shared global state:
     `FcObjectSetBuild` (strcmp/strdup/malloc), `FcDirCacheLoad`
     (pthread_mutex_lock), `FcAtomicDestroy` (malloc → free). Fontconfig's
     first-use config build is not serialized → heap corruption. 4 of the
     5 TSan reports are this path.
  2. `xcb::base::Connection::connect` → `XauGetBestAuthByAddr` →
     `XauFileName`/`fopen`, called from `xcap::monitor::Monitor::all` ←
     `waterkit_screen::max_refresh_rate_hz` ←
     `waterui_internal::runtime::task::runtime_guard::MonitoredLocalExecutor::
     with_config_and_probes`. libXau's static filename buffer races with
     another thread's malloc — same class of unsynchronized native init.

  Fix belongs in framework: serialize `deterministic_test_fonts` /
  `fontique::System` construction (OnceLock/mutex) and gate
  `waterkit_screen::max_refresh_rate` X-probing behind OnceLock on the
  headless/test path. Filed upstream: libXau/xcap race →
  **water-rs/waterui#1201**, fontconfig concurrent init →
  **water-rs/hydrolysis#92** (needs an independent repro to confirm —
  libfontconfig is not TSan-instrumented).

## Media capture: no audio/video recording components

Playback exists — `waterui-video 0.5.1` (`video_player`/`video`, facade
features `video` + `video-gpu`; GPU decode path pulls `waterkit-video`,
`cros-codecs` VA-API and `cpal`/`alsa-sys` — needs `libva-dev`,
`libpipewire-0.3-dev`, `libspa-0.2-dev`, `libasound2-dev` on Ubuntu).
Watergram plays downloaded video / voice-note / audio / animation files
inline via `video_player` (unverified at runtime until real-login e2e;
the semantic-test path only exercises the not-downloaded fallback).

**Capture APIs exist in waterkit; only the waterui view layer is
missing.** `waterkit-audio 0.1.4` provides `AudioRecorderBuilder`/
`AudioRecorder`/`InputDevice`/`AudioBuffer` (desktop = cpal);
`waterkit-camera 0.1.4` provides `Camera::list`/`Camera::open`,
`CameraConfig`, `Frame::into_texture()`, `Photo`, `Recording`. Watergram
implements voice notes as cpal capture → `opus-pure` (Opus + Ogg) →
`InputMessageVoiceNote`, and video notes as: `GpuSurface`/`GpuView`
that clones the surface's `wgpu::Device`/`Queue` into `Arc`s
(both are `Clone` — the pattern in `examples/waterkit_camera_filters`),
`Camera::open_default` on that shared device, frames drawn straight from
`Frame::into_texture()` in `render()` — no CPU round-trip for the preview
(src/capture.rs).

**Still missing upstream (hand-assembled in src/capture.rs):**

1. No ready-made camera-preview *view*. The GpuView pattern works, but
   the app still had to hand-write ~300 lines: WGSL shaders (fullscreen
   triangle + square-crop sampler), pipelines, bind groups, the
   per-render `Box::pin(camera.frames())` + `poll_next` with a noop waker
   (needed because `frames()` borrows `&Camera` — an owned/`'static`
   frames stream would compose with `spawn_local` instead). A facade
   `CameraPreview` view would eliminate all of it.
2. `Camera::recording()` is `CameraError::ControlUnsupported("recording
   not supported on desktop")` — **water-rs/waterkit#86**. Until fixed,
   the mp4 path is hand-rolled: a compute pass converts the camera
   texture to 360×360 NV12 (center-crop + downscale + BT.601) inside a
   storage buffer, and that one buffer is mapped back because
   `Encoder::encode_nv12` is a CPU API feeding VA-API — the preview never
   leaves the GPU, only the encoder's input crosses back. Then
   `Encoder::new(H264)` → `VideoWriter` mp4 on a worker thread fed by an
   mpsc channel. `encode_nv12` also returns a one-shot iterator yielding
   a single `Result<Vec<u8>>` rather than a per-frame stream; a
   pull-based frame encoder API would be more honest.

**Observed on a device-less Linux VM (Ubuntu 22.04, no /dev/video*,
/dev/snd, /dev/dri):**
- `AudioRecorder::list_devices()` can return a non-empty list (cpal
  enumerates the ALSA `default` PCM even with no hw backend), then
  `recorder.start()` fails — `ALSA lib pcm.c:2664:
  (snd_pcm_open_noupdate) Unknown PCM default` on stderr, `RecordError`
  (OpenFailed/StartFailed) returned. The app surfaces the error verbatim
  in the composer ("Capture" banner) and the mic button stays usable.
- `Camera::list()` returns `Ok([])` → app shows `camera: no camera found`
  in the video-note sheet and keeps Record/Send inert (no crash, no
  panic from nokhwa).
- `Encoder::new(H264)` opens VA-API by iterating DRM nodes; with no
  /dev/dri it fails `CodecError::InitializationFailed`, so the video-note
  encode path reports `encode: …` in-sheet. A software fallback exists
  only behind a non-Linux cfg, so desktop Linux without GPU cannot make
  video notes — worth flagging upstream.

- **`Lazy::vstack` inside a `scroll()` collapses the window to ~1 row on
  hydrolysis.** With `WATERGRAM_DEMO=1` (10 chats seeded) the sidebar-only
  main page sizes its winit window to ~400×370 — `min=max` WM hints forbid
  resizing, and the lazy list materializes only what the tiny viewport shows
  (~1 chat row), so users see a single chat until they select one (the
  split view grows the window only after selection). The same sidebar in a
  taller window renders 4+ rows, so the data/collection is fine — the
  locked-size + lazy-viewport interaction is the defect. Same class of
  issue as the "windows locked to layout size" note: the layout's intrinsic
  size of a lazy list should account for its content estimate, or the
  window should allow resize so lazy content can grow into it.
  Related: the nav-stack push target (Settings page) is constrained to the
  sidebar's ~230px width, so settings fields and buttons overflow the
  locked window's right edge — visible clipping, no way to widen.
- **Emoji render as tofu (□) in hydrolysis on this VM** — "🚀"/"👍2 ❤️1"
  in bubbles and icon labels draw as empty boxes; system has no emoji font
  installed for fontique to fall back to. Not a framework bug per se, but
  worth a bundled fallback note (any real user distro has Noto Color
  Emoji; headless CI does not).

## Scroll: index scrolling exists (correction)

Retracting the earlier claim that `ScrollController` is Point-only —
`waterui-internal 0.5.1` `component::list` provides
`List::for_each(data, |item| ListItem::new(view)).scroll_controller(
&ScrollController<usize>)`, which drives `scroll_to(index)`; hydrolysis
implements it (`ScrollController<usize>` lives on `ListConfig`) and the
docs note jumps do not materialize preceding rows. Watergram's message
list now uses it for exact jump-to-message + highlight. The remaining
gap is only key/id-based scrolling (`scroll_to(id: i64)`): the app keeps
the message→index lookup itself — fine for Telegram, where the target id
is known and the index is resolved before scrolling.
