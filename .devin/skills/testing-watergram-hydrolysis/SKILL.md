---
name: testing-watergram-hydrolysis
description: How to run, drive, and reset the Watergram hydrolysis desktop build for e2e UI testing — launch flags, input model, TDLib update-pump pitfalls, and state reset paths.
---

# Testing Watergram (WaterUI + hydrolysis + TDLib) end-to-end

## Running the app

- Prebuilt binary: `~/.water/build_cache/home/ubuntu/repos/watergram/managed_backends/hydrolysis/dist/linux/debug/watergram-hydrolysis`
- Always launch with `WATER_HYDROLYSIS_FORCE_FALLBACK_ADAPTER=1` (software llvmpipe adapter; no GPU on the test box). Renders slowly — allow extra settle time before screenshots.
- The process name is `watergram-hydrolysis` but `comm` is truncated to 15 chars → `pgrep -f` (not `pgrep -x`) or `pgrep watergram` to find it. Do NOT `pgrep -f watergram-hydrolysis | xargs kill` inside a shell whose own cmdline contains that string — you will kill your own shell.
- TDLib + app stdout go to whatever logfile you redirect; TDLib is extremely chatty (grep for `authorizationState`, `Receive request`, `Sending result`, `Sending error`).

## Input model

- **No AT-SPI bus** (`org.a11y.Bus unavailable` at startup) — the app exposes no accessibility tree. Drive it with coordinate clicks/typing from screenshots; there is no DOM or a11y-tree query path.
- Window = exact content size: hydrolysis sets WM min=max hints from layout, so windows are unresizable and shrink to content (the "Connecting…" screen collapses to ~123x54). Maximize is undone on the next screen change. Keep the window small/expect small windows; zoom screenshots for readability.

## The TDLib update pump (critical pitfall)

- `src/td.rs`: `tdlib_rs::receive()` returns `None` after a ~2s idle timeout. `while let Some(...) = receive()` treats that as end-of-stream → the receive thread exits → the update channel closes → the `.task` drain in `lib.rs` exits → **all TDLib→UI updates permanently stop** (frozen "Connecting…" etc.). Request results (`@extra`) die with it.
- Any auth-flow step that waits >2s for TDLib wedges the app. A defensive patch (`loop { if let Some(u) = receive() { send } }`) makes flows work and is the quickest way to reach downstream screens for testing.
- Errors surface as `auth_note` ("{code}: {message}") only for request failures handled in `state.rs` (submit_phone/submit_code/submit_password). `set_tdlib_parameters` and `request_qr` failures are silent — UI sits on "Connecting…"/"Working…" forever.

## Resetting app state

- Config (api credentials): `~/.config/watergram/config.json` — delete to get the credentials screen back.
- TDLib DB/files: `~/.local/share/watergram/` — delete for a fully clean first-run.
- Env overrides `WATERGRAM_API_ID` / `WATERGRAM_API_HASH` / `WATERGRAM_TEST_DC` prefill/override config (`state.rs Config::load`).

## Credentials & accounts

- No real Telegram credentials exist on the box. Fake-but-plausible creds (numeric api_id + 32-hex api_hash) pass client-side validation and let TDLib connect; the test DC then rejects auth requests with `400: API_ID_INVALID` — a useful way to exercise error-note paths.
- Telegram disabled self-service `99966XYYYY` test numbers in 2024 (see DOGFOOD.md) — a real login needs an account already registered on the test DCs; without one, code/password/register/QR/main screens are unreachable.
- `api_id` validation only rejects `0`/non-numeric — negative values like `-5` pass (then TDLib rejects them). Whitespace-padded numerics pass via `.trim()`.

## Screens

- `Screen` enum in `src/state.rs` drives the root `watch` in `views.rs`: Loading → ApiKeys → Phone → Code/Password/Register/Qr → Main. auth screens share `auth_scaffold` (title + padded vstack).
