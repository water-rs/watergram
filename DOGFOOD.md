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

## Layout findings (r7–r8) — one confirmed framework bug, API gaps

Measured with `waterui-testing` offscreen mounts (semantic bounds) +
`water mcp` tree dumps, not by eye. Watergram probes live in
`src/lib.rs` (`probe_*` tests) and are kept as the measurement harness.

### `ScrollView` reports `StretchAxis::Both` regardless of its axis — working as specified

Layout-spec §3 states plainly: "`ScrollView` is `Both`", and a lazy
container (`List`) reports `None` because it cannot enumerate its
children. So in a `vstack`, a `scroll_horizontal` chips row is the
*only* stretching child and correctly takes all free space; the chat
`List` gets none (measured: chips `(0,0,340,195)`, list `(0,205,340,195)`
in a 340×400 mount — the §3 contract, not a bug).

The app-side fix per §3's note that a lazy container that must fill "is
placed by a parent that stretches it (`ScrollView`, `Absolute`, a `Frame`
with `max = INFINITY`)": give the chips row its content height (chip
metrics: caption ~17 + 2×3 padding = 23) and let the chat `List`'s
`scroll` parent claim the rest. Verified bounds after fix: chips scroll
`(8, 214, 284, 23)`, chat list `(0, 261, 340, 439)` reaching the 700 dp
viewport bottom.

### M3 `TextField` reports a fixed 280 dp minimum — hstack overflow

**Now tracked as water-rs/hydrolysis#107** (violates §7 "a text field
answers the proposal width").

`hydrolysis-m3 0.3.1` `INPUT_FIELD_MIN_WIDTH = 280.0`
(`theme/dimensions.rs`, "TextFieldDefaults.MinWidth"). A field answers
the proposal width but never measures below 280, so under §4.2 a stack
whose children minima exceed the offer overflows rather than shares.

Minimal repro (semantic mount, 340×300, after `padding_with((8,8))`):

```rust
hstack((
    field("First name", &a).hide_label(),
    field("Last name", &b).hide_label(),
)).spacing(8.0).padding_with(8.0)
```

- measured: field1 `(8, 122, 280, 56)`, field2 `(296, 122, 280, 56)` —
  total 568 > 340 offer → second field clipped at the right edge.
- expected: ~158 each (`(340 - 16 - 8) / 2`) or a documented overflow
  policy. 280 is the M3 single-field floor; two fields in a row need a
  smaller per-field minimum or a stacking policy.

Watergram settings (sidebar-width pane) stacks the name fields
vertically as a legitimate narrow-pane layout; when #107 lands they
should return to one row, as in Telegram Desktop. The same floor shapes
the chat composer: a 280 dp field plus even a few controls cannot fit a
460 dp pane, so composer actions are minimized and search/members moved
to the navigation toolbar.

### API gap: `max_width(N)` is greedy, not a content cap

`frame_resolved_axis` (waterui-layout `containers/frame.rs`) resolves a
bounded axis to the parent's proposal whenever `max.is_some()` and the
proposal ≠ 0. Under §4.2 the stack's ideal probe is `INFINITY`, so a
`.max_width(420)` frame reports `min(∞, 420) = 420` — its *cap* — as its
ideal, regardless of content size.

Minimal repro (semantic mount, 800×200):

```rust
hstack((
    text("short").padding_with(10.0).max_width(420.0)
        .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
    spacer(),
))
```

- measured: background `(0, 30.6, 420, 38.75)` — always the cap.
- expected (Telegram bubble semantics): as wide as the text (~80), up to
  the 420 cap. Spec-conformant behaviour, but it means "hug content with
  a ceiling" is not expressible via `max_width` — there is no construct
  for `min(intrinsic, cap)`. A `Spacer` inside the max'd frame makes it
  worse (the spacer fills to the cap on the same axis).

Watergram bubbles now drop the cap frame entirely and let the stack's
`min(ideal, available)` clamp hug content; media parts keep their own
fixed media frames. Post-fix bounds: incoming `(12, 311.6, 347.9, 76.8)`,
outgoing `(308.3, 311.6, 479.7, 76.8)` — timestamps/reactions are zstack
overlays at the bubble's trailing/leading bottom so they cannot make the
bubble greedy.

### API gap: no compact icon-button semantic → water-rs/hydrolysis#115

`button()` carries `BUTTON_MIN_WIDTH = 58` (hydrolysis-m3
`theme/dimensions.rs`); an icon-only `button(label(t).icon(i).icon_only())`
measures **72 dp wide** (live MCP bounds `(12,134,72,40)` per button).
There is no waterui-level semantic for the ~40-48 dp icon button every
chat/composer toolbar needs — filed as water-rs/hydrolysis#115 (an
icon-only Button should get M3 icon-button size).

Measured widths until #115 lands (1000×700 window): sidebar toolbar —
`New chat (12,134,72,40)`, `Archive (94,134,72,40)`,
`Accounts (176,134,72,40)`, `Settings (321.7,134,72,40)` → toolbar
content ends at **x=393.7 > 340 sidebar width** (clipped); the "Online"
label lands inside at 268. Chat toolbar — `Search in chat (780,12,72,40)`,
`Members (852,12,72,40)`, `Scheduled messages (924,12,72,40)` → ends at
996 ≈ 1000-pane edge. Composer — `Attach file (346,648,72,40)`,
`Stickers & GIFs` same box. Watergram uses the semantic
`label(title).icon(icon).icon_only()` form everywhere (incl. the
`FilePicker` trigger, which wraps `Button::new` and inherits the same
box — measured `(346,648,72,40)`); nothing is hand-rolled any more, the
overflow stands until #115.

