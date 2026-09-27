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
