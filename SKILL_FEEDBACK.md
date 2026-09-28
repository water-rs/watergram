# SKILL_FEEDBACK.md

Feedback on the WaterUI agent skill (`water-rs/waterui` `dev`,
`.claude/skills/waterui/`) and `water mcp`, collected while building
this app. Each entry: what I tried, what the skill said (or didn't),
what is actually true (framework file:line), and the concrete edit
that would have saved me.

Pinned skill source for this round: waterui `db7a6e7d`, copied to
`.devin/skills/waterui/` (see entry 1).

## Entries

### 1. No CLI command installs the skill
- **Tried:** `water skill` / `water skills` to install the agent skill
  the way a user would.
- **Skill said:** nothing — SKILL.md documents skill *usage*, not
  installation.
- **Actually true:** `water --help` lists create/init/channel/
  backend/run/bench/build/package/clean/doctor/device/devices/gc/
  fetch/preview/inspector/mcp/update/completions — no install path.
  I copied `.claude/skills/waterui/` from the git checkout by hand.
- **Fix:** add `water skill install` (copies `skills/waterui/` into
  `.devin/skills/` or `.claude/skills/`), or document the manual copy
  in SKILL.md's first section.

### 2. `vstack(VStack::for_each(..))` fails the TupleViews bound
- **Tried:** `vstack((a, VStack::for_each(rows, |r| ..)))` inside a
  `when` — E0277 "TupleViews is not satisfied".
- **Skill said:** nothing about where a `for_each` collection may sit;
  the examples show it as a top-level view or inside `scroll(...)`.
- **Actually true:** `VStack::for_each` produces a collection node, not
  a tuple member — it cannot be an element of `vstack((..))`. Valid
  forms: bare `VStack::for_each(..)` as a `when`/closure payload, or
  `scroll(VStack::for_each(..))`.
- **Fix:** one line in `references/reactivity.md` (or components):
  "A `*_::for_each` / `ForEach` collection is itself a View, but not a
  `TupleViews` member — don't nest it inside `vstack((..))` tuples."

### 3. `.state(&x)` must be an ancestor of the handler that reads it
- **Tried:** `view.on_hover_enter(h).state(&hov)` order wrong — wrote
  `.state(&hov).on_hover_enter(h)` first; the `State<Binding<bool>>`
  extractor panicked at runtime (state not in env at handler build).
- **Skill said:** `state` documented as env injection, but not the
  ordering constraint relative to the consuming modifier.
- **Actually true:** environment flows downward — `.state()` must be
  applied *after* (outside) the modifier whose handler extracts
  `State<T>`. Reversed order compiles and panics at first event.
- **Fix:** add to `interaction.md`: "`x.on_tap(h).state(&s)` is
  correct; `x.state(&s).on_tap(h)` panics — `State`/`Environment`
  values are only visible to descendants."

### 4. `Command`/`Menu` builder surface is spread across references
- **Tried:** icon-only ⋮ trigger + menu item states.
- **Skill said:** `components.md` shows `Menu::new(label, items)` but
  not `label("..").icon(mdi::x()).icon_only()`, `Command::subtitle`,
  `.destructive()`, `.role()`, `.disabled(signal)`, `.selected(signal)`,
  `.shortcut(..)`, `.state(&x)`.
- **Actually true:** all exist on `Command`/`Menu`
  (controls/src/menu.rs:240-300); items may be a tuple / Vec /
  Option<Command> of `Command`/`Button`/`Divider`/`Menu`
  (MenuView impls ~:393+). `.context_menu(items)` accepts the same
  MenuView; `DismissContextMenu` is read from the accessory's
  environment (`env.get::<DismissContextMenu>()`), not constructed.
- **Fix:** a complete `Command` builder table in `components.md`
  (one line per modifier with signature).

### 5. `.a11y_label` on a container hides descendants' labels
- **Tried:** labeled the pinned-banner `hstack` with
  `.a11y_label("Pinned messages")` — the inner text node vanished from
  `app.query().label("Pinned message")` (test `pinned_banner_shows`).
- **Skill said:** nothing about label semantics on containers vs
  leaves.
- **Actually true:** a container with `.a11y_label` collapses its
  children in the a11y tree (the label replaces the subtree's name).
- **Fix:** `testing.md`/`troubleshooting.md`: "`.a11y_label` on a
  container masks descendant labels — label leaves, or accept the
  collapse and query the container's label instead."

### 6. `SignalExt::debounce`/`throttle` do not emit on hydrolysis
- **Tried:** `binding.debounce(400ms).on_change(..)` for live search.
- **Skill said:** reactivity.md lists `debounce` as available.
- **Actually true:** on hydrolysis it never re-emits
  (water-rs/hydrolysis#228 — timer wake never schedules).
- **RESOLVED (r37, verified live):** nami dev `78d8fd4f` (fix `1a3711e`,
  `UpstreamGuard` pins the upstream subscription for the watch's
  lifetime) restores emission — `.debounce(400ms)` now feeds the search
  filter as expected. The "broken on hydrolysis" caveat can be dropped
  once pins move past the fix.

### 7. Per-span `Style.background` dropped (fixed on #212)
- **Tried:** `StyledStr` span with `style.background = Some(c)` for
  search marks / spoiler masks — never painted.
- **Skill said:** styling.md documents `background` on `Style`.
- **Actually true:** `ResolvedTextStyleSpec` (hydrolysis
  text_service.rs:351-358) carried no `background` — silently dropped
  until hydrolysis#212 (r33+).
- **Fix:** none needed now that it lands — but a "known gaps per
  backend" table in troubleshooting.md would have saved a debug round.

### 8. `when(…)` popup + `.background(Surface)` never paints (#226)
- **Tried:** surfaced emoji/mention popup via `when(open, ||
  VStack::for_each(..)).background(Surface)`.
- **Skill said:** nothing about conditional-subtree backgrounds.
- **Actually true:** background modifier on a `when`→`for_each`
  subtree never paints (water-rs/hydrolysis#226); a static vstack with
  the same background paints fine.
- **Fix:** troubleshooting.md pattern: "background on a conditionally
  inserted collection subtree — known gap #226".

### 9. Retained `List`/`for_each` rows never rebuild on same-id field
   change (#227)
- **Tried:** mutating `row.field` and `Binding<Vec<Row>>::set` — mounted
  rows never re-render while the same `id` stays in the list.
- **Skill said:** for_each docs imply field changes propagate.
- **Actually true:** `VisibleSubviewCache::entry` (hydrolysis
  nodes.rs:428-435) + `cache.entry(row_id, || content)` (list.rs:1625)
  discard the fresh view; `reconcile` reuses `previous` without
  `get_view` (collection.rs:599-609). Rows update only after
  unmount/remount.
- **Fix:** reactivity.md: "a retained row keyed by `id` does not
  rebuild on field change — update-bound UI inside a row must read
  signals, not item fields (#227)".

### 10. `water mcp` basics undocumented for a headless VM
- **Tried:** `water mcp` against the running app per the skill's
  "Driving the app from an agent" section.
- **Skill said:** snapshot → act/pointer/key/type_text → screenshot →
  wait loop exists, but not: how to point it at a running instance,
  which env/binary it wraps, or that on a VM with no WM the window
  needs `xdotool windowfocus`/`windowsize` and `import -window root`
  for captures instead.
- **Actually true:** `water mcp` is a CLI subcommand serving the
  project app over MCP; usable, but the drive loop assumed a real
  desktop session.
- **Fix:** mcp.md: document headless-VM usage (DISPLAY/Xvfb, xdotool
  sizing/focus, ImageMagick `import -window root` for captures).

### 11. `when` nested inside a `when` payload never materializes live
- **Tried:** `when(a, || … when(b, || view) …)` — the inner `when`'s
  payload never paints on winit, though `b` is true; the same tree
  passes `mount_offscreen` (r36, DOGFOOD r36-2).
- **Skill said:** nothing about `when` inside `when` — condition.md
  only covers the top-level form.
- **Actually true:** nested-in-payload `when` subtrees skip
  materialization on the live renderer (headless inserts fine).
- **Fix:** condition.md: "nesting `when` inside another `when`'s
  payload is unreliable on hydrolysis (r36-2); hoist the inner
  condition out — `when(a && b)` / `.otherwise` / a signal-computed
  payload."

### 12. Only the topmost `.on_tap` group fires — descendant shadows
   ancestor
- **Tried:** a row-level `.on_tap` plus a bubble-level `.on_tap` —
  tapping the bubble never fired the row handler.
- **Skill said:** gestures.md documents `.on_tap` but not the
  topmost-group-wins rule.
- **Actually true:** `recognizers_at` activates only the topmost
  gesture group containing the point — the inner `.on_tap` shadows the
  outer; chained gestures on the SAME element co-fire.
- **Fix:** gestures.md: "each `.on_tap` is its own group; only the
  topmost group under the point activates — put the outer tap on a
  sibling overlay (zstack) if both must fire."

### 13. Scroll/List viewport clip does not clip hit regions
- **Tried:** row straddling the scroll viewport's top edge kept an
  unclipped `.on_tap` bound — taps on the chrome band ABOVE the list
  fired the row (r36-3, `probe_scroll_row_tap_clip`).
- **Skill said:** scroll.md documents paint clipping only.
- **Actually true:** `push_layer_rect` clips paint (list.rs:1137-1140)
  while gesture bounds flush into the unclipped `content_rect` (:1631,
  row_rect :1399) — hit bounds ignore the viewport clip.
- **Fix:** troubleshooting.md: "hits reach outside the scroll viewport
  (r36-3) — don't place tappable chrome flush against a scroll edge."

### 14. `TextField` has no `on_submit` — Enter inserts a newline
- **Tried:** Enter-to-send on the composer `field` — Enter produces
  `\n` (text_editing.rs:1898-1909); no submit hook exists
  (r36-4). `Command::shortcut` exists in waterui but hydrolysis never
  dispatches it (r36-1: `Event` is pointer-only, modifier chords bail
  at hit_test.rs:2148).
- **Skill said:** forms.md covers `field` but no keyboard-submit path.
- **Actually true:** no `on_submit`, no key-event surface at all —
  Enter-to-send / Cmd-chords are inexpressible on winit.
- **Fix:** forms.md: "`field` is multi-line; Enter inserts a newline.
  There is no submit event (r36-4) and no `Command::shortcut`
  dispatch on hydrolysis (r36-1)."

### 15. `button().action()` in a post-mount `when` payload never gets a
   press slot — `Menu`, `.on_tap`, `field` do (r36-5)
- **Tried:** select-bar Copy/Forward/Delete/✕ (`when(sel_active)`) and
  banner ✕'s — all paint, all dead (`pointer_hits=[]` at their painted
  bounds); the same payload's `field`, `.on_tap`, and `Menu::new`
  trigger work; `when(c,a).otherwise(b)` eagerly builds both branches
  so send/mic swap is fine.
- **Skill said:** nothing about `when` insertion and pointer targets.
- **Actually true:** paint + gesture + a11y land on insertion but
  `bind_interaction_target` press slots never bind on winit
  (`probe_when_payload_button_dead` — headless passes, live-only).
- **Fix:** condition.md: "`button`/`icon_button` inside a `when`
  payload materialized after mount never receive clicks on hydrolysis
  (r36-5); prefer `when().otherwise()`, `.on_tap`, or `Menu` until it
  lands."

### 16. `cargo build` output is NOT what runs — copy into `dist/`
- **Tried:** launched `dist/linux/debug/watergram-hydrolysis` after
  `cargo build`; spent a round chasing a "defect" that was a stale
  binary — cargo produces `target/debug/watergram-hydrolysis-<hash>`
  which is never synced to `dist/`.
- **Skill said:** build docs imply `water build`/cargo output lands in
  dist.
- **Fix:** getting-started.md / mcp.md: "after `cargo build`, copy
  `target/debug/<bin>-<hash>` over `dist/linux/debug/<bin>` before
  launching — the run path reads dist/, not target/."

### 17. `on_change` reentrancy: writing the watched signal inside its own
   handler panics — `RefCell already borrowed` (r37-1)
- **Tried:** inside `select_chat` (dispatched from `list_selection`'s
  `on_change`), writing `list_selection` to snap the highlight back —
  whole-app panic at `on_change.rs:84` `handler.borrow_mut()`.
- **Skill said:** reactivity.md documents `on_change` but nothing about
  re-entrancy, deferred writes, or that handlers hold a `RefCell`
  across dispatch.
- **Actually true:** any synchronous write to the observed signal inside
  its own `on_change` handler panics; `spawn_local` to defer one task
  turn is the escape hatch.
- **Fix:** reactivity.md: "`on_change` handlers must not synchronously
  write the signal they observe — defer via `spawn_local`."
- **Lint candidate below** — the pattern is statically detectable.

## Lint candidates

Mistakes I made this round that a lint could have caught, plus false
positives observed. (Patterns from earlier rounds folded in.)

- **`.state(&x)` before the consuming modifier** — `view.state(&s)
  .on_tap(h)` compiles and panics at runtime ("State not in env").
  Correct: `.on_tap(h).state(&s)`. Lint: flag `.state()` call whose
  receiver chain contains a handler modifier (`on_tap`, `on_hover_*`,
  `.action`, `context_menu`) *after* it.
- **Collection inside a tuple stack** — `vstack((a, VStack::for_each(..)))`
  is a hard E0277 far from the cause. Lint: `vstack|hstack|zstack`
  argument list containing a `*::for_each`/`for_each`/`List` call —
  suggest wrapping it as the stack's childless payload or `scroll(..)`.
- **Binding `.get()` vs `.snapshot()`** — `Binding<T>` has no `.get()`
  (compile error today; a nami-level lint `binding_get` could suggest
  `.snapshot()` directly instead of relying on E0599).
- **Unclosed `when` chains forgetting `.state`** — pattern `let h =
  Binding::bool(false); v.on_hover_enter(|State(h):..|..)` missing the
  trailing `.state(&h)` → runtime panic. Lint: a `State<T>` extractor
  in a handler with no matching `.state(` ancestor in the same chain.
- **`#[expect]` preferred over `#[allow]`** — the codebase uses
  `#[allow(if_else_view)]` on fns; per org rule, false-positive
  suppressions should be `#[expect(lint, reason="..")]` so a stale
  bypass becomes a warning when it stops firing. (Not enforced by a
  lint today — a `prefer_expect` lint could be it.)
- **`when` payload `FnOnce` closures** — `when(flag, move || view(t))`
  where `t` is captured gives E0525 ("`FnOnce` … moves the variable
  `t` out") — confusing because `then:` payloads look like ordinary
  closures. Correct: clone inside the body (`move || { let t =
  t.clone(); view(t) }`). Lint: flag a `when`/`.otherwise` payload
  closure that moves out of its environment and suggest the
  clone-in-body form.
- **`when(cond, X)` dead-button trap** — a `button`/`icon_button`
  under a bare `when` paints but never receives input on hydrolysis
  (r36-5) — the *shape* of the conditional is the bug. Lint: flag
  `button(..).action(..)`/`icon_button(..)` inside a `when(` payload
  that has no `.otherwise(` sibling, suggest `.otherwise` or `.on_tap`
  or a `Menu`.
- **`on_change` self-write** — `v.on_change(&b, |..| b.set(x))` panics
  at runtime (r37-1). Lint: an `.on_change(&b, ..)` whose handler body
  (or a fn it calls) synchronously `.set(...)`s `b` — suggest
  `spawn_local` deferral.
- **False positives observed:** none new this round (the four r35
  warnings — `.state` ordering mis-fix, `is_positive()`, two
  unnecessary `anyview()`s — were real findings).

## r38

- **`EdgeInsets` array order** — `EdgeInsets::from([a,b,c,d])` is
  `[top, bottom, leading, trailing]` (waterui `padding.rs:193`), which
  is NOT the CSS `TRBL` order many will assume. Cost me a compile-fix
  cycle while placing two floating buttons at bottom-right; got it
  right only after reading the source.
  - **Fix:** `references/layout.md` (or styling) — one line:
    "`EdgeInsets::from([..])` takes `[top, bottom, leading, trailing]`."
- **`WithOpacity` is not `Color`-aware** — `WithOpacity::new(color, a)`
  requires `T: Resolvable<Resolved = ResolvedColor>`
  (waterui `color/mod.rs:172`); `Color` (the role enum) does not
  implement it, so a translucent scrim must be written
  `WithOpacity::new(Srgb::from_hex("#000000"), 0.45)`, not
  `WithOpacity::new(Color::from(...), ..)`. The error is a trait-bound
  E0277 that points nowhere useful.
  - **Fix:** snippets — add a "scrim/overlay" snippet showing
    `Rectangle.fill(WithOpacity::new(Srgb::from_hex("#000"), 0.45))`
    and note that role colors can't be opacity-wrapped.
- **`toggle` lives where?** — skill docs show switch styling but the
  plain `toggle(label, &Binding<bool>)` signature isn't in any
  reference I could find; had to grep waterui sources.
  - **Fix:** forms/controls reference should list
    `toggle(impl Into<Str>, &Binding<bool>)` explicitly.
- **Lint candidates (new this round):**
  - `needless_anyview` fired correctly — `if flag { icon.anyview() }
    else { spacer().anyview() }` where both arms could be plain —
    caught; a `.foreground(if .. { Color::from(X) } else {
    Color::from(Y) })` ternary needs the same hint (both arms same
    type — skip the Color::from wrapper? no wait, the lint is about
    anyview). No false positives.
  - `collection_item_snapshot` on `sidebar_view` flagged MsgHit field
    reads inside `List::for_each` — legit item-level
    `#[expect(collection_item_snapshot, reason="MsgHit fields are
    immutable row data; a hit updates by list replacement, not in-place
    mutation")]` because MsgHit is rebuilt per hit, not mutated in place.
    Kept as an `#[expect]` per the false-positive rule; arguably the
    lint could special-case "fields consumed only at row build time".

## r39

- **Popup menus are separate winit windows** — `.context_menu(...)` and
  `Menu` button popups on hydrolysis mount via `PopupWindowManager` as
  their own OS window (hydrolysis `winit_runner.rs` `PendingWindow::popup`),
  NOT an in-window overlay. Under Xvfb with no compositor,
  `import -window <main>` returns black where the popup obscures the main
  window — I mistook a correctly-rendering popup for a "solid black box"
  regression and lost a cycle bisecting it. Capture `-window root` (root
  composites all windows) or `import -window <popup-id>` directly
  (`xwininfo -root -tree` lists popup windows by size).
  - **Fix:** if the skill ever documents live-testing popups, note that
    popups are sibling windows; screenshot tooling must composite.
- **Lint candidates (new this round):**
  - `manual_binding_mutation` correctly flagged `binding.set(x.into())`
    twice → `set_from(x)`; `needless_anyview` flagged two tail `.anyview()`
    calls inside a `vstack` tuple. Zero false positives this round.

## r40

- **`when(cond)` rebuilds only on the bool edge — a viewer-style overlay
  driven by a `Binding<Option<Row>>` is static after mount.** I placed
  `when(viewer_open, || viewer_layer(...))` and read `store.viewer
  .snapshot()` inside the layer: opening once worked, but stepping to
  another row (`Some(a) → Some(b)` keeps the condition `true`) never
  rebuilt the layer — the caption/media stayed on the first row and the
  ‹ › buttons looked "dead" (their action ran fine; the view just
  didn't change). Took a full instrumented round to see it. Correct
  form: pull row-driven bits into `Computed`s (`text(store.viewer
  .map(..))`) and rebuild only identity-changing nodes via
  `watch(viewer.map(|v| v.msg_id), ..)` — `Photo::new` takes
  `Computed<Url>` directly (reactive), while `video_player` takes a
  constant `MediaItem` so it needs the rebuild.
  - **Fix:** `when` reference should state explicitly: "rebuilds the
    branch when the condition's boolean value changes; an `Option`'s
    payload changing does NOT rebuild" + one line pointing to `watch`
    for identity-keyed subtrees.
- **`on_key_press` bubbles from the focused node up its ancestor chain
  — a handler on a *sibling* layer never sees keys focused elsewhere.**
  I first attached it to the content column inside a viewer `zstack`;
  after clicking a nav button in the sibling layer, ←/→ went dead
  because the focused button's ancestors don't include the content
  column. Attach to the overlay root.
  - **Fix:** a one-liner in the key-handling docs: "bubbling follows
    the focus chain — put the handler on a common ancestor."
- **`icon_button`/`Frame` trap (from the FAB):** `.size()` returns a
  `Frame` that is not `Clone` — you cannot feed one to another modifier
  chain; and `.padding()` takes no argument — uniform padding is
  `.padding_with((v, h))`. Both cost compile cycles.
- **Lint candidates (new this round):** none — `cargo dylint --all`
  clean on all new code.

## r41

- **`Tabs`/`TabItemLayout` does NOT substitute for a chat-folder chip
  strip — evaluated and rejected.** The folder chips need per-item
  context menus (`Edit folder / Mark all as read / Delete folder`),
  per-item icons + unread badges, and they drive a *filter* over one
  shared list — not pane switching. `Tabs` owns the whole content area
  (each item carries its own pane), has no per-item context-menu hook,
  and no icon/badge slot on the item label. Correct form stayed
  hand-rolled: `scroll_horizontal(HStack::for_each(chips, |chip|
  chip_button))`. Cost: one API read of `waterui` `tabs.rs`.
  - **Fix:** the Tabs reference should state the boundary explicitly:
    "Tabs switches panes; it is not a selectable chip row. For
    filter-style horizontal selectors (mutable labels, badges, context
    menus) compose `scroll_horizontal(HStack::for_each(..))`."
- **`when(cond, builder)` payloads are `Fn` — capturing `store` field
  paths makes the closure `FnOnce` with a confusing E0507/E0525.** A
  `move ||` payload that reads `store.something` *by value* (e.g.
  `store.chats.clone()` inside the body) partially moves `store`; the
  error surfaces at the `when` call far from the capture. Correct form:
  hoist the signal clones into the *outer* enclosing body
  (`let rows = store.chats.clone();`) so the `move` closure only
  consumes locals — or clone inside the body.
  - **Fix:** SKILL.md's `when` section should show the
    hoist-clones-then-move pattern; the raw trait-bound error is the
    worst possible teaching moment.
- **`.foreground(impl Into<Color>)` is constant-only — no signals.**
  Theme/environment modifiers accept signals, view color modifiers do
  not; `store.some_computed.map(to_color)` does not satisfy
  `Into<Color>`. For a conditional tint inside a `for_each` row I had
  to `map` to `(Color, Color)` tuples of already-resolved
  `Color::from(WithOpacity::new(token, a))` — workable, but a
  `foreground_signal`/`foreground(impl IntoComputed<Color>)` variant
  would be the obvious API.
  - **Fix:** document "view color modifiers are static; if you need a
    reactive tint, resolve to `Color` upstream and `map`" or add the
    signal-taking overload.
- **`WithOpacity::new` takes theme tokens directly** —
  `WithOpacity::new(AccentForeground, 0.18)` works, while
  `Srgb::from(Accent)` does not (`Srgb` only converts from
  `(u8,u8,u8)`/`[f32;3]`/tuples). Cost me an E0277.
- **`AccessibilityState::checked` takes `Option<bool>`**, not `bool` —
  `checked(Some(b))`. And `Map` signals are not `IntoText`: rendering a
  mapped string needs `text!("{s}")` interpolation, not
  `text(map)`.
  - **Fix (both):** one-line signature examples in the a11y + text
    references would have saved three compile cycles.
- **Lint candidates (new this round):**
  - **`when` payload `Fn` captures** — same trap as the existing
    candidate but the sharper form: a `move` payload that captures a
    `store` *field path* (not the whole `store`) is `FnOnce`. Lint:
    flag field-path captures in `when`/`otherwise` payloads, suggest
    hoisting `let x = store.x.clone()` one level up.
  - **`.visible(chats_visible)` vs `when`** — for section gating I
    deliberately used `.visible` instead of nested `when` to avoid the
    #251 nested-when dead-button family. A lint that flags `when`
    nested inside a `when` payload (on hydrolysis) would catch the
    whole defect class — but it's backend-shaped, so maybe a backend
    audit rather than a lint.
- **`.visible(false)` on a mounted `List` section panics** (DOGFOOD
  r41-1) — the skill doesn't cover when to prefer `when` over
  `.visible`. After the panic I switched the search-section gate to
  `when` (unmount — semantically right anyway). A "hiding vs
  unmounting" note in the conditional-view reference would have
  prevented the crash course.
- **Capture-tooling note (not the framework):** on bare Xvfb (no WM),
  `xdotool key/type` events never reach the app window until
  `xdotool windowfocus <winid>` is run once — `windowactivate` fails
  ("no window manager"). Verified via
  `RUST_LOG='waterui::hydrolysis::input_raw=trace'`: `keyboard_text`
  events only flow after `windowfocus`. Pointer events do not need it.
  If this is common, a line in the hydrolysis testing docs would save a
  debugging session.
- **Cloning a `distinct()`ed signal silently kills all but the first
  watcher** (DOGFOOD r41-2, nami defect) — `Distinct`'s dedup cell is
  shared across clones, so `let a = src.map(..).distinct(); let b =
  a.clone();` leaves one of `a`/`b` permanently dead once the other has
  seen a transition. Cost me most of a debugging session chasing
  "nested `when` never re-fires" — the `when` was fine, its signal was
  dead. Skill guidance needed: **never `.clone()` a signal you derived
  with `distinct()`; derive a second instance from the source
  (`src.map(..).distinct()` again) so each consumer dedups against its
  own history.** Worth a "common signal-combinator traps" section in the
  reactivity reference alongside the `zip`/`map` notes.
  - **Candidate nami fix (for the maintainer):** move the dedup cell
    out of the `Distinct` struct and into `watch` — seed
    `RefCell::new(Some(current_value))` per watcher so dedup is
    per-consumer, matching every other reactive library's `distinct`
    semantics.
- **Lint candidates (new this round):**
  - **`.clone()` on a `distinct()` chain** — if nami keeps the shared
    cell, a lint that flags `Clone` on the result of
    `SignalExt::distinct` (suggest "derive a second instance from the
    source") would catch this at write time. Pattern:
    `let b = distinct_signal.clone()` → warn.

## r42

- **nami#31 fix verified** — `Distinct` dedup is now per-watcher on nami
  dev `ebe55e7`; reverted the per-consumer `distinct` workaround in the
  sidebar search (one shared `searching` instance feeds both `when`
  arms again) and re-verified the second subscriber live
  (r42_searchpane_1400: the results pane — the clone-side watcher —
  renders matches + highlights again). The "signal-combinator traps"
  entry from r41 should be marked *fixed upstream* rather than deleted:
  the guidance stays useful for anyone pinned before ebe55e7.
- **`.state(&x)`-before-handler trap hit again — with `.on_key_press`
  and a full `Store` extract** (entry #3, now confirmed for keyboard
  handlers too). `.state(&store).on_key_press(|_, store: Store| ..)`
  compiles and panics on the *first keystroke*:
  `failed to extract Store … not found at position 0; install the
  value with .state(&value) in the handler's modifier chain or on an
  ancestor` (waterui `core/src/foundation/handler.rs:31`). The panic
  message is now excellent — it names the fix — but the crash is still
  runtime-only and fires on user input, not at mount. The existing
  lint candidate should include `.on_key_press`/`.on_key_release` in
  its modifier list; a compile-time check would have saved a rebuild
  cycle.
- **`Window::new` title takes `impl IntoComputed<Str>`** — a live
  computed title (`store.window_title()`) flows through to X11
  WM_NAME/_NET_WM_NAME and updates when the signal changes. I had to
  read `window.rs` to learn the title accepts a signal rather than a
  static string — worth one line in the window reference.
- **`Command::builder` accepts `text!` output via `IntoLabel`** —
  `Command::builder(text!("{n} reacted with {e}", n=name, e=emoji))`
  compiles (controls `label.rs:268` impls `IntoLabel` for `StyledStr`)
  and was needed to stay lint-clean under `manual_string_signal`.
  The Command/Menu reference shows string literals only.
- **Disabled `Command`s make good info rows in context menus** —
  per-reactor "Alice reacted with 👍" rows are
  `Command::builder(..).action(||{}).disabled(true)`: they render as
  dimmed non-interactive items, which is exactly the Desktop pattern.
  Worth a snippet — menus aren't only actions.
- **Rust 2021 disjoint-field capture, sharper instance** — a `move`
  `when`/`watch` payload that mentions `store.emoji_query.clone()`
  moves the *field* out of `store` (closure turns `FnOnce`). Hoist
  `let eq = store.emoji_query.clone()` to the enclosing scope; the
  `move` closure then captures `eq` whole. (Extends the r41 note —
  same trap, now also inside nested `when` under `when`.)
- **Lint candidates (new this round):**
  - **`manual_string_signal` suggests `s!` — there is no `s!` macro.**
    On `text!`-incompatible `format!`/`String` sites the lint's help
    text reads "use `s!(..)`"; `s!` is not exported by the waterui
    facade at a2e63ddd (checked: no such macro in `core`/macros
    re-exports; `text!`/`rich_text!` are the interpolation macros).
    Recorded `#[expect(manual_string_signal, reason="suggested s!
    macro does not exist; Computed via map() is the signal form")]` on
    `window_title` — false positive by suggestion, the lint itself is
    right that a String literal isn't a signal.
    **Fix for the lint:** point at `text!` interpolation or
    `map().computed()` instead.
  - **`.on_key_press` in the `.state` ordering lint** — see above;
    extend the existing candidate's modifier list.

## r43

- **`NavigationSplitView` compact mode is one column — the reference never
  says so.** navigation.md only says it "adapts to a sliding pane on a
  phone and side-by-side columns on a large window." The actual rule
  (learned from hydrolysis `widgets/nav/navigation.rs`
  `split_measure_plan` / `render_compact_split`): below
  `sidebar_ideal + 360` the split renders **exactly one column** — the
  detail when `primary_selection` is `Some`, the sidebar otherwise — and
  the rendered back control just sets `selection=None`. Consequence an
  app author needs spelled out: at compact widths, pushing a page onto a
  `NavigationStack` that lives in the sidebar is invisible until the user
  presses back. To show a pushed page immediately (e.g. tap a sender →
  Profile while a chat is open at 600px), also set `selection=None`, or
  give the detail column its own `NavigationStack`. Watergram chose the
  former (`Store::show_profile` deselects below 700px — the same
  threshold the framework computes from `sidebar_width.ideal + 360`).
  One paragraph in the "Split views" section would have saved a debug
  loop where the tap looked dead.
- **`App::menu_bar` — the skill says nothing, the runners drop it.** No
  skill file mentions `menu_bar`; the hydrolysis runners destructure and
  discard it (`winit_runner.rs:225`, `web_runner.rs:302`,
  `runner/mod.rs:179`), so app-level menus render nothing and arm no
  shortcuts on hydrolysis (DOGFOOD r43-1). App-level commands still work
  via an always-mounted in-window `Menu` — worth one line in the window
  or components reference until the runner realizes the bar.
- **A mounted `Menu`'s `shortcut` chords arm globally — not just while
  its popup is open.** `MenuShortcutRegistry` collects every mounted
  `Menu`'s commands regardless of popup state
  (`input/menu_shortcuts.rs`), and dispatch precedes text input and the
  plain-modifier early-return (`hit_test.rs:2352-2359`). So a "Quit"
  command in an always-mounted hamburger menu makes Ctrl+W work
  app-wide — this *solved* our global-shortcut gap, and it is the
  correct mental model for "menu shortcuts are window-scoped, always
  armed" rather than "armed while open." The Menu reference should say
  this explicitly.
- **Lint candidates (new this round):** none new — clippy/dylint clean
  on all new code; `Binding::snapshot()` (not `.get()`) was a plain
  compile error, no lint would have caught it.

## r44

- **Typed drag: the skill's drag/drop reference predates #1254.** It still
  describes `DragData`/`drop_destination(DragData)`; on waterui
  eda24225 `DragData` is gone and `.drop_destination` is generic over a
  typed payload: `.drop_destination(|f: waterui::component::drag::Files, store: Store| …)`
  with `f.into_urls() -> Vec<Url>` and `path`/`is_local` per item.
  `.drop_hover(&Binding<bool>)` toggles on hover-in/out. One updated
  snippet pair (drag source `draggable(typed_payload)` + destination)
  would carry the whole mental model.
- **OS file drops: the destination must accept `Files`; winit only emits
  `file://` URIs.** On hydrolysis 437ef045, `WindowEvent::DroppedFile`
  becomes a `Files` payload resolved at `pointer_position`
  (hit_test.rs; platform.rs:2530-2542). For testing: winit 0.30's XDnD
  parser rejects bare paths — the dropper must send
  `text/uri-list` entries carrying the `file://` scheme, and winit
  `Path::canonicalize()`s each URI, so the file must exist. A "how to
  synthesize a drop" note in testing docs would have saved reading
  winit's `parse_data`.
- **`.then`-gated nested `Menu` is a silent trap.** A nested `Menu`
  produced via `flag.then(|| Menu::new(...))` renders, its `›` parent
  opens a submenu popup, and clicks there never dispatch (DOGFOOD
  r44-1). The Menu reference should warn that conditional content at
  nested-menu level is lossy on hydrolysis today — write the `Menu`
  unconditionally and put conditionality inside its items.
- **Popup hit regions can die below the main window's bottom edge.**
  Low-placed `.context_menu` popups lose dispatch on their lower items —
  clicks are swallowed or pass through to the row beneath (DOGFOOD
  r44-2, evidence in shots). Until fixed, any "menu looks fine but won't
  click" report is suspicious: check where the popup's own X window sits
  (`xwininfo -root -children` lists popups) vs the main window bottom.
- **`App::menu_bar` handlers run in the app env, and a `#[state]` type
  extracts `State<T>` — not `T`.** `.action(|store: Store| ...)` on a
  menu_bar command type-checks but panics `extract_or_panic::<Store>` at
  chord dispatch unless the app env holds the `State<Store>` slot. The
  app-level equivalent of `.state(&v)` is
  `env.insert(waterui::extract::State(store.clone()))` — a bare
  `env.insert(store.clone())` writes a different key (`TypeId<Store>`)
  and fools you completely. This also satisfies
  `handler_captures_binding` cleanly (DI instead of closure capture).
  Skill fix: document "app-level state install = `env.insert(State(v))`"
  next to `.state(&v)`. Fixed this round in hydrolysis (devin/menu-bar
  057cc0b): chords arm on `MenuShortcutRegistry` app-wide; only macOS
  renders a real menu bar (NSApp.mainMenu), Linux/Windows/web arm-only —
  worth a "what renders where" table in the menu reference.
- **A git `rev` pin cannot carry an unpushed fix — and fails silently.**
  With `hydrolysis = { git, rev = "68ec29a5" }`, the checkout builds and
  `App::menu_bar` compiles, but the fix living only in a local branch is
  simply absent: chords armed nothing, no warning anywhere. The fix's
  own symptom (dead shortcuts) looked like the original bug. Until the
  branch lands upstream, `path = "../hydrolysis"` is the only correct
  pin; a stale-Cargo.lock check (`cargo metadata --locked`) does NOT
  catch this — the lock happily records the rev as requested.
- **Lint candidates (new this round):**
  - **`when` payload closures must be `Fn`, and a per-call
    `store.X.clone()` inside `.state`-scored closures keeps that
    property** — could a lint flag `FnOnce`-capturing `move` closures
    passed to `when`/`watch`? Second round bitten (r41/r44); the error
    text is clear but the pattern (`.map(...)` inside the moved closure
    body touching `store.field`) is easy to write.
  - none other — `Str::from(&name)` lifetime error surfaced at compile
    time; dylint clean on all new code.