**FilePicker has the same box.** `FilePicker::open(label, …)` wraps
`Button::new`, so its trigger inherits the ~72 dp box with no way to
compose a compact one — measured `Attach file` at `(6,648,72,40)` in a
660 dp pane. Combined with the `TextField` 280 dp floor
(water-rs/hydrolysis#107), the composer row's minimum width is ~500 dp: on any
pane narrower than that the whole chat `vstack` adopts the composer's
ideal width, gets centered in the offered space, and every sibling
shifts left — in the 460 dp probe the pinned-bar icon lands at
`x = -10` and `Attach` at `x = -16`, clipped at the pane edge (the
"stray clipped icon" from r7 was this exact mechanism, at ~640 dp
composer width with the old 72 dp buttons). Post-#107 the composer fits
its 660 dp pane — pin icon `(352, 70)`, `Attach file (346,648,72,40)`,
all composer children inside `(340..1000)` — but panes narrower than the
button-row's intrinsic width still shift left until #115 lands
and FilePicker accepts a custom trigger view. **Update: #107 has
landed on hydrolysis dev (PR #112) — the TextField now answers the
proposed width; first/last name are back on one row.** The remaining
width floor is the icon-button box (#115).

**Knock-on at r10 (info panel, #115-bound).** The detail root is
`hstack((chat_vstack, info_panel.width(280)))` in a 660 dp offer at
1000×700. `compress_to_fit` (layout `distribute.rs`) never pushes a
child below its `measure(proposal 0)` minimum and overflows instead of
collapsing — correct per spec. But the chat pane's floor is the
toolbar/composer button rows: 4+ icon-only buttons × 72 dp ≈ 495 dp
minimum, so the stretchy chat vstack keeps 495 dp, the 280 dp panel is
placed at x=835 and runs to 1115 — **115 dp clipped past the 1000 dp
window edge** (measured `scroll_view (835,172,280,528)`, cells clipped
at x≈1000). Once #115 lands (~40-48 dp icon buttons) the pane floor
drops ≈150 dp and the panel fits. Not routed around: the panel is the
M3-correct `hstack`+fixed-width sibling composition; shrinking it
below 280 would mis-size the shared-media grid. (Superseded at r12:
the panel now docks ≥1120 px and overlays below, per Telegram
Desktop's own threshold — the clip no longer occurs at any width.)

**r14 update — #115 landed on hydrolysis dev e676ca8.** Icon-only
`button()`s now lay out at the M3 48 dp touch target: probe bounds on
the new pins show `Attach file` `(6,644,48,48)` and `New chat`
`(280,6,48,48)` — 48×48 boxes with the centred 40 dp container, all
composer/toolbar children inside their panes. **One exception:
`Menu::new(label(t).icon(i).icon_only(), items)` still measures
72×40** (`(12,10,72,40)`) — the menu trigger builds its own
`MENU_TRIGGER_STYLE` path and does not honour `icon_only`. Filed as
**water-rs/hydrolysis#149** — **FIXED at hydrolysis dev e1ffa2c6 (verified
r16): the sidebar `Menu` now measures `(4,8,48,48)`.**

### `EdgeInsets` tuple order is `(vertical, horizontal)` — ergonomics trap

`impl From<(f64, f64)> for EdgeInsets` is documented as
`(vertical, horizontal)` — the opposite of the CSS/`margin: v h`
intuition every other API trains you for (`padding_with((12, 2))`
reads as 12 horizontal, 2 vertical). In Watergram, 38 call sites were
written with the (h, v) reading; the silent result was rows ~24 pt
taller than designed (12 vertical instead of 2) — invisible in code
review, only caught by bounds probes. Not a bug — documented — but a
strong footgun; consider a named constructor
(`EdgeInsets::hv(h, v)`), `padding_with(h, v)` two-arg form, or a
dylint that flags asymmetric tuples.

### Emoji tofu: resolved by font, not framework

Installing `fonts-noto-color-emoji` on the VM fixed all tofu — emoji
now rasterize correctly in bubbles/icons. The earlier entry stands as
an environment note only (headless VMs need the font installed).


## Layout findings (r9)

### `Lazy` inside `scroll(vstack)` clips to realized rows — app misuse, spec-consistent

Sidebar `scroll(vstack((Lazy::vstack(chat_rows), Lazy::vstack(results))))`
measured: outer `scroll_view (0,261,340,439)`, inner scroll_view
`(0,261,340,209.5)` holding 4 chat rows (avatars y267/333/399/465, fourth
clipped), second inner `(0,490.5,340,209.5)`; pointer `scroll` was a
no-op (identical tree). Per layout-spec §3 a lazy container reports
`None` on its axis → measured to its realized rows (~209.5); a second
same-axis scroller inside `scroll()` is app misuse, not a framework
clip bug. Fix: `List` (StretchAxis::Both) for fill regions,
`VStack::for_each` / `HStack::for_each` for eager rows inside scroll
content. Post-fix: single `list (0,241,340,459)`, 7 rows to window
bottom; chat `list (340,160,660,365.9)` to the composer. Worth a
note in the Lazy/List docs: the failure mode (silent mid-pane clip,
dead scroll input) is not obvious.

### Collapsed `when()` children still consume vstack spacing

A `when(cond)` child that renders nothing still occupies a child slot —
with default 10px spacing each hidden optional section adds ~10px.
Three hidden `when` panels in the sidebar pushed the chips row ~30px +
paddings: toolbar bottom y174 → chips y225 (~70px gap incl. section
padding). Numbers post-fix (wrap the optional sections in
`vstack(…).spacing(0)`): chips `scroll_view (8,194,284,23)`.
Question for the framework: should an empty conditional child skip its
spacing slot? (Not obviously a bug — spacing is between child slots —
but a real footgun: the gap is invisible in code review.)

### ScrollView reports `StretchAxis::Both` regardless of `axis` → water-rs/waterui#1208

`raw_view!(ScrollView, StretchAxis::Both)` — the scroll `axis`
(Horizontal/Vertical/All) is not consulted. A `scroll_horizontal` in a
vstack therefore stretches vertically and would eat the sidebar
surplus; the only constraint mechanism is a literal `.height(23)`
(chip caption ~17 + 2×3 padding, measured). There is no View modifier
that proposes `None` on one axis / fixes a view to its content size on
one axis only (`Frame::ideal_height` fills an unspecified proposal
with a fixed f32 — not "propose None"), and the M3 chip/caption
metrics are not reachable from view code, so the literal is derived by
hand and breaks under font scaling. Suggestion: a single-axis
ScrollView should stretch only on its scroll axis (or a
`.fixed_size(axis)`-style modifier should exist).

## Emoji font fallback → water-rs/hydrolysis#119 (r9, fonts-noto-color-emoji installed)

`fc-match "sans:charset=<cp>"` per codepoint — DejaVu Sans (monochrome)
wins for every default-emoji-presentation codepoint it covers; Noto
Color Emoji only for codepoints DejaVu lacks. **Filed upstream as
water-rs/hydrolysis#119.** Rendered grid matches
fc-match exactly. Monochrome faces: U+1F600–1F606, 1F609, 1F60A–1F61D,
1F617–1F61A, 1F612–1F615, 1F618, 1F623, 1F625, 1F62A–1F62F, 1F632,
1F634, 1F636, 1F60C, 1F60F, 1F643,
U+2764 ❤, U+26A0 ⚠, U+270F ✏, U+2615 ☕, U+2600 ☀, U+26A1 ⚡,
U+1F431 🐱 — all have Emoji_Presentation=Yes yet get a monochrome
face. Colour (Noto) only: 1F923, 1F970, 1F910, 1F911, 1F924, 1F928,
1F642, 1F914, 1F917, 1F971, 1F641, 1F644. This is the font-fallback
ordering bug you suspected: the text stack needs to prefer the emoji
face for emoji-presentation codepoints (fc-match `emoji:charset=1F600`
→ Noto Color Emoji), not the generic sans chain.

## waterui-cli / water mcp defects (r9) → water-rs/cli#177, water-rs/cli#176

- **Dev-channel `prepare_build` seed-merge produces incoherent
  lockfiles.** With `channel = "dev"` the CLI seeds the generated
  crate's Cargo.lock from `previous ∪ canonical(Water.lock) ∪
  project_lock`; the merged seed left wasm-bindgen/js-sys
  (.105↔.128 lockstep), then cc/find-msvc-tools/log edges in conflict
  (`cargo update -p cc --precise 1.4.7` rejected by Water.lock
  validation which pinned cc=1.4.5). **water-rs/cli#177.** Whack-a-mole;
  abandoned dev channel — `[patch.crates-io]` on the app side reaches the same
  hydrolysis dev rev and `Source::Stable` skips the whole
  validate/prepare path (waterui-cli-0.4.3
  `src/project_model/framework.rs` ~684).
- **Staged hydrolysis runtime holds stale dylib hashes.** `water mcp`
  spawned `…/shared/x86_64-unknown-linux-gnu/debug/watergram-*` which
  requires `libwaterui_dylib-4624ee4d224e52f7.so`, but the staged
  runtime dir held an older-hash `libwaterui_dylib-492067d7….so` →
  child exits before `initialize` → "the app binary did not answer
  initialize: Connection closed". **water-rs/cli#176.** Workaround: copy the built
  `deps/libwaterui_dylib-<newhash>.so` into the artifact dir. The
  staging step should refresh, not union-keep-oldest.

## M3 text field: label/prompt sits high in the 56 dp container (r10)

→ **water-rs/hydrolysis-m3#85**. Field height itself is spec-exact: the
filled container measures 56 dp on the nose (verified again at 1400×900
on `e676ca8` — `Search chats` (4,68,332,56)). Not a framework sizing
bug; the deviation is prompt/label *position*:

Measured on the empty, unfocused fields `Search chats` (bounds
(4,68,332,56)) and `Message` (bounds (498,640,340,56)) at 1000×700,
scale 1. Text ink rows inside the 56 dp container: **10–21** for
"Search chats" (cap height ~12 px) and **11–24** for "Message" —
vertical centre ≈ **15.5–17.5 px** from the container top. Per the M3
filled-text-field spec an empty, unfocused field shows the
label/prompt vertically centred in the 56 dp container: expected
centre ≈ **28 px**, ink ≈ rows 20–34. hydrolysis-m3 places it ~11 px
too high — as if the label slot were already in its "floating"
position. Minimal repro:

```rust
field("Search", &binding).prompt("Search chats").hide_label()
```

in an empty, unfocused window → prompt ink at y+10..21 of the 56 dp
container, expected centred at y≈20..34.

## nami: nested `when`-gates on signals derived from one `Binding` panic in `WatcherManager::cancel` (r10)

**Reproduced crash** (hydrolysis dev `47b1081`, nami-core 0.3.3): a
`Binding<Vec<Url>>` (`attach`) feeds `has_attach`/`is_img`/`fname` —
`Map`/`Distinct` signals — and the view has `when(has_attach, …)` whose
child itself contains `when(is_img, …)`. Setting the binding at runtime
(`attach.set(...)`, e.g. the FilePicker callback or the strip's Remove
button) panics:

```
nami-core-0.3.3/src/watcher.rs:357:40: RefCell already borrowed
  5: WatcherManagerGuard<Vec<Url>>::drop          (guard on attach's manager)
 27: WatchedDynamic<Option<usize>, Computed, When<Distinct<Map<Binding<Vec<Url>>,bool>>,..>>::drop_slow
 56: Rc<Fn(Context<bool>)>::drop_slow             (the when-gate's own watcher)
 59: WatcherManagerGuard<Vec<Url>>::drop          (nested — second guard, same manager)
```

Mechanism, from `nami-core watcher.rs:355-360`:

```rust
pub fn cancel(&self, id: WatcherId) {
    let (origin, subscribers) = {
        let mut inner = self.inner.borrow_mut();   // borrow_mut HELD
        inner.cancel(id);                          // map.remove(&id) → drops Rc<Watcher>
        (inner.origin, inner.len())
    };                                             // released only here
```

`map.remove` drops the removed watcher's `Rc` while `borrow_mut` is still
held. When that watcher closure transitively owns another
`WatcherManagerGuard` on the *same* manager (the `when` gate's
`WatchedDynamic` teardown does), its `Drop` re-enters `cancel` →
`borrow_mut` → panic. Fix belongs in nami: take the `Rc` out of the map,
release the borrow, then drop the `Rc`.

Minimal-repro shape (any second-level `when` on a signal derived from
the same binding whose teardown owns a guard on that binding's
manager):

```rust
let b = binding(Vec::<i32>::new());
let outer = b.clone().map(|v| !v.is_empty());
let inner = b.clone().map(|v| v.len() > 1);
vstack((when(outer, move || {
    let i = inner.clone();
    when(i, || text("many")).otherwise(|| text("one"))
}),))
// b.set(vec![1]) → outer rebuild → teardown → nested cancel → panic
```

Watergram repro: `WATERGRAM_DEMO=1 WATERGRAM_DEMO_PAGE=attach` mounts
the chat page, then sets `store.attach` ~800 ms post-mount → panic.
The production path hits the same code (`FilePicker` on-pick →
`attach.set`), so attachment picking crashes until the nami fix; the
`#[waterui::test]` semantic test (`attach_preview_shows_caption_field`)
passes because the testing runtime drives the same views without the
real renderer's watcher topology. Left as-is per dogfooding rules —
not restructured around it.

---

## r11-3: `#[waterui::test]` cannot reproduce nami#23 — testing-runtime gap → water-rs/waterui#1213

**Versions:** waterui-testing 0.5.1 (patched to waterui dev `b838d73`),
nami-core 0.3.3, hydrolysis dev `47b1081`.

The minimal crash topology — a `when` gated on signal A (derived from
one binding) whose body builds a second `when` on signal B (same
binding), toggled false post-mount — **passes** in the semantic
testing runtime while the identical shape panics the hydrolysis
renderer:

```rust
// tests::nami_cancel_reentrancy_minimal — passes (green, no panic)
let b = waterui::reactive::binding(Vec::<Str>::new());
let outer = b.map(|v: Vec<Str>| !v.is_empty()).distinct();
let inner_src = b.clone();
ui.mount(move || {
    let s2 = inner_src.clone();
    when(outer.clone(), move || {
        let inner = s2.map(|v: Vec<Str>| v.len() > 1).distinct();
        when(inner, || text("many")).otherwise(|| text("one"))
    })
});
b.set(vec![Str::from("a")]);  // mounts B's watcher under A's watcher
b.set(Vec::new());            // A toggle → cancel → nested guard drop
```

**What differs:** in `hydrolysis` the `when` node's teardown cancels its
`WatcherManager` subscription while `cancel` still holds
`inner.borrow_mut()`, and the removed watcher's `Rc` transitively drops
the inner `WatcherManagerGuard` → re-entrant `borrow_mut` panic. The
SemanticApp in waterui-testing drives the same views and the same nami
signals but does not retain the child subscription's guard inside the
outer cancel scope — teardown ordering differs — so the re-entrant
cancel is invisible to `#[waterui::test]`. Symptom: the attach-preview
test and this minimal test both go green while the real renderer panics
(`RefCell already borrowed` at `nami-core-0.3.3/src/watcher.rs:357`).
Result: nami#23 needs a runtime-level repro or a renderer-level harness;
`#[waterui::test]` alone cannot guard against this class of watcher
re-entrancy. **Filed as water-rs/waterui#1213.**

---

## r11-2b: `Frame` signal props sampled at mount — no re-propagation → water-rs/waterui#1214

**Version:** waterui dev `b838d73` (facade), hydrolysis `47b1081`.

`Frame::new(view).max_width(sig)` where `sig` is a
`Computed<f32>` derived from `Window.frame` (`win_frame.map(...)`) is
applied with the signal's **initial** value only. `Window.frame` reads
`Rect::from_size(0,0)` until the runner writes real geometry during
mount — so the Frame's max latched at the 0-width-derived value
(`(0-340).max(240)*0.65 ≈ 140px`) for the rest of the session, while a
`when` gate driven by the same `win_frame` correctly re-rendered on the
same signal change (panel docked at 1400px). Symptom: every bubble
wrapped at ~135px under a 1400px window; expected cap ≈ 480px once the
frame signal emitted the real width. If signal props on Frame are meant
to be live, the Frame realization must subscribe and re-measure on
change; today it samples once. **Filed as water-rs/waterui#1214.**

**r12 status:** the r11 static-480 workaround is reverted — the bubble cap
is again the signal-derived `win_frame.map(|f| ((f.width() - 340.0) *
0.72).clamp(220.0, 480.0))`, which latches at the mount-time pane width
(220dp under `Window.frame = 0`). This is the honest reproduction; every
bubble pins to the stale cap until #1214 lands. Repro: `WATERGRAM_DEMO=1
water mcp --viewport 1400x900` → any long message row.

---

## r11-4a: `when(a).otherwise(b)` (WhenComplete) panics the real renderer at mount — nami#23 again

**Version:** waterui dev `b838d73`, hydrolysis `47b1081`, nami-core 0.3.3.

```rust
let docked = open.zip(&win_frame).map(|(o, f)| o && f.width() >= 1120.0).distinct();
when(docked, || hstack((chat_column(), info_panel())))
    .otherwise(|| zstack((chat_column(), overlay_panel())))
```

**Observed:** both the winit renderer (`water run` under Xvfb) and `water mcp`
(SemanticRuntime) panic **at mount** on every page containing this node —
`RefCell already borrowed` at `nami-core-0.3.3/src/watcher.rs:357`.
Backtrace: the `NavigationView` destination closure drops a `WhenComplete`
whose `WatchedDynamic` teardown cascades `Option<AnyView> → FixedContainer →
Computed<StyledStr> → (Box<WatcherGuard>, Binding<Screen>) →
WatcherManagerGuard<Rect>::drop` — a second `cancel` entering while the first
still holds `borrow_mut`. Two mutually-exclusive plain `when`s on the same
signals mount and run fine on both renderers — the crash is specific to
`WhenComplete`'s shape, not to watching the zip'd `Binding<Rect>`.
`#[waterui::test]` mounting the same tree passes (same testing-runtime gap as
r11-3). **Filed as water-rs/nami#23** (fix session running).

**r12 status:** the r11 two-`when` mitigation is reverted — the info-panel
switch is back to the honest `when(docked, ..).otherwise(..)` form shown
above, so every page containing `chat_detail` **panics at mount** on both
the winit renderer and `water mcp`'s SemanticRuntime until nami#23 lands.
Repro: `WATERGRAM_DEMO=1 water mcp` or `water run` → any chat page.
Real-renderer verification of the chat page, the docked/overlay switch,
and the media grid is therefore **blocked on nami#23** — the equivalent
`#[waterui::test]` probes stay green (waterui#1213).

**r13 RESOLVED:** nami#23 landed (nami dev `969900ea`, consumed via
`[patch.crates-io] nami-core`). The chat page — including the honest
`when(docked,…).otherwise(…)` switch and the shared-media grid — now
mounts and renders on the winit renderer with **zero panics** at both
1400×900 (docked) and 1000×700 (overlay). waterui#1213 stays open: the
`tests::nami_cancel_reentrancy_minimal` repro still does not panic under
SemanticApp, so the testing-runtime gap is real even though the crash
itself is fixed upstream.

---

## r11-4b: SemanticRuntime layout diverges from the winit renderer and from SemanticApp

**Version:** waterui dev `b838d73`, hydrolysis `47b1081`.

Three divergences on the identical view tree, all reproducible via
`WATERGRAM_DEMO=1 WATERGRAM_DEMO_PAGE=info water mcp`:

1. **`Window.frame` is never driven** — filed as **water-rs/hydrolysis#128**.
   The SemanticRuntime runner builds its own `Window` and never writes the
   app-facing frame binding, so `win.frame` stays at whatever `app()`
   seeded (here 1280×800 or the `WATERGRAM_WIN_SIZE` demo hook) regardless
   of `--viewport`. Signals derived from it (`info_docked` at ≥1120) can
   never turn true under mcp without the seed. Expected: the runner
   seeds/drives `Window.frame` from the viewport like the winit runner
   does on `Moved`/`Resized`.
2. **`when`-materialized sibling does not shrink the flex sibling** — filed
   as **water-rs/hydrolysis#129**. With `hstack((chat_column,
   when(docked, panel)))` and `docked=true` at mount, the winit renderer
   and SemanticApp place the column at 780 and the panel at 1120 (bounds:
   list `(340,160,780,556)`); SemanticRuntime gives the column
   `(340,160,1060,556)` — its measured-before-materialize width — and
   overlays the panel on top of it. **r12: the computed-width mitigation
   is reverted** — the column is a bare `chat_column(...)` again, so #129
   is the honest repro until it lands.
3. **Text does not wrap inside a container `max_width` under
   SemanticRuntime** — filed as **water-rs/hydrolysis#130**.
   `Frame::new(bubble).max_width(480)` wrapping `text()` that exceeds the
   cap: winit renderer and SemanticApp wrap the text (bounds e.g.
   `118.8×93.75` ≈ 5 lines at cap 120); SemanticRuntime renders one line
   clipped at the cap edge (visible in r11 mcp screenshots at
   `WATERGRAM_WIN_SIZE=1400x900`).

---

## r13-1: `water run` ignores the app's `[patch.crates-io]` — git-pin fixes never reach the real renderer

**RESOLVED upstream (cli#178 → PR #179).** Verified on cli dev
`0414a255`: the regenerated managed `Cargo.toml` follows the app's
`[patch.crates-io]` on **every** `water run`, not only on first
generation — flipping hydrolysis `ab98d5fb ↔ e676ca88` in the app's
patch table was reflected in the managed manifest on each run. The
manual repin step below is no longer needed.


**Version:** water dev channel, watergram stable channel + `[patch]`
pins (waterui `b838d73`, hydrolysis `47b1081`, nami-core `969900ea`).

`water run` generates a managed crate at
`~/.water/build_cache/<app>/managed_backends/hydrolysis` with its OWN
`Cargo.toml` and `Cargo.lock`, and builds *that* as the binary. The app
is only a `path` dependency there, and `[patch.crates-io]` does not
propagate from non-root manifests — so the shipped binary resolves
**registry** nami-core 0.3.3 and **registry** hydrolysis 0.3.1 (no #115
icon-button fix, no dev waterui, no nami#23 fix) even though the app's
lockfile pins all three to dev revs. Observed: `cargo tree` in the
managed dir shows `nami-core v0.3.3 (registry)` after the app-side patch
was added; `water run` reproduced the *pre-fix* panic on the fixed nami
dev rev until the same patch was hand-added to the generated manifest.
Expected: the generated manifest inherits/merges the app's
`[patch.crates-io]` table (or `water run` builds the app workspace
directly) so a documented `[patch]` flow exercises the pinned revs end
to end. Workaround used this session (documented, not in the repo):
patch the generated Cargo.toml the same way and build the bin target
inside the managed dir.

---

## r13-2b: compressed single-line preview starves the spacer — badge off the trailing edge, no ellipsis

**Version:** waterui dev `b838d73`, hydrolysis `47b1081`, nami-core
`969900ea`. Probes: `tests::probe_row_bisect` (variants D vs E) and
`tests::probe_chat_row_badge`.

```rust
hstack((
    text("RC").padding_with(44.0),                    // avatar slot
    vstack((
        hstack((title, spacer(), "14:32")).spacing(4.0),
        hstack((preview.caption().line_limit(1), spacer(), badge))
            .spacing(4.0),
    ))
    .spacing(2.0)
    .leading(),
))
.spacing(10.0)
.padding_with((6.0, 10.0))
```

at a 340px viewport (the inner column is offered ~201.7 after the
avatar):

- title row: `14:32` placed at `(300.1, 259.3, 29.9, 14.1)` — spacer
  expands, text lands on the trailing edge at x=330. ✓
- preview row: preview text placed `(128.3, …, 141.7, 14.1)` — narrower
  than the ~193.7 share it could keep — `spacer` allocated ~0, badge
  `Image (277.9, …, 25.5, 18.1)` ends at x=303.4, **~27 short** of the
  trailing edge. The preview is hard-clipped with **no ellipsis**.
- Same shape with a preview that fits (`"call me when free"`): badge at
  `(311.3, …, 18.7, 18.1)` ends x=330 — correct. Only breaks under
  width pressure.

Expected: under `compress_to_fit` the leftover slack goes to the stretch
child (the spacer) so the badge keeps the trailing edge, and a
line-limited text ellipsizes at its box edge. Observed: slack is given
to no child (the hstack leaves ~26.6dp unused inside its placed box),
and `text().line_limit(1)` clips without an ellipsis — `text.rs` says
the ellipsis comes "where the backend's native text machinery provides
it", so the ellipsis half is a hydrolysis text gap; the spacer
starvation is the layout half.

---

## r13-3: `Menu` item actions cannot extract env state — popup env does not inherit `.state(&store)`

**Version:** waterui dev `b838d73`, verified under SemanticApp
(`tests::archive_toggle_rebuilds_list`).

```rust
Menu::new(
    label("Menu").icon(mdi::menu()).icon_only(),
    (
        "Archive".action(|store: Store| store.toggle_archive_view()),
        ...
    ),
)
```

Mounted under `views::sidebar_view(...).state(&store)`: the menu button
opens the dropdown, the item resolves — and tapping it panics:
`failed to extract watergram::state::Store from environment for action:
Environment state Store not found at position 0; install the value with
.state(&value) on an ancestor of the handler's view`. Installing
`.state(&store)` directly on the `Menu` widget does not reach the popup
either — the popup env is a fresh root, not a descendant of the widget's
env. Working pattern: zero-arg closures that capture the store
(`move || s.toggle_archive_view()`), navigation via a store-owned
`NavigationPath`. The same gap presumably applies to `context_menu`
`.action(|store: Store| …)` items (same popup-env mechanism) —
unverified: SemanticApp offers no way to open a context menu and tap its
items, and the ~60 context-menu action sites in the app are unchanged
until the real renderer can drive one. Whether the hydrolysis Menu
realization shares the env-rooting behavior is likewise open — the
finding is SemanticApp-verified.

---
## r14: keyboard + accessibility-tree findings

All verified with probe bounds/dumps; app-side fixes (decorative
`.a11y_hidden(true)`, Button-role promotion, real `button()` for chips,
`ModalInteraction` escape scopes on every overlay) are in views.rs.

### Semantic walk never registers `hit_test.modal_interaction` — modal
### Escape is untestable on `ui.mount` → water-rs/hydrolysis#147
### FIXED in hydrolysis dev e1ffa2c6 (verified r16)

`ModalInteraction` reaches `hit_test.modal_interaction` only inside
`bind_interaction_target_with_focus` (hit_test.rs:2275), which runs in the
*rendered* widget emit path. The headless semantic walk emits a11y nodes
from retained state via `emit_button_accessibility(ctx=None)` — no ctx, no
interaction-target binding — so on `ui.mount` the flag is always `None`
and `handle_keyboard_key_down`'s modal-Escape branch can never fire.
Minimal repro (now passes on `ui.mount` at e1ffa2c6):

```rust
let closed = waterui::reactive::binding(false);
let esc = waterui_backend_core::widget::ModalInteraction::new(
    true,
    waterui::handler::SharedAction::new(move |_: Environment| closed.set(true)),
);
ui.viewport(300, 300).mount(move || {
    vstack((button("Inside").action(|_: Store| {}),)).with(esc.clone())
});
app.settle(); app.press_named_key("Escape"); app.settle();
// expected: closed == true; observed (mount at e676ca8): false;
// observed (mount at e1ffa2c6): true.
```

Verified r16: `probe_modal_escape_minimal`, `keyboard_escape_dismisses_emoji`
and `keyboard_escape_dismisses_info` now run on `ui.mount` and pass.

### `press_named_key` emits `KeyState::Pressed` only — Enter/Space can
### never activate on rendered runtimes → water-rs/waterui#1222
### FIXED in waterui dev c634e557 (verified r16)

`key_press_event` (testing/driver.rs:288) emitted only `Pressed` — no
release event. Rendered runtimes (`mount_offscreen`, winit) use
`KeyboardActivation::PressRelease`: press arms on key-down, activation
fires on key-UP — so `press_named_key("Enter")` could never activate
anything there. `press_named_key` now sends a full stroke; the whole
keyboard contract (Enter activation + modal Escape) is testable on both
runtimes — `keyboard_enter_activates_offscreen` passes on `mount_offscreen`.

### `ui_focus()` reports text-input focus only — by design

`ui_focus()` is `focused_text_input_accessibility_node` — it stays `None`
when keyboard focus sits on a button/list item. Confirmed *intended*: a
dev-side test pins this contract. The a11y focus that covers every
focusable node is `app.tree().focus()` (`update.focus`); keyboard tests
must use it (fixed in `keyboard_tab_cycles_chat`).

### Every drawn shape/fill leaks an unnamed `Image` a11y node → water-rs/hydrolysis#148
### FIXED in hydrolysis dev e1ffa2c6 (verified r16)

`emit_graphics_image_accessibility` (renderer/tree/nodes.rs:984) emitted
`AccessibilityNodeRole::Image` for every graphics leaf — `.background(fill)`,
`Circle.fill` avatars, badge dots, selection pills — each an unnamed
role-Image leaf in the tree. At e1ffa2c6 the emit sites are only
SceneView/GpuSurface/custom-drawing content (flush.rs:292,350,565,597);
plain fills emit no node. App-side `.a11y_hidden(true)` calls that only
silenced fill noise were removed; the ones hiding icons/Photos (still real
nodes) stay. Audits rerun green.

### `text().on_tap` gesture targets: pointer-only by design; the modal
### gap is water-rs/hydrolysis#147

Gesture-only `on_tap` targets never call `bind_interaction_target` — they
get no Button role, no keyboard focus, no focus binding — confirmed
*intended* (pointer-only); `button()` is the keyboard-reachable form.
The real defect is the modal consequence: a `ModalInteraction` scope
registers only through a *descendant interaction target*, so a `when`
panel whose rows are gesture-only `text().on_tap` cannot be dismissed by
Escape (the scope never registers) — filed as hydrolysis#147. App fix:
real `button().style(ButtonStyle::Plain)` for those rows (panel_tab_chip
rewritten; verified Escape now closes the emoji panel on the rendered
runtime).

### No arrow-key navigation on list rows → water-rs/waterui#1223

`keyboard_arrows_chat_list` probe: `ArrowDown` on the sidebar `List` does
not move the selection (tree rows emit no Increment/Decrement/step
semantics the arrow path can use). Telegram Desktop moves the chat
selection with Up/Down. App-side this would need the framework to give
list rows arrow semantics — recorded, not worked around.

## r15-1: dynamically-mounted `NavigationSplitView` detail ignores the proposal — detail overflows the window → water-rs/hydrolysis#153

`measure_navigation_split_node` (hydrolysis `src/widgets/nav/navigation.rs:910`)
takes `_proposal: ProposalSize` — **unused** — and measures a
materialized detail through `measure_owned_navigation_view_intrinsic`
(intrinsic measure, no proposal bounds). Once a detail exists, any
re-measure therefore answers with its content's intrinsic size: a lazy
`List` reports its full content height, the split's answer exceeds the
pane, and the composer is pushed offscreen (r14chat1400.png: no
composer, message list runs off the bottom edge). On the real renderer
the bug appears even though the demo sets the selection before the
window maps — a post-map resize/re-measure is enough to hit the
ignored-proposal path.

Watergram repro at 1400×900 offscreen (`tests::probe_root_chat`):

```rust
NavigationSplitView::new(sidebar, move |id| chat_detail(store, id))
    .sidebar_width(ColumnWidth::new(260.0, 340.0, 420.0))
    .placeholder(...)
// mount, settle — then: selected.set(Some(1)); open_chat.set(1);
```

Observed (selection set post-mount): chat `List` `(340,150,1060,906.9)`
— 906.9 ≈ full content height — composer's `Attach file` at `y=1085`,
below the 900 px window; the sidebar `List` inflates identically
`(0,241,340,902)` vs `(0,241,340,661)` pre-mount.

Expected — same mount with `selected=Some(1)` set *before* mount
(`tests::probe_root_chat_pre`): `List` `(340,150,1060,665.9)`,
composer `(346,844,48,48)` inside the window. The pane rect must be the
proposal for a dynamically materialized detail exactly as for a static
one (§7). Not worked around — the app path is the repro.

## r16-4: `LazyStackNode::measure` relays an item's over-measured cross width — a multi-line (or trailing-spacer) item inflates the whole section

`LazyStackNode::measure` (hydrolysis `src/renderer/tree/collection.rs:766`)
reports the stack's cross extent as `sample.width` — item 0's measured
width from `ensure_estimate` → `measure_item(0, cross)` (collection.rs:660),
which proposes `ProposalSize::new(cross, None)` to the item. Two item
shapes answer more than `cross` to that proposal, and the lazy stack
relays the over-measure unclamped, so the enclosing `vstack`'s intrinsic
cross envelope (waterui `stack/distribute.rs` `measure_stack` +
`vstack_intrinsic_cross_metrics`) widens past its own proposal and every
sibling is laid out ~2.9 px wider — in Watergram's Settings this pushed
the Privacy/Notifications section's trailing values to x≈326.9 while the
Account section's ended at x≈324.

Watergram minimal repro (offscreen, M3, 340×900, all inside
`scroll(vstack(...).padding_with((12, 16)))` so content cross = 308):

```rust
VStack::for_each(rows, |row| {
    hstack((
        text!("{title}\n{subtitle}", ..),  // or vstack((text(a), text(b)))
        spacer(),
        text("current"),
    ))
})
```

Verified trigger matrix (same page, same 308 proposal):

| lazy item shape | section row width |
|---|---|
| `hstack((text("one line"), spacer(), text("x")))` | 308 (correct) |
| `hstack((text("a\nb"), spacer(), text("x")))` — multi-line first child | 310.86 |
| `hstack((vstack((text("a"), text("b"))), spacer(), when))` | 310.86 |
| `hstack((text("a — b"), spacer()))` — two children, trailing spacer | 310.80 |
| `hstack((vstack((text("a"), text("b"))), spacer()))` | 310.80 |
| identical row placed literally (non-lazy) | 308 |

Expected: an item must never answer a width above the cross-axis
proposal it was measured under; `LazyStackNode` should also clamp
`sample.width` to `cross` when reporting its cross extent. r16's app-side
workaround (a flat single-line row) was reverted in r17 — the honest
two-line row is restored and stays in the app as the repro; the defect
is being filed upstream.

## r17-2: the 72×40 "menu pill" in r16 screenshots was a stale-binary artifact — the generated managed-backend crate carries its own `[patch]` table (water-rs/cli#178)

r16chat1400/r16info800 showed the top-left navigation item as a filled
pill ≈72×40 while offscreen probes at the same rev measured the sidebar
`Menu` at `(4,8,48,48)` with a centred 40 dp circle — the hydrolysis#149
contract. The pill was the **pre-#149 Menu trigger drawn by an old
binary**: `water run`'s generated crate at
`~/.water/build_cache/.../managed_backends/hydrolysis/Cargo.toml` has its
own `[patch.crates-io]` table that `water run` does not refresh from the
app's `[patch]` (cli#178) — it still pinned the r14 revs (hydrolysis
e676ca88 + waterui a3e2818c, both pre-#149) while the app's `[patch]`
had moved on. So every real-renderer capture since ran hydrolysis older
than the app's pins. (The stale-table root cause — cli#178 — is fixed
upstream by cli PR #179; verified on cli dev `0414a255`, see r13-1's
RESOLVED note. No manual sync step is needed any more.)

App side is correct: `Menu::new(label("Menu").icon(icon).icon_only())`
→ `icon_only` → `icon_button_metrics` = `ButtonMetrics(0,0,48,48)`,
`draw_chrome` 40 dp primary circle (hydrolysis-m3 `src/icon_button.rs:88-160`,
`src/lib.rs:480-510`; trigger path hydrolysis `src/widgets/button.rs:276-330,
835-870,955`). After repinning the generated crate by hand, the real
renderer draws the 48×48 circle. Lesson: the managed-backend patch table
must be kept in sync manually until cli#178 lands — kept out of the repo.
(cli PR #179 fixed it; verified on cli dev `0414a255`.)

## r17-3: chips row clipping "Archive" → "Archiv" is the scroll viewport, not a truncation defect

The sidebar chat-filter row is `hstack((scroll_horizontal(chips),
button("New folder")))` — a horizontal scroller by design (Telegram
Desktop scrolls folder chips too). At a 340 px sidebar the probe shows
chips All (34.4) / Work / Personal / Archive (59.7) ≈ 296 px of content
vs a ~236 px scroll viewport → ~55 px genuine overflow. "Archive"'s
button bounds `(186.9,133.5,59.7,20.1)` extend past the viewport edge
(x≈244), so the scroll clip cuts mid-glyph → "Archiv". This is correct
scrolling behaviour: the waterui#141 ellipsis fix governs label
truncation inside a text view, not scroll clipping — pinning it will not
change this row. No app or framework fault. (A trailing fade affordance
at the scroller edge would be a nicety, not a defect.)

## r17-4: "5 members" flush to the info-panel edge — resolved by the 8ae78f4 / ab98d5fb repin

The members-section header row has had `.padding_with((4.0, 14.0))` all
along, yet r16info800 showed the trailing "5 members" at the panel's
right edge — a trailing-allocation defect of the same family the
rule-D stack allocation (#1224) / layout conformance (#1231) fixes
address. On the new pins the label measures `(1325.5,132,60.5,14)` inside
a panel ending at x=1400 → text ends exactly 14 px before the edge, and
the 800 px overlay capture shows the same inset. App was never at
fault; no DOGFOOD repro needed — resolved upstream.

## r17-5: geometry that moved on waterui 8ae78f4 + hydrolysis ab98d5fb

- **The composer is back** on the chat page at 1400 and 800 — the
  NavigationSplit intrinsic-measure defect (r15-1, hydrolysis#153) no
  longer pushes it below the viewport.
- All icon buttons render at the 48 dp touch target with the centred
  40 dp state-layer circle (menu ☰, chat-header search/members/
  scheduled/info, composer 📎, mic/camera).
- Chat-list previews now end in a real ellipsis ("…") where clipped —
  the #141 truncation fix is in this build.
- Info-panel member rows render avatar circle + two-line name/status —
  the phantom indent is gone.
- Member count trailing padding restored (r17-4).
- Colour emoji glyphs now render (🚀 etc.); DejaVu fallback for
  default-emoji codepoints is improved.
- A scrollbar now draws on the message list.
- Settings "Set avatar"/"Save"/"Log out" render filled pill buttons;
  notification toggles render M3 switches with the check thumb.
- Unchanged: chips row still shows "Archiv" by scroll design (r17-3);
  "Watergram" title still sits flush after the 48 dp leading item per
  `NAVIGATION_BAR_ITEM_SPACING` = 0 (r16-1 verdict — spec-correct).

## r18-1: nav-bar subtitle is not realized in the semantic tree

`NavigationView::navigation_subtitle(text)` renders under the title on
the real renderer (winit), but `ui.mount`'s semantic tree contains no
node for it: `resolve_elements` on `chat_detail` shows `Header
"dogfood crew"` plus toolbar buttons and content — no `"5 members"`
label anywhere (probe: `dump_bounds` after mounting chat_detail).
Title is realized, subtitle is silently absent, so subtitle content is
untestable via `#[waterui::test]` queries. Sibling of waterui#1213:
the semantic runtime realizes only part of the navigation chrome.

Filed upstream as a hydrolysis issue (the nav-bar subtitle is not realized
in the semantic tree); the signal-level `chat_header_subtitle` test stays
until it lands.

## r19-1: hydrolysis dev never merged the List::selection r2 head — dev + r2 is a needed upstream merge

waterui#1233 removed `ListItem.selected` (selection moved to
`List::selection(&Binding<Option<Id>>)`), but hydrolysis dev head
`cb5db15` still reads `item.selected` at
`src/widgets/layout/list.rs:795` and `:1125` — the adaptation lives on
the unmerged r2 head `cb872581` (feat(list): render the
framework-owned row selection, `5df4599` + semantic held-modifier
tracking `cb872581`), which predates dev's #157/#158/#159. So no
single upstream rev satisfies "waterui ≥ 1ece08f0 AND hydrolysis with
selection + ellipsis + torn-frame fixes":

- `hydrolysis = rev 2b05cf0` + `waterui* = rev 5a1ec24b` fails to
  compile: `error[E0609]: no field 'selected' on type ListItem`
  (`hydrolysis/src/widgets/layout/list.rs:795`, `:1125`).
- `hydrolysis = rev cb872581` compiles the selection API but drops
  #157 (truncation ellipsis), #158 (when()+shared-signal torn frame)
  and #159 (scroll/lazy cross-proposal sizing).

RESOLVED upstream (r20): hydrolysis dev `2e6f401` carries #165 (the
ListItem→`List::selection` compat merge) plus #157/#158/#159/#160/#161,
so a single upstream rev now compiles — `vendor/hydrolysis` is deleted
and the app is back on the git pin. The local
`fix/merge-list-selection-r2` branch is retired.

## r19-2: `List::apply_scroll_request` asserts on a transient `row_count` — a pending scroll target is not a programmer error

Repro (real renderer, deterministic): `WATERGRAM_DEMO=1
WATERGRAM_DEMO_PAGE=list watergram-hydrolysis`, then click any chat
row → panic:

```
thread 'main' panicked at hydrolysis/src/widgets/layout/list.rs:565:13:
List scroll target 8 exceeds collection length 0
```

backtrace: `apply_scroll_request` ← `list_accessibility` ←
`render_list_node` ← `RenderNode::flush` (the a11y flush applies the
scroll request before the collection catches up).

Why it fires: `row_count` is read from the list's contents signal
(`list.rs:741-742`), and a scroll target lives in
`ScrollController.target` until consumed. Watergram opens a chat by
clearing `messages` and re-seeding — the message `List` materializes
mid-flush with `row_count == 0` while a stale target (`scroll_to(8)`
from the seed) is still pending → `assert!(index < row_count)` panics
on a state the app cannot avoid: contents is signal-driven, so 0 is a
*transient* length, and a target written before a shrinking update is
stale, not invalid. Both asserts live identically on upstream dev
(`2b05cf0`, `list.rs:460`/`:469`) — pre-existing, only surfaced now
that the selection path opens chats by pointer.

Filed upstream as water-rs/hydrolysis#168 — **FIXED upstream** at
hydrolysis dev `3ba17866` by #178 (a pending scroll target waits for
its row). Verified end-to-end on the real renderer: pointer tap on the
"WaterUI devs" chat row opens the conversation — no panic, full message
history rendered (`r21/chat_open_click.png`). Closed.

## r20-1: filter chip "Archive" cuts to "Archiv" mid-glyph with ~40 pt of slack — no ellipsis, no compression

Repro (real renderer, `r19/sel_down3.png`, 1400-wide): the sidebar filter
row is `scroll_horizontal` over chips; the "Archive" chip is clipped at
the scroller's right edge mid-glyph ("Archiv") while ≈40 pt of free
space sits between the chip row and the trailing folder icon. So the
row had room to compress or the scroller to reveal more, yet the chip
truncates without an ellipsis and without the layout granting the free
space to the scroll viewport.

Filed upstream with the water-rs/hydrolysis#168 session. No app-side
workaround — the chips stay `scroll_horizontal` by design (Telegram
Desktop scrolls them too); the defect is that the viewport edge clips a
glyph instead of (a) getting the available width, or (b) ellipsizing the
clipped label.

**r21 re-verification (hydrolysis `3ba17866`, #179 landed): still
reproduces.** #179 ("a vertical scroll's minimum width is its content's")
covers a different axis — it does not apply to this horizontal scroller.
Measured on `ui.mount` at 340 pt (`probe_chips_bounds`): the ScrollView
viewport is x8..x244 (w=236); the "Archive" chip button is x186.9..x246.6
and its label x196.9..x236.6 (39.69 — the semantic layout claims it
fits); the "New folder" button sits at x262, leaving ≈18 pt of unclaimed
slack between the viewport edge (x244) and the button. On winit the
label renders ~6 pt wider than the semantic measure → its 'e' crosses
the viewport edge and paints "Archiv" (pixel-verified: text ends at
x243 = the scroller's clip edge). Two live defects, both framework:
the hstack does not grant its leftover to the scroll viewport, and the
text node's semantic measure under-reports the winit glyph width.

Filed as **water-rs/hydrolysis#182** (the chip clips in a horizontal
scroll: the viewport misses the leftover width, and the semantic
runtime's text measure differs from what the renderer paints). Fix
session running. Still open.

**CLOSED (r26, hydrolysis `eb962313`, #186 landed):** the nested scroll
now reports its content's extent — the horizontal chip scroller is
sized from its content, so the "Archive" chip is inside the viewport
and reads whole. Re-verified on the real renderer at 1400, 800, 600
and 420 (`r26/chat1400_new.png`, `chat800_new.png`,
`chat600_new.png`, `list420_pre.png`): "Archive" paints complete at
every width — no mid-glyph clip anywhere, so there is nothing the
rail needs to scroll to.

## r20-2: `on_tap` targets inside a `List` row are unreachable on the real renderer — the row's press target consumes the pointer event

Repro (real renderer, deterministic): `WATERGRAM_DEMO=1
WATERGRAM_DEMO_PAGE=chat watergram-hydrolysis`, then tap either of two
nested interaction targets inside a message row — the reaction pill
("👍 3", `views.rs` chip hstack, `.on_tap → toggle_reaction`) or the
photo media slot (`.on_tap → open_viewer`). Neither fires: no reaction
toggle, no viewer. Verified on two separate pointer taps at the glyphs'
bounds (`r20/react_tap2.png`, `r20/photo_tap.png` — identical before/after).

The rows are `ListItem`s inside the message `List`; the row registers a
press target (that is what makes row focus/selection work — r19). On the
winit renderer the row-level press resolves first, so inner `on_tap`
targets inside the row content never receive the event. The semantic
runtime dispatches them correctly — `reaction_chip_tap_toggles`
(`#[waterui::test]`) taps `"React with 👍"` via `query().tap()` and the
chip toggles both ways.

Contrast: Telegram Desktop lets you tap a reaction pill on a bubble to
toggle it. The app wiring is correct (same path the context-menu React
items use); only the real-renderer hit test fails to reach it.

Upstream: innermost-target-wins dispatch for pointer press inside
`ListItem` content (or a documented way for a row to host nested tap
targets). Filed as **water-rs/hydrolysis#175**; fix session running.
No app-side workaround.

Re-verified on hydrolysis `3ba17866`: pointer tap at the "👍 3" chip's
bounds still does not toggle the reaction (`r21/react_tap2.png` —
count unchanged); a tap on a bubble body instead scrolls/presses the
row. Still open. Fix session running on hydrolysis `c19e8437`.

**FIXED upstream** — hydrolysis dev `5fd1c37f` (#183: innermost pointer
target wins inside a `ListItem`). Verified live on the real renderer at
1400×900:

- pointer tap on the "❤ 1" reaction pill under the 🔥 message toggles it
  to the chosen **accent "❤ 2" pill** (`r22b/after_drag.png`); the row
  content change re-anchors the list to the unread divider.
- pointer tap on the photo media slot ("photo · 182 KB" pill) opens the
  media viewer overlay — "Alice" header, Close button, "Downloading…"
  body, "photo.jpg" caption (`r22b/viewer.png`).

Both fires land on the nested `on_tap` inside the `List` row, matching
the semantic runtime. Closed.

## r20-3: `NavigationView` emits an empty subtitle `Label` node when no subtitle is bound

On the r20 pins (waterui `2912a678` + hydrolysis `2e6f401`) the nav-bar
subtitle slot IS now realized in the semantic tree — r18-1's fix half
landed. But on a page that binds no subtitle (settings, and any other
`NavigationView` without one) the slot materializes as an empty `Label`
node directly under the Window (`#2 Role(Label) '' bounds=None`) —
unbound placeholder noise for assistive tech.

Repro: `a11y_audit_settings` dump on the r19 pins had no such node; on
the r20 pins it is present on every nav page with no subtitle. The same
regression makes an empty `text("")` emit an empty `Label` — the empty
`profile_note` was the first one the audit caught (the app gates it with
`when` because Telegram shows no note until there is feedback, which is
also correct product behaviour). Expected: neither an unbound subtitle
slot nor an empty text emits a node. Filed as **water-rs/hydrolysis#176**;
fix session running. The audit has NO skip — `a11y_audit_settings`
fails honestly on this node until the fix lands.

**FIXED upstream** — hydrolysis dev `c19e8437` (#181: no empty `Label`
nodes). Verified: `a11y_audit_settings` and all four audits pass again
on `ui.mount` (the r22 suite: 4/4 audits ok). The `profile_note` `when`
stays as product behaviour (Telegram shows no note until there is
feedback), not a workaround. Closed.

## r21-1: sidebar `Menu` inline icon row — RETRACTED (stale-binary artifact)

The r21 captures that showed the `Menu`'s items painting as a persistent
inline icon row under the search field ran a stale binary: the managed
backend's launch artifact had been copied from
`~/.water/build_cache/target/shared/...` (a Sep-23 build at the r14-era
pins), not from the managed crate's own `target/debug/` output. On the
real r21 pins (hydrolysis `3ba17866`, m3 `94bde7b`, waterui `70c3b7c`,
nami `5dc2d92e`) the icon row is ABSENT — the sidebar shows only the
48 dp `Menu` icon-button trigger in the top bar (`r21/list_3ba1786.png`),
which is the m3#87 contract verified. What appeared in the stale build
was the pre-r19 `Menu`-trigger fallback: at those pins `icon_only` was
unimplemented, the 72×40 trigger showed the first item's glyph, and the
companion items painted beside it — matching every observed detail
(~80 pt pitch, item glyph semantics, Settings landing past the sidebar
edge). No framework defect at the current pins; the earlier write-up is
kept below for the record.

Lesson recorded: the managed backend's real build output lands at
`$MB/target/debug/watergram-hydrolysis-<hash>` (where `$MB` is the
managed crate dir), and only that path proves a binary matches the
pinned sources — verified via `ls -la` on the artifact copied to
`dist/linux/debug/`. Filed upstream as **water-rs/cli#181** (the
managed backend launched a stale binary from the shared build cache);
fix session running. Until it lands, each round's report states the
launched artifact's path and mtime. The stale `~/.water/build_cache/
target/` cache (45 GB) was deleted this session.

**CLOSED (r26, cli `b17efc04`, cli#182 landed):** the mechanism is
verified — the launched binary is this build's own output (fresh
`Finished` + full relink every `water run`; first run logged "cache
changed shape, cleaning" and rebuilt the managed crate from scratch),
and the staged `resources/` (including `resources/fonts/
Roboto-Variable.ttf`) is copied beside the packaged executable at
`dist/linux/debug/`, so the renderer now measures with the app's own
fonts. The cli#182 work surfaced two NEW cli defects — r26-3 (env
`RUSTFLAGS` silently replaces user `[build] rustflags`) and r26-4
(the staged `libwaterui_dylib.so` does not match the binary's
hash-suffixed `DT_NEEDED` name) — both for upstream filing.

<details><summary>Superseded r21-1 text (stale binary)</summary>

On the r21 pins the `sidebar_stack` navigation toolbar carries one
`NavigationToolbarItem::new(TopBarLeading, Menu::new(label("Menu").icon
(menu()).icon_only(), items))` (`src/views.rs:261-287`). On the stale
build the Menu trigger was not drawn in the top bar and the menu's items
painted as a persistent inline icon row under the search field at
y≈153: `+`  📦  👤  `Online`  ⚙ at x≈48/128/208/290/360 — ~80 pt pitch,
purple Material glyphs, with a muted `Online` text co-located between
the account and cog glyphs; the last glyph landed past the 340 pt
sidebar edge. Taps fired the items' commands (New chat / archive view /
accounts / Settings). The row was absent from the semantic tree and
from `mount_offscreen`.

All of it is now explained by the pre-r19 Menu-fallback behaviour of
the stale build; nothing was a defect of the current pins.

</details>

## r23-1: emoji-only timestamp detached from its glyphs — APP BUG (fixed)

**Report:** on `r22b/chat1400.png` the outgoing 🎉🎉🎉's meta row
("09:49 ✓✓") sat at the far right edge (~x=1360) ~40 pt below the glyph
run (ends ~x=1065); the incoming 🔥's "09:50" sat ~390 pt right of its
glyph. Telegram Desktop anchors the meta row at the trailing edge of the
content run.

**Diagnosis (bounds evidence):** the probe `probe_emoji_bubble`
(`src/lib.rs`) dumps the real `message_bubble` view tree. The bubble's
content is a `zstack((content, meta_overlay))` where the `meta_overlay`
was wrapped in `Frame::new(...).max_width(f32::INFINITY)` — intended to
right-anchor the meta row. But
`ZStackLayout::size_that_fits`
(`components/foundation/layout/src/stack/zstack.rs:86-96`) returns
`f32::INFINITY` when ANY child measures infinite ("a layer that answers
an unbounded extent makes the stack answer it too"). The infinite child
inflated the whole stack to its proposal — the outer `bubble_cap`
(~480 pt at 1400) — so the meta row anchored at the cap's trailing edge
while the bubble's own fill and content stayed content-sized. The bubble
looked narrow but reported wide: `glyphs x≈778-942` vs `meta x≈937-980`
on a ~184 pt content run inside a ~220 pt measured stack (probe);
~480 pt at real window width. App misuse, not a renderer defect —
finite children ARE aligned inside the resolved bounds.

**Fix (`src/views.rs` `message_bubble`):** `meta_overlay` is now a
finite zstack child and the stack takes a shared
`.alignment(BottomTrailing)` — stack width = max(content, meta), and
both finite children align to the stack's bottom-trailing edge, so the
meta row hugs the content run's trailing edge:

```rust
let bubble_inner = zstack((
    zstack((content, reactions_overlay)).alignment(BottomLeading),
    meta_overlay,
))
.alignment(BottomTrailing);
```

Alternatives considered and rejected: `overlay(base, layer)` keeps the
decoration at ITS OWN size aligned inside the base's bounds — a meta
wider than the content (e.g. `edited 09:43 ✓✓` under `hi`) overflows
outside the bubble's bounds instead of widening it (probe
`probe_meta_wider_than_content`: meta at x=-64.4 vs base 0-97). The
shared-alignment zstack contains the meta in every case; when meta
exceeds content the content right-aligns under it — the accepted edge
case.

**Verified:** `r23/chat1400.png` — "09:49 ✓✓" sits directly under the
🎉🎉🎉 trailing edge, "09:50" under the 🔥 (`r23/chat800.png`,
`r23/chat600.png` confirm at all widths).

## r23-2: media-viewer scrim almost transparent — APP BUG (fixed)

**Report:** on `r22b/viewer.png` the chat behind the viewer stayed
readable (poll, bubbles, 🎉). Telegram Desktop's media viewer is an
opaque dark overlay with a light header and caption.

**Diagnosis:** the app styled the scrim
`WithOpacity::new(Color::srgb_hex("#FAFAFA"), 0.97)` — a 97%-white fill
with ~3% bleed-through. `WithOpacity::new(color, opacity)`
(`components/visual/graphics/src/color/mod.rs:144-154`) documents 0 =
fully transparent, 1 = fully opaque; the renderer blended the fill
exactly as asked — alpha honored, not a framework blend bug. The defect
was the app's colour choice (light, near-transparent), not the pipeline.

**Fix (`src/views.rs` viewer overlay):** opaque dark scrim
`#101010` + white header row (sender + white ✕ close) + white caption —
Desktop's presentation. Verified `r23/viewer1400.png`: nothing of the
chat bleeds through; "Alice" header, ✕, "Downloading…", "photo.jpg"
caption all light on opaque dark.

## r23-3: message grouping + sender avatars (pick)

The most visible remaining gap vs Desktop's chat view: every incoming
bubble repeated the sender name and groups showed no avatar column —
the list read as flat uniform bubbles. `set_messages` now computes run
flags (`group_first`/`group_last`/`avatar_col`/`show_avatar`); the
sender name renders only on a run's first row, groups/channels reserve
a bottom-aligned 32 dp avatar slot on the run's last row
(`sender_photo`, initials fallback), and same-run rows pack at ~1 pt vs
~4 pt between runs. `Store::regroup_messages()` re-passes when the demo
seed's `open_chat` lands after `set_messages`. Test:
`set_messages_groups_runs`. Captures: `r23/chat1400.png`,
`r23/chat800.png`, `r23/chat600.png`.

**Still open — cites:** hydrolysis#182 ("Archiv" chip clips mid-glyph in
the horizontal-scroll chips row — visible in this round's captures too,
`r23/chat1400.png` x≈243), cli#181 (launched-artifact staleness — this
round's artifact: `$MB/dist/linux/debug/watergram-hydrolysis`, mtime
2026-09-24 20:36 UTC, 171,101,608 bytes), hydrolysis#130
(no-wrap-in-cap), waterui#1214 (`max_width` signal sampled at mount),
nami#23, water-rs/cli#178 (verified fixed).

## r24-1: reaction chips and the meta row collided on one line — APP BUG (fixed)

**Report:** on `r23/chat1400.png` the 🔥 row drew "❤ 1" at x≈415–440 and
"09:50" on top of it at x≈436–470 on the same baseline (same at 600).
r23's fix anchored chips `BottomLeading` and meta `BottomTrailing` in the
SAME ~20 pt reserved zone — disjoint anchors, shared line → collision
whenever the chips row ran long enough to reach the meta's trailing x.

**Fix:** the two overlays now reserve two SEPARATE lines inside the
bubble's bottom inset: chips occupy the zone directly under the content
(`BottomLeading`, padded up by `meta_band`), the meta row the band below
(`BottomTrailing`) — Desktop's "time drops below the reactions"
arrangement for every chip count. Overlap is impossible by construction;
the bubble hugs `max(content, chips, meta)` exactly as before.

**Why not a shared hstack `(chips, Spacer, meta)` band — measurement
evidence:** a `Spacer` (or `StretchAxis::MainAxis` member) inside
`measure_stack` is handed the WHOLE main-axis offer, not the leftover
(`waterui components/foundation/layout/src/stack/hstack.rs` +
`stack/distribute.rs`, confirmed empirically): with the shared band the
bubble measured at its ~480 pt `max_width` cap on every row — all
bubbles stretched to uniform width and the meta detached again. The
greedy member is consistent with what it reports (it answers the offer),
so this is recorded as framework behaviour to design around, not an
inconsistency bug; a `space_between`/flow layout primitive does not
exist in waterui.

**Verified:** `reactions_band_never_overlaps_meta` mounts the bubble at
600 pt for 1/3/6 chips × text/emoji-only bodies and asserts every chip's
bounds are disjoint from the meta rect and equal to the laid-out chip
bounds. Captures `r24/chat1400.png`, `r24/chat800.png`,
`r24/chat600.png` — chips line under the content, "09:5x" on its own
line below, at every width. At 600 the six-chip row compresses (count
wraps under the glyph) rather than clipping — accepted.

## r24-2: tap a reply quote to jump to the original (pick)

Desktop scrolls to and flashes the original message when a reply quote
is tapped. `MessageRow` gained `reply_to_id` (from
`MessageReplyToMessage.message_id`); the quote block is now an accent
bar + excerpt with `on_tap` → `Store::jump_to_message`, which takes a
new local fast path — highlight + `scroll_to` without a fetch when the
target is already in `messages` (the async `getChatHistory` path stays
for unloaded targets). Demo: m22 quotes m21 inside the visible window.

**Verified:** `reply_quote_jumps_to_loaded_message`; live on the real
renderer — tapping m22's quote flashed m21's bubble
(`r24/replyjump.png`, highlight vs `r24/chat1400.png`).

**Still open — cites:** hydrolysis#182 ("Archiv" chip still clips
mid-glyph, `r24/chat1400.png` x≈243), cli#181 (artifact staleness —
this round's launched artifact:
`$MB/dist/linux/debug/watergram-hydrolysis`, mtime 2026-09-24 21:46 UTC,
171,111,976 bytes), hydrolysis#130, waterui#1214, nami#23.

## r25-1: reaction chips rendered bare inside bubbles — APP BUG (fixed)

**Report:** on `r24/chat1400.png` chips under the emoji-only rows drew as
pills but the SAME chips inside text bubbles drew as bare "👍 1" text.

**Root cause:** the unchosen pill filled `SurfaceVariant` — identical to
the incoming bubble's fill (`views.rs` bubble background), so the pill
was present but invisible inside a bubble. One chip component already
served both placements; the tint just didn't account for the surface.

**Fix (`reaction_chip` + `ChipSurface` in `src/views.rs`):** one
component for all three surfaces — `Page` (emoji-only rows) keeps
`SurfaceVariant`; `Incoming` uses `Surface` (page tint shows through the
variant bubble — Desktop derives the pill tint from the bubble colour);
`Outgoing` uses translucent on-accent `WithOpacity(AccentForeground,
0.18)` + `AccentForeground` text. Chosen is accented in every placement
(`Accent`/`AccentForeground`; on the accent bubble `AccentContainer` +
`Accent` text, since accent-on-accent would vanish). Demo seeds a
chosen chip in each placement (m14 outgoing, m21 incoming, m24 page).

**Verified:** `r25/chat1400.png` — "👍 1" reads as an accent pill inside
m21's bubble, variant pills on m22/m23, chosen accent "👍 1" + variant
pills under 🚀🚀, chosen `AccentContainer` "👍 3" on m14's accent
bubble (`r25/jump_far.png`). Same at 800 and 600.

## r25-2: reply-jump verified with an out-of-view target + fetch path

**Requirement:** r24's verification only showed a same-screen
highlight. Now: m23's quote targets m10, ~9 rows above the viewport.
Before: `r25/chat1400.png` (m21–m25 visible, m10 offscreen). Tap → the
list scrolled m10 to the top AND flashed it — `r25/jump_far.png` shows
m10 highlighted at the top under the "Yesterday" pill. The
`ScrollController<usize>` index path works end to end on the real
renderer.

**Unloaded target:** m25's quote targets id 9, not in the loaded
window. Tap → the async path ran `getChatHistory(chat, 9, -20, 40)` on
client 0, which fails (demo has no TDLib client) → no-op: list
unchanged (`r25/jump_fetch.png` identical to the pre-tap frame). On a
live client the same code merges the 40-message window, sorts,
highlights, and scrolls — the merge/sort/scroll tail is identical to
the verified fast path, only the fetch result differs. Honest result:
the fetch path is wired correctly but unreachable in demo; real-login
e2e remains blocked on api_id/api_hash + test account.

## r25-3: bubble tails on a run's last row (pick)

Desktop hooks the bubble's bottom outer corner toward the sender's
avatar (incoming) or the screen edge (outgoing) — the most distinctive
missing chat shape. `bubble_tail` draws it as a `Path` (inner edge
flush with the bubble, `quad_to` concave outer edge tapering to the
tip) in the bubble's own fill, only when `group_last` and a bubble
exists. The 8 pt gutter is reserved on EVERY row (invisible box when
tail-less) — any element in an hstack contributes width, and a
permanent reservation keeps bubble edges aligned across a group, the
same trade the avatar column already makes.

**Verified:** `r25/jump_far.png` — incoming tails bottom-left toward
the avatar on m10/m12/m14, outgoing tails bottom-right on m11/m13/m15;
`r25/chat1400.png` — tail on m23; none on the emoji-only rows. Same at
800 and 600.

**Still open — cites:** hydrolysis#182 ("Archiv" chip still clips
mid-glyph, `r25/chat1400.png` x≈220), cli#181 (artifact staleness —
this round's launched artifact:
`$MB/dist/linux/debug/watergram-hydrolysis`, mtime 2026-09-24 22:45 UTC,
171,138,192 bytes), hydrolysis#130, waterui#1214, nami#23.

