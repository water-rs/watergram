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
than the app's pins.

App side is correct: `Menu::new(label("Menu").icon(icon).icon_only())`
→ `icon_only` → `icon_button_metrics` = `ButtonMetrics(0,0,48,48)`,
`draw_chrome` 40 dp primary circle (hydrolysis-m3 `src/icon_button.rs:88-160`,
`src/lib.rs:480-510`; trigger path hydrolysis `src/widgets/button.rs:276-330,
835-870,955`). After repinning the generated crate by hand, the real
renderer draws the 48×48 circle. Lesson: the managed-backend patch table
must be kept in sync manually until cli#178 lands — kept out of the repo.

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
