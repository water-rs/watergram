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

- **`List::content` children do not appear in the semantic a11y tree.**
  `List::content((row("Name", value), row("Username", value)))` renders fine
  natively, but `#[waterui::test]` queries find **zero** nodes for the row
  labels — the entire settings/new-chat screens had to be rewritten as
  `scroll(vstack(..))` just to be testable. Rows inside `List::content`
  (or `ListItem`s in a `List`) need semantic nodes.
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