## r26-1: bubble tails re-attached — `Path` wedge as a background layer + `.offset`, no reserved layout space

**User review of r25:** the r25 tails were detached — a small triangle
hanging *below* the bubble's bottom border, ~45 px from the corner, and
the 8 pt gutter hack reserved layout space on every row.

**Root cause of the r25 detachment:** the tail `Path` lived in a
sibling `tail_slot` column *beside* the bubble (`hstack((bubble,
tail_slot))`), and `PathCommand` coordinates are **normalized
(0.0–1.0)** against the view's bounds (`shape/src/lib.rs:55`) — feeding
absolute 0–6/0–11 point values drew a sliver stretched 6×/11× inside
the frame, landing under the bubble rather than on its edge. Lesson:
normalized path coords scale with whatever frame the view gets — pin
the frame (`.size(6,11)`) to get absolute geometry.

**Reconstruction (user's option (b)):** the wedge is now a layer of the
bubble's own `zstack` background — same `fill` as the rounded rect, so
fill + tail read as one shape — positioned by a `hstack((wedge,
spacer()))` row inside a `vstack((spacer(), row))` so it sits on the
bottom edge, then `.offset(±5.0, 0)` pushes it outward past the bubble
edge (`Offset` is purely visual, `view.rs:1023`; hydrolysis translates
it at `flush.rs:129` — no layout space is reserved, so the gutter and
the invisible `Surface` box are gone entirely). The wedge's flat side
lands 1 pt inside the edge so no antialiased seam shows; the joined
corner is drawn square (radius 0) via `UnevenRoundedRectangle::new`
while the other three corners keep 0.18 — Desktop's geometry.

Both of the user's offered constructions were available — no framework
limitation hit: (a) a single `Path` outline would have worked too
(normalized coords aside), and (b) an overlay/background layer may
extend past its base's bounds unclipped (confirmed: the offset wedge
draws ~7 pt outside the zstack's bounds on the real renderer).

**Verified (rebuilt at hydrolysis eb962313; wedge retuned to 10×13,
offset ±7 — a ~6 pt hook, closer to Desktop's sweep):**
`r26/tail_in_m12.png`, `r26/tail_in_800.png`, `r26/tail_in_600.png`
(incoming hook grows tangentially out of the bottom-left corner —
square join, down-left sweep toward the avatar gutter, at 6×),
`r26/tail_out_m13.png`, `r26/tail_out_800.png`,
`r26/tail_out_600.png` (outgoing mirror on the bottom-right — a ~6 pt
sweep, tip level with the bubble bottom, Desktop's shape). Full
captures `r26/chat1400_jump4.png`, `r26/chat800_jump2.png`,
`r26/chat600_jump.png` — all reached by tapping the m23→m10 reply
quote (the loaded fast-path scroll re-verified in the same shots).

## r26-2: per-peer accent colors (pick)

Desktop assigns every peer one of seven accent colors — the colored
userpics in the chat list/members/message sender column and the group
sender names. `peer_color(accent, name)` in `src/views.rs` maps TDLib's
`accent_color_id` (`id % 7`) over the userpic palette
(red/orange/violet/green/cyan/blue/pink); when the peer carries none
(demo rows, unresolved senders) the display name is hashed instead, so
a peer keeps one color everywhere it is drawn — "Alice" matches in the
chat list, the group bubble name and the member row. Rows carry the
real accent id end to end: `ChatRow.accent` ← `chat.accent_color_id`,
`MessageRow.sender_accent` ← sender user/chat accent,
`MemberRow.accent` ← resolved user/chat accent; the avatar component
takes the accent and falls back to its `title` hash.

**Verified:** `r26/chat33.png` — every chat-list userpic a distinct
color (WD cyan, Alice orange, Saved pink, News green, RC red, Bob
blue, DC violet), "Alice" sender name + run avatar cyan in the group,
member avatars colored. Same at 800 and 600.

**Still open — cites:** hydrolysis#130, waterui#1214, nami#23, plus
cli#183 (env `RUSTFLAGS` drops user config rustflags — fix in
progress; session workaround stays) and the new staging defect in
r26-4 (staged dylib name ≠ `DT_NEEDED`). r20-1 and r21-1 closed this
round: hydrolysis#186 (via #182) verified — the "Archive" chip reads
whole at every width; cli#182 verified — the launched binary is this
build's own output and `resources/fonts` sits beside it.

## r26-3: `water run` drops `[build] rustflags` from cargo config — env `RUSTFLAGS` replaces them (**water-rs/cli#183**)

**Finding (cli bug):** at cli `b17efc04`,
`water run --platform linux` fails the final link of
`watergram-hydrolysis-a0f23df4` with
`undefined symbol: __isoc23_strtol` out of tdlib-rs's bundled OpenSSL
objects — **only because** the user's own `[build] rustflags` never
reached rustc.

**Repro:** put link flags the binary needs in a cargo config file —
here `~/.cargo/config.toml` carries
`-L /home/ubuntu/.cargo-shims -l dylib=shim_c -l dylib=shim_cpp`
(shim libs supplying `__isoc23_strtol`/`__isoc23_strto*` and libc++
verbose-abort symbols; tdlib-rs 1.4.0's `download-tdlib` prebuilt
archive is compiled on a glibc≥2.38 toolchain while this box runs
glibc 2.35, so the shims are required to link and to run). Then
`water run --platform linux` → the link step errors.

**Root cause:** `src/workflows/build.rs:1520-1526` in the cli injects
the new shared-runtime flags by *setting the `RUSTFLAGS` env var*
(`-Cprefer-dynamic -Crpath`, added by `with_preferred_dynamic_linking`
at :1076 — part of the cli#182 dist work). Cargo treats env
`RUSTFLAGS` and config `build.rustflags` as **mutually exclusive
sources** — env wins, so every flag in the user's config files is
silently dropped from every unit in the managed build. The `--config`
file args the cli also passes load the files for other keys but do not
save `build.rustflags` from the precedence rule.

**Not a cli#182 regression (corrected after user review):** the same
env-`RUSTFLAGS` injection sits at the same place before #182
(`b17efc04^1`, build.rs:1461) — the earlier builds likely reused
cached units, so the link only broke now that cli#182's cache-clean
forced a full relink. Filed upstream as **water-rs/cli#183**; fix in
progress.

**Impact:** any user whose cargo config carries rustflags — custom
linker, `-Ctarget-cpu`, `-L`/`-l` shims — loses them under the cli;
the failure surfaces as unrelated-looking link or behavior bugs.

**Session workaround (not committed, kept until cli#183 lands):**
export `RUSTFLAGS` with the config flags before `water run` — the cli
*prepends* the existing env value (build.rs:1521) then appends its
own, so the shims link again.

## r26-4: packaged app won't launch — staged `libwaterui_dylib.so` name does not match `DT_NEEDED` (cli#182 regression)

**Finding (cli bug, for upstream filing):** at cli `b17efc04`, the
packaged artifact `dist/linux/debug/watergram-hydrolysis` exits 127 on
launch: `error while loading shared libraries:
libwaterui_dylib-0267d27d87854afc.so: cannot open shared object file`.

**Repro:** `water run --platform linux` on a project built against the
`waterui-dylib` shared runtime (cli#182's dev-build linkage). The
binary records `DT_NEEDED =
libwaterui_dylib-0267d27d87854afc.so` (cargo's hash-suffixed output
name), but `dist/linux/debug/` stages the dylib as the GENERIC
`libwaterui_dylib.so`. The hash-named copy exists in the build tree at
`deps/libwaterui_dylib-0267d27d87854afc.so` — only the staged name is
wrong. `libstd-c64e6e11aa24fc43.so` in the same dist dir IS staged
under its hash name, so the staging code already handles hashed names
for the other shared units.

**Session workaround (not committed):**
`ln -sf libwaterui_dylib.so dist/linux/debug/libwaterui_dylib-0267d27d87854afc.so`
— the file contents are identical; only the name mismatched.

## r27: message context menu (right-click) parity — one real defect, one vocabulary gap, one verified mechanism

Round: right-click a bubble opens Desktop's action set — Reply /
Edit (own only) / Copy text / Pin·Unpin / Forward / Select /
Delete — plus a quick-reaction block on top, anchored at the
pointer and kept inside the window. hydrolysis `eb962313`,
waterui* `0613b49f`, m3 `f15d9196`.

### r27-1: `MenuItem` can't host a horizontal reaction strip — vocabulary gap (waterui)

Desktop opens a horizontal emoji strip *above* the action list.
WaterUI's context-menu vocabulary is `Command | Divider | Menu`
only (`waterui::menu::MenuItem`, resolve at
`components/menu.rs`): a menu item cannot carry an arbitrary
`View`, so a pill row of emoji inside the popup is not
expressible — the closest form is flat `React <emoji>` commands
with `.selected(my_reaction == e)` marking the chosen one, which
is what the app ships (views.rs `message_context_menu_desktop`).
A `MenuItem::Custom(impl View)` (or a `reaction_strip` first-class
item) would be needed for true parity. **Filed upstream:
water-rs/waterui#1245** — an API decision pending with the
maintainer; the flat reaction commands stay until it is decided.

### r27-2: popup stays inside the window at 1400/800/600 — verified, no defect

`WinitWindow::apply_properties` (`src/platform.rs:2588-2597`)
clamps every window's `target_position` to the current monitor's
rect (`x.clamp(monitor.x, monitor.x + monitor.w − window.w)`), so
a pointer near the edge yields an edge-snapped popup instead of
an off-window one. Live-verified with real pointer events on
Xvfb: click (1300,550) at 1400×900 → popup 118×672 at
+1283+228 (= 1400−118 ±1, 900−672 exactly); click (700,530) at
800×900 → +683+228; click (500,545) at 600×900 → +483+228. Both
axes clamp exactly to the window edge. The app's `.context_menu`
needs no positioning code — the framework does it.

### r27-3: context-menu popup window maps and dispatches but never paints — renderer defect

**Finding (framework bug, for upstream filing):** on the winit
renderer a `.context_menu` popup opens as a real
`Window::style(Borderless)` X11 window with the correct bounds,
the full menu a11y tree (14 items incl. Edit on own messages), a
correctly-clamped origin — and renders **zero pixels, forever**.

**Filed upstream: water-rs/hydrolysis#118** — reopened with the
same fill-only evidence from hydroterm; a fix is in progress.

**Repro:** any `.context_menu` on the winit renderer —
```rust
text("right-click me")
    .context_menu(("Copy".action(|_| ()),
                   "Delete".action(|_| ())))
```
then a real right-click on the item.

**Observed:** the popup window maps at the clamped origin
(`xwininfo`: `118x672+1283+228`), the a11y/semantic tree carries
all 14 menu items, and **commands dispatch** — clicking into the
invisible window hit "Edit" and the app's composer switched to
editing with the message text prefilled (`r27_after_react.png`).
But `import -window <popup>` reads only the pixels *behind* the
window: `r27_popup_new.png` vs `r27_popup_30s.png` are
pixel-identical chat content; the popup never presents a frame
over ≥30 s.

**Likely cause:** `animated_popup_panel`
(`src/renderer/input/popup_menu.rs:181`) starts the panel at
`opacity 0.0` + scale 0.96 and animates to 0.96/1.0 over 120 ms
via `.on_appear` — on this renderer the appear animation either
never ticks or the window never presents at all. The input/hit
machinery is fully alive; only presentation is dead. This makes
context menus **unusable** on the winit renderer while being
functionally correct underneath.

**Expected:** the panel animates in and paints. No app-side
workaround exists (opacity/scale are inside the framework's
popup panel).

**r32 update — appears FIXED on hydrolysis `f84c538`:** a live
right-click opened a fully-painted popup — all 14 items
visible at the clamped origin (`r32_ctx.png`), and a menu
command dispatched on click (`r32_pin_menu.png` shows the
pin applied). The Mesa-26.2.3 wolfi run also clears the
depth-32 presentation panic path. Recommending #118 for
closure pending upstream confirmation.

### r27-4: `.context_menu` does not open on a long press — no long-press binding (hydrolysis#191 — recognizer works; `context_menu` must open on touch/pen press-and-hold, not a held mouse button)

**Original claim was wrong in scope.** I grepped hydrolysis's own
crate for a `LongPress` recognizer and found none, and reported
"no recognizer in crate." The recognizer lives in
`waterui-backend-core`: `LongPressDetector`
(`backends/core/src/gesture.rs:687-780`) consumes the generic
`GestureInput::{PointerDown, PointerMove, PointerUp,
PointerCancel, Tick}` stream — any pointer kind, not touch-only —
fires on the `Tick` deadline (or on `PointerUp` after
`duration`), and move-cancels at `LONG_PRESS_SLOP`.
hydrolysis's `GestureEngine` is fully wired
(`register_target`, `handle_pointer_down/up`, `handle_tick`,
`next_deadline`, `sync_after_layout`). hydrolysis#189 closed as
not-a-bug on that basis.

**What I actually tried and saw (r29, live on winit @
`5c8e570d` / waterui `7083a27a`):**

- View code: the chat bubble carries
  `.context_menu((…14-element tuple…))` plus, temporarily, a probe
  `.on_long_press_gesture(500, move |store: Store| store.toggle_select(r11))`
  (probe reverted after verification; it was never shipped).
- Input: `xdotool mousemove 500 420 mousedown 1; sleep 1.3;
  mouseup 1` — a 1.3 s held *left* press on the "six distinct
  reactions" bubble.
- Observed: the probe handler **fired** — "1 selected" and the
  selection toolbar appeared (screenshot r29_probe_longpress2).
  `xwininfo -root -children` before and after shows only the main
  window — the `.context_menu` popup did **not** open.
- Earlier inputs I had driven were only `xdotool click 3`
  (secondary click) which does open the menu — I never drove a
  held press before this round.

**Diagnosis.** The recognizer machinery works end-to-end on a
held mouse press on winit: `on_long_press_gesture` fired at the
deadline and dispatched the handler. The remaining gap is narrow
and precise: `.context_menu` opens only on
`PointerButton::Secondary` (hydrolysis
`renderer/input/hit_test.rs:738,796,897`). `PointerKind::Touch`
flows through the same handler, so no long-press — touch or held
pointer — can open a context menu. `LongPressGesture` is
app-bindable per view, but a gesture handler cannot open the
framework-owned `.context_menu` popup: the popup's anchor point
and lifetime are internal to the context-menu path, so the app
cannot wire "long press → open that menu" itself. Filed as
**hydrolysis#191** (r30): touch and pen press-and-hold must open
`.context_menu`, matching every platform's convention; a held
mouse button must not. It follows platform convention, so it is
a bug, not an API decision — a fix is in progress. Until it
lands, touch/pen long-press → context menu is unreachable on
this backend. PARITY context-menu row keeps the caveat.

### r27-5: cli#183 + cli#184 verified fixed @ `314040e1` — workarounds dropped

- **cli#183 (env `RUSTFLAGS` replacing `[build] rustflags`)**: the
  fix landed in cli#185. `water run --backend hydrolysis` was run
  with no `RUSTFLAGS` in the environment; the managed build picked
  up `~/.cargo/config.toml`'s shim `-L`/`-l` flags and linked
  cleanly (the earlier `__isoc23_strtol` failure path). Export
  removed.
- **cli#184 (staged dylib name ≠ `DT_NEEDED`)**: fixed by cli#187.
  `dist/linux/debug/` now stages
  `libwaterui_dylib-<hash>.so` under the name the binary records —
  the two `libwaterui_dylib-*.so` symlinks are deleted and the
  packaged binary launches with no `LD_LIBRARY_PATH` and no
  manual step. `resources/fonts` still lands beside the exe.
- Both workaround blocks in r26-3/r26-4 are retired; entries kept
  for history with their fix references.

### r27-6: tuple `MenuView` with a realistic item count makes `mount`/`settle` effectively hang — n-deep `zip` fold (waterui `0613b49f`)

Every action on the bubble works, so shipping the full Desktop menu
order meant ~14 `MenuView` items per row × ~25 list rows. Measured on
this tree (debug, `mount_offscreen`-class semantic runtime):

| construction | mount+settle | full `keyboard_tab_cycles_chat` |
|---|---|---|
| no `.context_menu` | baseline | 7.8 s |
| 2-item tuple on all ~25 bubbles | baseline | 8.0 s |
| **14-item tuple on ONE bubble** | ~6 s | **99.9 s** |
| **14-item tuple on all ~25 bubbles** | **~60 s** | **>35 min (killed)** |
| same 14 items via `Computed::constant(Vec<MenuItem>)` on all | back to baseline | **9.0 s** |

**Where it lives:** the tuple `MenuView` impl
(`components/foundation/controls/src/menu.rs:502-514`) left-folds
into `items.zip(&next).map(extend_menu_items).computed()` per element —
a 14-element tuple is a **13-deep zip chain**, then
`resolve_menu_items` (:556) adds one more `zip(locale)`. `Vec<T>`
folds identically (:439-449). Each intermediate `.computed()` node
subscribes its whole left spine again, so per-flush re-evaluation cost
grows super-linearly with chain depth — each re-settle re-walks every
registered menu target's chain (`resolve_menu_items_now` →
`Command::resolve` → `semantic_text().resolve` + `label.resolve` +
`Disabled::resolve`, :200-218).

**Escape hatch that already exists in the API:** `impl MenuView for
Computed<Vec<MenuItem>>` (:359-363) passes the computed through
untouched — `bubble.context_menu(Computed::constant(vec![...14
MenuItems]))` collapses the fold to a single node and the suite is
back to ~9 s with the identical menu content. The app uses this; it is
not an app-side patch of framework code, just the non-tuple
construction the API already offers.

**Expected:** a 14-item context menu is ordinary content — `Vec<T>`/
tuple `MenuView` should not cost ~30× more than `Computed::constant`.
**Filed upstream: water-rs/waterui#1244.**

**CLOSED r29 (verified):** waterui#1247 merged at `7083a27a`; tuple,
array and `Vec` menus now evaluate through a single computed. The app
reverted to the 14-element tuple form (the conditional `Edit` item is an
`Option<Command>` member, which `MenuView` supports) and
`keyboard_tab_cycles_chat` passes in **9.51 s** — same as the
`Computed::constant` escape hatch (9.0 s) and the no-menu baseline
(7.8-8.0 s).

## r28: upstream history rewrite — pins remapped to rewritten SHAs

water-rs rewrote every commit that carried an AI-tool author (and every
commit after them); trees are identical, SHAs changed, and the old SHAs
are off every branch. Pin remapping applied this round:

| repo | old pin | rewritten pin |
|---|---|---|
| hydrolysis | `eb962313` | `5c8e570d` |
| hydrolysis-m3 | `f15d9196` | `7ff961b8` |
| waterui* (all crates) | `0613b49f` | `2fb2dee2` |
| water CLI | `314040e1` | `4c71570b` (reinstalled, `water 0.4.3`) |

Earlier waterui/hydrolysis pins inside older entries (`8886bd42`,
`5fd1c37f`, `c8a39201`) map to `02f9febe` / `cd93c88a` / `56b37983`
respectively. nami (`5dc2d92e`) and waterkit (`123b0581`) still equal
their repos' dev heads — unchanged. Every other framework pin moved to
that repo's current dev head (barcode `7ca3c617`, canvas `61348102`,
chart `848a82f6`, image `6dab5605`, map-gpu `b9465627`, math
`a55cf564`, particle `ea6e350a`, video-gpu `fb0d46cd`, lints
`85c9c1d0`, dew `e9ab6a52`, winui `04d2be02`). Non-water-rs pins
(rust-block, rust-xcb, lexoliu/vello) untouched.

**Caveat for readers:** every SHA cited inline in older entries below
refers to the pre-rewrite object — still resolvable in any checkout
that has it, but no longer on `origin/dev`. File:line references and
repros are unaffected (trees are identical).

`cargo check` compiles clean on the new pins; `water run` rebuilds and
launches with no manual step (cli#183/#184 still closed).

## r29: upstream citations + service message rows (pin)

### r29-1: `ScrollController` is write-only — no way to read scroll position back (waterui, noted)

While choosing the r29 parity item the obvious candidate was
Desktop's floating "jump to latest" button: it appears only while
the list is scrolled away from the bottom, shows the unread count,
and tapping it scrolls to the newest message. `ScrollController<T>`
(`components/foundation/layout/src/collections/scroll.rs:14-51`)
exposes exactly `scroll_to(target)`, `target()` and `generation()` —
a one-way request channel. Nothing reports the realized scroll
offset or the at-bottom state back to the app, so the button's
show/hide condition is unimplementable today: an app can *ask* to
scroll but can never observe that the user scrolled up or reached
the bottom. The same gap also blocks "scroll to bottom when the
user is already at bottom, else keep position" on new messages.
Noted for upstream (a `position`/`at_bottom` signal on the
controller, or an `on_scroll` metadata, would cover it); the
service-row pick below was chosen instead because it has no
framework dependency.

### r29-2: service message rows ("X pinned a message" / "joined the group") — implemented

Desktop draws centered grey service lines in the message flow for
pin notifications, member joins, adds and similar actions. Real
TDLib content (`MessagePinMessage`, `MessageChatAddMembers`,
`MessageChatJoinByLink`, `MessageChatJoinByRequest`) already mapped
to preview text but rendered as ordinary bubbles. `MessageRow`
gained `is_service`: `message_row` marks those content kinds and
prefixes the actor (`"You"`/sender name; add-members resolves the
added names from the users cache), the bubble renders a centered
muted caption on a `SurfaceVariant` pill with no tail/avatar/meta/
context-menu, `set_messages` breaks sender runs across service
rows, and multi-select skips them (no checkbox, `toggle_select`
rejects service ids — Desktop doesn't let you select them). The
demo `pin_message` path appends `"You pinned a message"` like a
real notifying `pinChatMessage`, and the seed carries
`"Alice pinned a message"` after m14 (matching the pinned banner,
which also got `pinned_id = 14` so the banner tap jumps) and
`"Bob joined the group"` before Bob's first message. Tests:
`pin_appends_service_row`, `service_row_not_selectable`,
`service_rows_break_runs`.

## r30: service-row spacing + photo thumbnails

### r30-1: `List` clamps every row to the M3 one-line height floor (56 pt) — no opt-out (hydrolysis, noted)

**Bounds evidence.** A service pill asked for ~24 pt (18 pt text
line + 2×3 pt padding) but its list row was laid out ~56 pt tall
— the user measured ≈55 px of empty space above and below the
pill at 1400 px capture scale.

**Root cause, traced through three layers:**

- hydrolysis `src/renderer/render/measurement.rs:888`
  (`measure_list_item_row_height`): row height =
  `(intrinsic + vertical_inset * 2).max(one_line_row_height)` —
  an unconditional `max` against the theme's one-line minimum;
  `:831` applies the same floor to the whole-list intrinsic
  answer.
- The theme numbers are style-blind:
  `theme.list_metrics()` (waterui-backend-core
  `widget.rs:1527`) returns the same values for every `List`
  regardless of how it is used; hydrolysis-m3
  `theme/dimensions.rs:263` sets
  `LIST_ONE_LINE_ROW_HEIGHT = 56.0` with
  `LIST_HORIZONTAL_INSET = 16.0` and
  `LIST_VERTICAL_INSET = 10.0` (:264-265).
- No app-side opt-out exists: waterui `List`'s builder surface
  (`src/component/list/mod.rs`) exposes only editing / on_delete
  / on_move / scroll_controller / selection / multi_selection —
  no style or row-height API; `ListItem` (:797-819) has
  `new`/`deletable`/`section` only. There is no "compact" or
  content-sized row mode on any platform.

**Why this matters beyond service rows:** any List used as a
chat/timeline where rows are *not* M3 list items pays a 56 pt
minimum per row. Our bubble rows clear the floor so it was
invisible until the ~24 pt pill hit it.

**App resolution (not a workaround — a presentation change):**
Desktop draws service events as interstitial lines inside the
message timeline, at ~the gap of a bubble-run break — the same
treatment day headers already get (they live inside a row, not
as list items). `set_messages` now folds each service row into
the following message row's `service_above` lines (a trailing
run lands in the previous row's `service_below`), rendered as
~24 pt pills inside that row's column — so the list sees only
bubble rows and the service lines keep Desktop's rhythm. The
`is_service` row type survives for a degenerate all-service
list. Selection (`toggle_select`), sender-run breaks
(`service_rows_break_runs`), and pin appends all keep working
on the folded model. If upstream later grows a compact row
mode (or a documented way to opt a row out of the one-line
floor) this fold still reads correctly, but the floor is worth
a look on its own for dense-timeline uses of `List`.

### r30-2: photo messages render the actual image — implemented

The demo seed now writes a procedural 640×360 PNG (stored-
deflate zlib, generated in `demo_photo_png`, no codec dep)
into the app data dir and seeds `files[1]`, so m17's
`media_file = 1` resolves through `file_signal` and the
existing `Photo` branch draws the real thumbnail instead of
the file-card fallback — and the viewer overlay shows the
image rather than "Downloading…". The real TDLib path is
unchanged (downloaded photos always resolved this way; only
the demo lacked bytes). Test: `demo_seeds_photo_file`.

### r30-3: folded service lines vanished after the first `set_messages` — app bug, fixed

Verification of r30-1 on the real renderer first showed the
`service_above` pills painting in the semantic-tree probe but
never on GPU — several wrong theories (overlay layer, Vec
children, view structure) were chased before the data was
checked. Root cause was in the fold itself: the first
implementation reassigned `row.service_above` from the
`pending` drain on *every* `set_messages` call, and
`set_messages` re-runs from ~a dozen sites (`open_chat`,
`jump_to_message`, edits, sends, `regroup_messages`). On the
second run no `is_service` rows remain, `pending` is empty,
and every folded line was silently overwritten with `[]` —
the pills were never in the view tree to paint. `service_below`
only survived because it was assigned solely when `pending`
was non-empty at the end. The fold is now idempotent
(prepend/extend guarded on `!pending.is_empty()`); pills paint
on GPU at 1400/800/600. Not a framework defect — recorded here
because earlier captures made it look like one.

## r31: fold reverted — spacing gap blocked on waterui#1249

### r31-1: service events are their own `List` rows again

The r30 fold was reverted per maintainer decision: routing
service events through neighbouring rows' columns worked
around a real framework gap, and the idempotency bug it
needed (r30-3) showed how brittle it was. The gap is filed as
**water-rs/waterui#1249** (per-row insets + a minimum row
height) — an API decision, not in progress. `service_above`/
`service_below` and the `pending` drain are deleted; service
rows render through `service_line` as their own list items
again, which means each one pays the 56 pt one-line floor
(r30-1) until #1249 lands — visible as ~30 px of extra space
above and below the pill at 1400/800/600 (r31chat*.png). No
other spacing change was made.

**Resolved in r33** — waterui#1252 shipped `ListItem::insets`
+ `.list_min_row_height`; service rows now measure 34.06 pt
through the real APIs (r33-1).

### r31-2a: `List` never re-measures a row whose content changed size — hydrolysis defect (minimal reproduction)

**Filed as water-rs/hydrolysis#199** — fix in progress. Keep
`probe_list_row_remeasure` until the fix lands and the pin is
bumped; remove it then.

A `List` row's extent is measured once — the first time the
row enters the visible window — cached in
`VirtualExtentIndex`, and **never re-measured** until the
whole index resets:

- `src/widgets/layout/list.rs:1162-1177` (flush): the cached
  `extent_index.measured(index)` wins unconditionally;
  `measure_list_item_row_height` runs only on a cache miss.
- The index resets only on (a) a `config.contents` watch
  firing — the rows *collection* changed
  (`list.rs:354-360` sets `rows_dirty`, consumed by
  `prepare_rows` at `:492-517`), or (b) section-chrome arrival
  (`:1090-1096`). A row's own content changing intrinsic
  size touches neither path.
- Because the row subview also keeps its stale slot
  (`flush_in_rect` at `:1568-1574` bounds it to
  `list_content_rect`, which vertically *centers* the content
  inside the slot, `:1946-1959`), content that grew is
  clipped top and bottom on the real renderer.

Reproduced live: the photo row (`Alice 📷 photo.jpg`,
media_file seeded → `when(has, Photo.max_width(320).clip(..))`
in `media_slot`) measures **104.8 pt** for content that lays
out ~262 pt — the row was measured while `file_signal`'s
`has` was still false (the `otherwise` file-card branch) or
before the image's intrinsic arrived; `has`/decode landed
afterwards and the stale extent stayed. a11y dump from a
headless mount jumping to the row:

```
#59 Role(ListItem) "Alice 📷 photo.jpg 09:46" bounds=(0, 151.9, 1400, 104.8)
    children at y≈555-582 — avatar "A" (39.9,581.7),
    "📷 photo.jpg" (74,555.9), "09:46" (246,582.6) —
    ~330 pt below the row, i.e. the laid-out content
    overflowing its stale slot
#65 Role(Image) bounds=(-146, 192, 640, 360) — intrinsic
    640×360 centered into a ~340-wide frame
```

On GPU the row shows only the image's top slice — no bottom
rounded corners, no timestamp (r31_final1400.png, photo
visible y≈456-519 inside a ~156 px row).

**Minimal reproduction** (runnable in this repo —
`probe_list_row_remeasure`, lib.rs): a one-row `List` whose
row content is `text("growing row").size(signal)`, signal
20 → 160 after mount:

```
before: Role(ListItem) bounds=(0,0,400,56)   Role(Label) (16,16.28,368,23.44)
after : Role(ListItem) bounds=(0,0,400,56)   Role(Label) (16,10,368,36)
        — row keeps its cached extent (56 pt floor) while
          the label re-measures to 160 pt and is squeezed
          into the stale 36 pt content rect
```

Expected: the list re-measures the row's extent when the row
subview's measured height changes (e.g. compare the freshly
measured extent with `extent_index.measured(index)` each
flush, or invalidate the entry when the row's own signals
fire). Observed: the extent is permanent until the rows
collection changes. No app-side workaround exists that does
not route around the framework — do not want to
over-provision `max_height` on every row — so the fix
belongs upstream. Blocks the r31 photo row until then.

**Outcome on e00b1f0 (r33-2):** the #202 per-frame
re-measure landed and runs, but the photo row still clips —
the decoded image's size never reaches the transient
intrinsic measure. Follow-up gap documented in r33-2.

### r31-2b: two stray 1 px horizontal lines — same defect, not a divider

The two hairlines under Bob's forwarded bubble and under the
poll bubble in r30chat1400.png are **not** a divider, row
separator, or clip edge:

- m3's `draw_list_separator` is a no-op
  (hydrolysis-m3 `src/layout/list.rs:164`), and neither row
  carries `day_header`/`unread_divider` (checked the seed).
- Measured on GPU: each strip sits at exactly one row's
  bottom edge (`row_rect.y1`) — one under the `svc18` row,
  one under `m19`'s row — spanning the chat width, ~1 px
  tall, lavender-grey blends.
- Every row fills its full `row_rect` with the theme surface
  (hydrolysis `list.rs:1278` → m3 `draw_row_background`
  `list.rs:58`, `fill_rect(bounds, surface)`). The ~1 px
  antialiasing seams between adjacent row fills reveal
  whatever paints beneath.
- What paints beneath is the photo row's **overflow**: its
  content (~262-312 pt) is flushed into the stale 104.8 pt
  slot and spills below the row (r31-2a) — row backgrounds
  below cover it except at the seams.

Verified by experiment, no code change needed:

- Scroll the photo row off screen and back: strips redraw at
  the same row boundaries; scroll past the photo entirely:
  zero strips anywhere in the list (scroll_down.png).
- Removing the photo's `.clip(RoundedRectangle)` moved/reshaped
  the strips with the wider unclipped overflow (bisect1_jump.png)
  — clip is innocent; restored.

So the strips are the second symptom of the stale-extent
defect: no overflow → nothing to reveal → no strips. The fix
is the same upstream re-measure as r31-2a — both entries are
covered by **water-rs/hydrolysis#199**; suppressing them
app-side would mean hiding the overflow with another layout
hack — not done per the no-workaround rule. On e00b1f0 the
seam sliver persists (r33-2); still blocked upstream.

### r32-2: per-span `background` is dropped by the text service — spoiler mask reads as plain text

**Finding (framework gap, for upstream filing):**
`TextStyle` carries a per-span `background: Option<Color>`
(waterui `components/foundation/text/src/styled.rs:24`), but
hydrolysis's text service never reads it:
`ResolvedTextStyleSpec`
(`src/renderer/render/text_service.rs:351-358`) projects only
`font`, `foreground`, `italic`, `underline`, `strikethrough`,
and `span_cache_key` (`:411-419`) matches — no `background`
field exists anywhere in the resolve/shape/draw path.

Consequence: a spoiler span styled as "mask" (foreground ==
bubble fill, background == mask colour — the only way to draw
the bar over the glyphs) renders as ordinary text: the
background is discarded and the masked span's foreground is
either invisible (if it matched the fill it wouldn't need a
mask) or, as seeded, plain black-on-bubble — the "hidden"
text was fully readable (`r32_before_1400.png`, `…800`,
`…600`).

Workaround shipped in watergram instead, no app-side masking
hack: the masked variant styles the span `foreground =
bubble fill` so the glyphs merge into the bubble and only an
empty gap remains; revealed variant uses the normal style.
Files: `src/state.rs:761-764` (mask branch),
`src/views.rs:1743-1762` (dual `.visible` zstack +
`on_tap → reveal_spoiler`).

Minimal repro:

```rust
let mut s = StyledStr::empty();
let mut st = TextStyle::default();
st.background = Some(Color::srgb(0, 0, 0));
st.foreground = Some(Color::srgb(0, 0, 0));
s.push("it's a trap", st);
text(s) // renders "it's a trap" — no black bar behind it
```

Expected: the span's background paints behind the glyphs
(parley `StyleProperty::BackgroundBrush`, or a drawn rect
behind the run). Observed: background is silently dropped at
`resolve_text_style` (`text_service.rs:433-443`).

### r32-3: in-row gesture regions sit ~15 px below the painted
content on a pristine virtualized launch

**Finding (framework bug, for upstream filing):** on the live
winit renderer, taps on the spoiler row's tap target only
register ~15 px BELOW where the target's text paints — the
gesture region is offset relative to the row's painted
content inside a `List`.

Live numbers (wolfi Mesa 26.2.3 lavapipe, Xvfb 1400×1000,
pristine launch, list already scrolled to bottom — no prior
input, no scroll):

- m29's masked spoiler text paints at y≈836-852.
- Click (480, 843): `pointer down candidates → pointer_hits=[]`
  — zero gesture hits; nothing happens.
- Click (480, 860): `pointer up handled gesture_changed=true`
  and the action runs — the spoiler revealed (r32_tap860.png
  vs r32_tap843.png).
- Same for the 🔥 chip row: its visual chip (y≈780-845 in the
  same capture) is dead; taps land only ~15 px below paint.

What it is not:

- Not shadowing: temporarily removing the row-level `on_tap`
  left ZERO recognizers at the painted point (843) — nothing
  eats the press; the region simply isn't there.
- Not stale-from-scroll: reproduced on a fresh launch with no
  prior pointer input.
- Not headless-invisible: `mount_offscreen` +
  `query().label(..).tap_at()` resolves real element bounds
  and reveals correctly
  (`spoiler_tap_reveals_offscreen` passes); on a viewport
  tall enough to mount every row without virtualization the
  region and paint agree
  (`spoiler_region_matches_paint_full_list` passes).
- Not the action never running: a temporary pin+reveal
  diagnostic on the same `on_tap` produced the service pin
  row on the 860 tap — the recognizer fired.

Mechanism candidates (unresolved): the ~15 px matches the
sender-name caption height — the region may be registered
against a content rect that includes a part the paint pass
places differently; or a retained-node stale transform only
in the virtualized path. Files worth a look upstream:
gesture region registration `bindings.rs:233-296`, hit-test
geometry built in `hit_test.rs` — and note `render_depth` is
never incremented anywhere, so `top_group_id_at` priority
reduces to registration order (all equal-depth targets).

Minimal repro shape:

```rust
List::for_each(rows, |row| ListItem::new(
    vstack((
        text("Sender").caption().muted(),   // ~14 px line
        text("tap me").on_tap(|_| /* state flip */),
    )),
))
```

in a list tall enough to virtualize, launched fresh, then a
real pointer click on the "tap me" glyphs.

## r33: repin to e00b1f0 + waterui dev — insets/min-row-height, context_menu preview+accessory, TextStyle.background spoilers

Pins: hydrolysis `e00b1f0467` (carries #199 re-measure via
3d49bf2/#202, #201/#1249 row metrics via 9366b1b/#204, #203
theme), waterui* `98cb7f34` (carries #1245 context-menu API,
#1249/#1252 `ListItem::insets` + `.list_min_row_height`),
hydrolysis-m3 `7ea75913`, nami* `6908aac8`, cli `4c71570b`.

### r33-1: waterui#1252 verified — service pills get Desktop spacing through the real APIs

`ListItem::insets(EdgeInsets)` (waterui `list/mod.rs:798`,
`:839`) and `.list_min_row_height(f32)` (`view.rs:1110`)
landed and are adopted:

```rust
ListItem::new(row_view)
    .insets(if is_service {
        EdgeInsets::symmetric(2.0, 0.0)
    } else {
        EdgeInsets::all(0.0)
    })
// List ends: .scroll_controller(&scroller).list_min_row_height(0.0)
```

`.list_min_row_height(0.0)` removes the m3 56 pt one-line
floor (r30-1/r31-1) — rows size to content plus insets.
Bounds-verified live: service rows measure **34.0625 pt**
(the pill content) instead of 56, seams pack flush
(`row.y1 == next.y0` throughout the extent index). Pills
render compact at 1400/800/600 (r33chat*.png). r31-1 is
**resolved** — no fold, no spacers.

Note: `.scroll_controller` is an inherent `List` method and
`.list_min_row_height` returns an env wrapper (`With<V,T>`),
so the controller must be applied first — ordering
constraint of the API, documented for app authors.

### r33-2: hydrolysis#199 outcome — re-measure landed, but the photo row still clips (new adjacent defect)

e00b1f0's `render_list_parts` now calls
`measure_transient_view_intrinsic` on every window row each
frame and writes `set_measured` (hydrolysis
`src/widgets/layout/list.rs:1149+`). The stale-extent
mechanism of r31-2a is gone for content that *re-measures
to a new value* — but the photo row still clips, so the
answer to "is it gone" is **no**.

a11y bounds on e00b1f0 (full list mounted, jumped to the
row):

```
#17 Role(ListItem) bounds=(0, 937.25, 1400, 84.8125)
    └─ Role(Image) bounds=(-162, 967.31, 640, 360)
       — paints at natural 640×360, overflowing ~305 pt
         below the row's extent into the rows beneath
```

Every frame the row re-measures and every frame the
transient intrinsic returns ~84.8 pt: the decoded image's
natural size never reaches `measure_transient_view_intrinsic`
— either `Photo` reports its placeholder/post-decode
intrinsic only through the semantic layer, or the `when(has,
image)` node measures its inactive branch. The paint path
disagrees with the measure path: `Role(Image)` semantic
bounds are 640×360 while the row's measured extent stays at
the pre-image value.

Minimal repro shape: a `List` row whose media slot is
`when(file_ready, Photo::new(path).max_width(320).clip(..))`
where `file_ready` flips true after the row's first measure —
the row extent sticks at the placeholder height while the
image paints at natural size through the row seams.

Consequence: the bubble is still cut mid-image at
1400/800/600 (r33chat*_photo.png), and the overflow sliver
still leaks through the 1 px row-fill seams — the hairline
above the forwarded bubble at 1400 (~336 px wide, the photo
bubble's width) is that sliver, **not** app-drawn chrome
(the app draws no rules there; verified by pixel scan at
600/800/1400 — the line's width tracks the photo bubble,
not the pane). r31-2b stands; both symptoms block on the
same measure-path gap upstream of #202.

### r33-3: context menu adopted — `ContextMenu` preview + accessory (hydrolysis#200 pending)

`waterui#1245` landed; `message_bubble` now uses:

```rust
bubble_view(&store, &row).context_menu(
    ContextMenu::new(bubble_menu_items(&row, pinned))
        .preview(bubble_view(&store, &row))
        .accessory(reaction_strip(&row)),
)
```

- `bubble_menu_items` is the tuple `MenuView`: Reply, Edit
  (own messages only), Copy text, Pin/Unpin, Forward,
  Select, Divider, Delete — Delete via
  `.command().action(..).destructive()` →
  `CommandRole::Destructive`.
- `reaction_strip` (the accessory) is a five-emoji pill;
  each entry's tap applies the reaction and dismisses via
  `Use(dismiss): Use<DismissContextMenu>` —
  `dismiss.dismiss()`. The multi-extractor handler form
  `move |store: Store, Use(dismiss): Use<DismissContextMenu>|`
  works as designed.

Live on Linux: `apply_context_menu` (hydrolysis
`src/runtime/metadata.rs:548`) registers only
`value.items` — preview and accessory are **not presented**
yet (hydrolysis#200, lifted presentation in progress). The
plain popup opens next to the pointer with all commands
(r33_ctx800.png); clicking **Pin message** dispatches —
pinned banner changed to "📌 single reaction on text"
(r33_ctx800_pin.png). The accessory exists in the
constructed `ContextMenu` (test
`context_menu_items_and_roles` asserts
`accessory.is_some()`, 7 commands, 1 divider, last role
Destructive, incoming = 6 commands) but cannot be mounted
headlessly — `DismissContextMenu`'s constructor is private
(`metadata.rs`), so tap+dismiss is construction-verified
only until #200 lands.

### r33-4: spoiler mask now uses `TextStyle.background` — shows unmasked until hydrolysis#207

Per instruction the foreground-equals-bubble-fill masking is
removed. `styled_from_formatted_mask` (state.rs:675) now sets
`st.background(mask)` on spoiler chunks — mask = the bubble
foreground colour (`Foreground` incoming,
`AccentForeground` outgoing). This is exactly how the mask
should be written. Until **hydrolysis#207** (per-span
`background` dropped at `ResolvedTextStyleSpec`,
text_service.rs:351-358) lands, hydrolysis discards the
background and the span renders as ordinary text — spoiler
text is fully readable in r33chat*.png. Tap-to-reveal state
(`revealed_spoilers` + dual `.visible` zstack) and all
spoiler tests are unchanged and pass; the reveal is a
visual no-op on GPU until the background paints.

### r33-5: reply bubbles — sender name moved above the quote

Desktop order is sender name, then the quoted-reply block.
`bubble_view` (views.rs:1641+) now emits the sender caption
before the reply quote (was quote-first). Verified live:
"Alice" sits above "earlier history, not loaded" at
1400/800/600 (r33chat*.png, r33chat600_mid.png).

The hairlines above/below the forwarded "Telegram News /
Bob" bubble: **not app-drawn** — same photo-row overflow
sliver as r31-2b/r33-2 (width matches the photo bubble;
framework mechanism, no app-side suppression per the
no-workaround rule).

## r34: five parity picks — scroll anchor, list keynav, link card, peer taps, emoji autocomplete

Picks (all implemented + tested):
1. Opening a chat with unread lands on the **unread divider** row instead of
   the tail (`scroll_to_open`); an incoming message while the divider is
   pending does **not** yank the viewport, your own send always follows
   (`follows_tail`).
2. Chat-list **arrow-key navigation**: Up/Down/Home/End step the selection
   via the framework's `navigate_list_row` (hydrolysis
   `hit_test.rs:1889`) which writes the row's `selection` slot →
   `on_change` → `select_chat`; Enter activates the focused row. Test
   `keyboard_arrows_chat_list` now asserts focus→ArrowDown→open_chat=2→3,
   ArrowUp→2, End→10, focus row 6 + Enter→open_chat=6 (was a write-only
   probe before).
3. **Link-preview card is tappable** → `open_link` (robius-open → system
   browser; `link_opened` binding records the URL for tests). `link()`'
   label requires `IntoLabel` — a card can't be a link label, so the tap
   calls `robius_open` directly and the binding keeps it observable.
4. **Peer taps → profile card**: sender avatar, sender name and the
   "Forwarded from" badge open the peer's profile (`open_peer`; demo
   synthesizes the card since `client_id==0`). Sender name already sits
   above the reply quote (r33-5).
5. **`:emoji` autocomplete**: a trailing `:token` (≥2 chars, whitespace or
   start before the colon — `emoji_token`) shows a suggestion row above
   the composer (`emoji_suggest`, ~120-entry `EMOJI_SHORTCODES` table);
   tap → `apply_emoji` replaces the token. Matches Desktop's token rules.

### r34-1: OS file drag-and-drop onto a window is inexpressible — two-layer framework gap

Desktop: dragging files onto the chat window opens the send dialog with a
caption field. Two gaps block it end to end:

1. `waterui::drag_drop::DragData` (`src/interaction_support/drag_drop.rs:49`)
   is `Text(Str) | Url(Str)` — there is no File/Path/Bytes payload variant,
   so a `drop_destination` cannot express "a file was dropped".
2. hydrolysis's drop machinery (`renderer/input/hit_test.rs`,
   `ActiveDrag`/`call_drop_action` at :545-619) only routes **in-app**
   drags started by `.draggable(...)`; winit's `WindowEvent::DroppedFile`
   / `HoveredFile` are never translated into any event (zero occurrences in
   `src/`).

Minimal repro: a `.drop_destination(|data: DragData| …)` region can never
see an OS file drop — `DragData` has no variant to carry it, and no event
reaches the runtime. No app-side workaround exists (both layers are
framework-owned).

### r34-2: `ScrollController` is write-only — no offset/edge readback

`ScrollController<T>` exposes `scroll_to(target)`, `target() -> Computed<T>`
and `generation()` — i.e. the app can *command* a scroll but can never
*read* where the viewport actually is. Two consequences:

- "Is the user pinned to the bottom?" (Desktop's precondition for
  auto-scroll on incoming messages + the "↓ N new messages" button) is
  inexpressible — `follows_tail` uses the pending unread divider as the
  anchor proxy instead.
- Desktop's scroll-position memory per chat is also inexpressible.

Minimal repro: `let s = ScrollController::<usize>::new();` — there is no
`offset()`, `at_end()` or `position` API; `target()` returns what was last
*requested*, not where the viewport sits.

### r34-3: custom text-context-menu commands cannot read the selection

`execute_text_context_menu_action` (hydrolysis
`renderer/input/text_editing.rs:731`) dispatches
`TextContextMenuAction::Custom(command)` via
`call_action_discarding_result(&command.action, env)` — the environment
carries no selected-text handle, so a custom "Bold selection"/"Quote" menu
command cannot know which text is selected. Desktop's composer
format-on-selection (B/I/U/S on a context menu) is therefore inexpressible
through `selection_menu`; the format buttons in the composer row work
through our own bindings instead. No app-side fix — the selection handle
is framework-internal.

### r34-4: ListRow keyboard navigation is unreachable via pointer input

`navigate_list_row` (hydrolysis `src/renderer/input/hit_test.rs:1889`)
moves the chat-list selection with ArrowDown/Up, but only when the node
holding keyboard focus is the row's `AccessibilityActionTarget::ListRow`
node. Rows register focus links for `row_interaction_base` and `+3` only
(list.rs:971-977). A pointer click resolves the *innermost* actionable
node — the `.on_tap` press slot inside our row content — and
`set_keyboard_focus` (:844) lands there, not on the ListRow. From that
focus position ArrowDown/Up are dead keys, and the rows are not
Tab-reachable either, so a pointer-first or keyboard-only user can never
reach arrow navigation. Live-verified on winit/X11: clicking a chat row
then pressing Down/End leaves the selection highlight unmoved; driving
the `Focus` accessibility action onto the `LIST_ITEM` node directly (as
the headless test `keyboard_arrows_chat_list` does) makes the same keys
work, which pins the gap to the pointer→focus mapping, not the nav code.

Minimal repro: a `List` of `ListItem` rows whose content carries an
`.on_tap` — click a row with the pointer, then ArrowDown: nothing moves.
No app-side workaround: the app cannot re-target keyboard focus onto the
ListRow node from view code.

Observed collateral while capturing r34: the oscillating #210 photo-row
extent makes live taps in its overflow region land on the wrong row
(the media viewer opens on clicks painted over the forward badge /
avatar below the photo). Presumably clears with the same fix.

### r34-5: `.on_tap` + naming metadata duplicates a11y nodes on leaf views

`text(name).on_tap(..).a11y_label(..).a11y_role(Button)` emits TWO
identical `Button` nodes at the same bounds: the tap gesture emits one
(`apply_gesture_observer`, metadata.rs:332-358) and the text leaf emits
another, because the gesture's content walk does not strip naming
metadata from the child environment — the container path does
(`accessibility_container_child_environment`,
accessibility_impl.rs:635-651 removes label/role/… before descending),
the gesture path (`render_gesture_content`, metadata.rs:411-428)
suppresses only when `AccessibilityChildren::excludes_descendants`.
"Naming metadata is nearest-consumer" (:44) is thus violated for
gesture-wrapped leaves. Found via `a11y_audit_chat` on the r34
sender-name tap ("Open profile of Alice" twice per row).

Minimal repro: `text("Hi").on_tap(|_| {}).a11y_label("Go").a11y_role(AccessibilityRole::Button)`
in an `OffscreenApp` → two `Role(Button)` `Go` nodes at the same bounds.

An `.a11y_hidden(true)` on the leaf silences it (used here), and a
container between metadata and leaf (e.g. an `if`/`else` AnyView branch)
routes naming through the container-claim path — but that container node
registers with NO `Activate` action
(`begin_accessibility_container_inner` passes `None`,
accessibility_impl.rs:1231-1233), so the scope-claim suppresses the
gesture's actionable node and the announced Button is dead for assistive
activation. Both are framework-side semantics gaps; the leaf-hidden form
is the only composition that yields one Button node carrying `Activate`.

## r35 — repin hydrolysis 1076084 / waterui b956632 (verifications + two new defects)

Re-verified on the new pins (Mesa 26.2.3 lavapipe, wolfi container):

- **#207 (via hydrolysis#212) VERIFIED**: spoiler spans now paint their
  `TextStyle.background` mask — the trailing spoiler text renders as an
  opaque block before reveal at 1400/800/600 (r35_chat1400). No app change.
- **#208 (via hydrolysis#214) VERIFIED**: in-row gesture regions align
  with painted content after virtualization; taps on the painted bounds
  of spoiler/fwd-badge hit the right target (r32-3 closed).
- **#210 (via hydrolysis#216) VERIFIED**: row extents re-measure — at
  1400/800/600 message rows carry their full content; the 1px hairlines
  at row seams and the clipped first bubble are gone. `probe_list_row_
  remeasure` still guards the regression until removed.
- **#211 (via hydrolysis#213) VERIFIED**: winit synthetic focus-replay
  keystrokes no longer double-fire; composer typing via xdotool
  windowfocus+type lands exactly once.

### r35-1: inserting a `when(…)` → `VStack::for_each(SignalCollection, …)`
subtree blanks every color-emoji glyph in the window permanently

Observed live on hydrolysis 1076084 + waterui b956632, wolfi Mesa 26.2.3
lavapipe (also on a4b89a5 — first seen in r34_emoji_600):
- Fresh launch: chips 👍❤️🔥🎉👀🚀 and emoji-only messages paint fine.
- Type `@a` in the composer → the mention suggestion popup (a `when` over
  a `SignalCollection::for_each`) appears; from that frame every emoji in
  the window is blank forever — chips show bare counts, emoji-only
  messages leave empty space, and the popup's own `text(s.emoji)` glyphs
  never appear. Non-emoji text keeps painting. Identical for the `:smi`
  emoji-suggestion popup.
- Control: the sticker/emoji picker — also a `when` subtree but a plain
  `vstack`/`HStack::for_each` grid — opens with every emoji painting and
  does NOT blank anything. Removing the popup's `.clip(RoundedRectangle)`
  changed nothing (rebuilt + retested).
- Broader trigger confirmed in this session: blanking also struck when
  the in-chat search bar (a `when(search_open)` insertion, no for_each)
  appeared, and on plain `field` typing with no popup — so the common
  factor is a *subtree insertion or text-layout mutation* after the
  first emoji pass, not the for_each path specifically. The same event
  also wiped a decoded photo texture in one frame (Alice's photo bubble
  rendered its rounded shell with no image), so the invalidation is not
  emoji-specific either.
- In one launch emoji were blank from the very first frame (flaky —
  never painted at all), so the corruption may also occur without the
  trigger.
- This defect fully explains the three r34_emoji_600 anomalies asked
  about: chips showing bare counts (emoji span blanked), the 09:54
  bubble rendering only its reaction row (its text was an emoji-only
  message whose glyphs vanished), and the `:smi` list showing no glyph
  next to each shortcode (`text(s.emoji)` blanked by its own popup's
  insertion). The "empty band" under the search field is unrelated — it
  is the result-row `for_each` region when the query has no hits yet.

Minimal repro (app-level): a `List` of rows containing `text("👍 …")`
plus a `when(popup, || VStack::for_each(sig_rows, |r| text(r)))` —
assert popup opens, then re-shoot: all emoji gone.

Pointers: glyph scenes are cached per layout
(`glyph_scene_with`, renderer/render/text_service.rs:245-279) and encode
via `scene.draw_glyphs(font).brush(..)` (measurement.rs:490-527); color
emoji ride glifo's bitmap atlas with deferred `PendingBitmapUpload`s and
`pending_clear_rects` drained after eviction (`maintain`,
vello-glifo atlas/cache.rs:74-135) — an insert-time ordering bug there
(clear landing after the upload, or the uploaded bitmap never reaching
the captured subtree) is the likely site, but the exact line is
upstream's to pin down.

### r35-2: `.background(Surface)` does not paint on `when(…) → VStack::for_each(…)`

Both suggestion popups carry `VStack::for_each(rows, …)` +
`.spacing(0).padding_with((4,0)).background(Surface).clip(Rounded…)`.
The rows paint; the background never does — the popup is bare text over
the chat (pixel-verified: popup region #141218 == chat bg; the composer's
static `hstack().background(Surface)` paints #36343B in the same frame).
Removing `.clip` doesn't help; the sticker picker's per-cell
`background(shape.fill())` form DOES paint, so the failure is specific to
the `when`→for_each→`.background(color)` path (likely the background
measuring/painting against the collection's bounds). Minimal repro:
`when(c, || VStack::for_each(rows, |r| text(r)).background(Surface))` on
a colored background → expect a Surface rect, observe none.

### r35-3: retained `List`/`for_each` rows never rebuild when a same-`id` item's fields change

`SignalCollection` emits a new snapshot with updated row fields;
`nami::Binding::set` notifies unconditionally (nami binding.rs:1072) and
`SignalCollection::watch` forwards the whole `Rc<[T]>` (nami
collection.rs:328+) — so the update DOES reach the view layer. It is
dropped there:

- `VisibleSubviewCache::entry` keys subviews by `CollectionItemId` only
  and `or_insert_with` builds each row view exactly once
  (hydrolysis 1076084 renderer/tree/nodes.rs:428-435). `List`'s flush
  calls `cache.entry(row_id, || content)` (widgets/layout/list.rs:1625)
  — the freshly-materialized view for an already-cached id is discarded.
- `prepare_rows` (list.rs:493) resets `extent_index`/`sections` on
  `rows_dirty` but never evicts `item_cache`, so even a `set` that only
  changes fields leaves stale subviews.
- Non-virtualized `CollectionNode::reconcile` (renderer/tree/
  collection.rs:599-609) reuses the `previous` node for live ids without
  calling `get_view`, so the same staleness hits plain
  `VStack::for_each` collections.

Effect: every row-field update is invisible while the row stays mounted
— unread badges, typing indicators, reaction-chip counts, edits,
spoiler reveal, search-highlight spans, the `@` mention badge. Only
signal-driven content inside the row repaints.

Live evidence (this session, hydrolysis 1076084):
- `mention_jump` clears `unread_mentions` in state (unit-tested). The
  `when(has_mentions)`-driven floating `@` button hid immediately, but
  the sidebar row's `@` badge kept painting across many frames
  (r35_mention_after800.png — "Rust China" still shows "@12").
- Sidebar search: the `zip`/`map` filter narrows the list correctly
  ("rus" → 2 rows) and `highlight_styled` computes mark spans
  (unit-tested), yet survivors' titles never repaint — no mark on
  "Rus" even after a resize (r35_side1400.png). Sidebar rows are
  fixed-width so resize does not re-materialize them; chat rows are
  variable-width, which is why in-chat marks DID appear after a resize
  (r35_search800.png) — same defect, opposite accident.
- Consequence: double-tap quick-react and reaction toggles are
  unverifiable live — chips don't repaint until the row is evicted.

`probe_list_row_content_update` (src/lib.rs) is the minimal repro: a
50-row `List::for_each(SignalCollection)` in a 200px viewport, flip a
same-id row's label — it PASSES headless because `mount_offscreen`
materializes rows fresh each settle; the defect is retained-path-only.
Kept enabled as a semantic guard.

To fix this properly, rows also need structural equality so a future
replace-detection pass can tell same-id updates apart: this round adds
`PartialEq` for `ChatRow`/`MessageRow`/`ReactionChip`/`PollOptRow`/
`PollRow` (styled fields compare via `to_plain()` + chunk count —
`styled_row_eq`, src/state.rs:139-169).

### r35-4: `SignalExt::debounce` never re-emits on hydrolysis

`store.chat_search.debounce(400ms)` feeding
`.on_change(.., run_chat_search)` produced nothing live — no n/N
counter, no marks, no result rows — while the identical `on_change` on
the raw `Binding` works immediately (r35_search1400.png shows "1/8" +
results). Same story for the sidebar's `store.search.debounce` →
`run_search`, which is why "Search chats" never filtered before.

Verified chain links (all present, so the broken link is inside them):
- `nami::async_signal::Debounce::watch` spawns
  `executor.spawn_local(async { sleep(d).await; watchers.notify(ctx) })`
  (nami 6908aac async_signal/debounce.rs:120-145); `sleep` is
  `nami::utils::sleep` → `async_io::Timer::after` (support/utils.rs:142).
- The winit local executor is installed
  (`winit_runner.rs:549` `try_init_local_executor`), `spawn_local` posts
  `PollLocalTasks` (:190) and `user_event` drains the queue
  (:748-750). Plain `spawn_local` futures do run live (TDLib calls,
  photo decode complete).
- Headless probes cannot isolate it: the test executor parks runnables
  (hydrolysis renderer/tests/mod.rs:136-152), so any timer-backed
  assertion would vacuously fail there.

Suspect: `async_io::Timer`'s wake → `runnable.schedule()` →
`PollLocalTasks` → `watchers.notify` — possibly the wake never schedules
(waker captured on async-io's reactor thread, proxy event dropped), or
`Debounce`'s own `watchers`/`timer` Rc bookkeeping drops the
subscription. Needs a real event loop to bisect — reported as a live
defect with the chain mapped.

App impact handled without a workaround: both searches watch the raw
binding (`search_now`/`search_live`) — local filtering of a snapshot is
cheap; debounce was only an optimization. DOGFOOD stands: the API is
broken on this backend.
