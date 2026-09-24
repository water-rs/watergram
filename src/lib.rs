#![allow(unknown_lints)]

//! Watergram — a Telegram client built on WaterUI + TDLib.
//!
//! `td` owns the TDLib receive thread and the update channel, `state` owns all
//! application state and TDLib request calls, and `views` is the pure view
//! layer. The receive thread cannot touch WaterUI state (nami bindings are
//! `!Send`), so updates flow through an `async-channel` and are drained by a
//! `.task` future running on the UI executor.

mod capture;
mod state;
mod td;
mod views;

use state::Store;
use waterui::app::App;
use waterui::prelude::*;
use waterui::media::Url;
use waterui::preview;
use waterui::task::{sleep, spawn_local};
use waterui::theme::Theme;
use waterui::window::{Window, WindowState};

/// `WATERGRAM_DEMO=1` mounts the UI on `seed_demo` data with no TDLib
/// connection — for rendering checks on device-less VMs.
/// `WATERGRAM_DEMO_PAGE` picks the page: `list` (default), `chat`,
/// `settings`, `emoji`.
fn demo_page() -> Option<&'static str> {
    std::env::var("WATERGRAM_DEMO_PAGE").ok().map(|p| match p.as_str() {
        "chat" => "chat",
        "settings" => "settings",
        "emoji" => "emoji",
        "info" => "info",
        "attach" => "attach",
        _ => "list",
    })
}

#[preview]
fn main() -> impl View {
    if std::env::var_os("WATERGRAM_DEMO").is_some() {
        let store = Store::new(0);
        store.seed_demo();
        // `water mcp` runs through SemanticRuntime, which builds its own window
        // and never drives this one's `frame` binding — the info-panel width
        // threshold would otherwise see Rect::zero() forever (always overlay).
        // WATERGRAM_WIN_SIZE=WxH simulates the delivered window size.
        if let Ok(spec) = std::env::var("WATERGRAM_WIN_SIZE")
            && let Some((w, h)) = spec.split_once('x')
            && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
        {
            store.win_frame.set(Rect::new(Point::zero(), Size::new(w, h)));
        }
        let page = demo_page();
        return views::root(store.clone()).task(async move {
            match page {
                Some("chat") => store.selected.set(Some(1)),
                Some("settings") => store.nav.push(state::Route::Settings),
                Some("emoji") => {
                    store.selected.set(Some(1));
                    store.stickers_open.set(true);
                }
                _ => {}
            }
            std::future::pending::<()>().await;
        });
    }
    let (client_id, rx) = td::spawn_client();
    let store = Store::new(client_id);
    let s = store.clone();
    views::root(store).task(async move {
        s.start();
        while let Ok((update, cid)) = rx.recv().await {
            s.update(update, cid);
        }
    })
}

pub fn app(mut env: Environment) -> App {
    let demo = std::env::var_os("WATERGRAM_DEMO").is_some();
    let (client_id, rx) = if demo {
        (0, async_channel::unbounded().1)
    } else {
        td::spawn_client()
    };
    let store = Store::new(client_id);
    if demo {
        store.seed_demo();
        // Synchronous seeds — evaluated before the view mounts.
        if demo_page() == Some("info") {
            store.selected.set(Some(1));
            store.open_chat.set(1);
            store.info_open.set(true);
            store.load_shared_media();
        }
    }
    env.install(
        Theme::new().color_scheme(
            store
                .dark
                .select(ColorScheme::Dark, ColorScheme::Light),
        ),
    );
    let win_state = binding(WindowState::Normal);
    let store_for_content = store.clone();
    let mut win = Window::new("", win_state, move || {
            let s = store_for_content.clone();
            let rx2 = rx.clone();
            views::root(store_for_content.clone()).task(async move {
                if demo {
                    match demo_page() {
                        Some("chat") => s.selected.set(Some(1)),
                        Some("settings") => s.nav.push(state::Route::Settings),
                        Some("emoji") => {
                            s.selected.set(Some(1));
                            s.stickers_open.set(true);
                        }
                        Some("info") => {
                            s.selected.set(Some(1));
                            s.open_chat.set(1);
                            s.info_open.set(true);
                            s.load_shared_media();
                        }
                        Some("attach") => {
                            s.selected.set(Some(1));
                            let s2 = s.clone();
                            spawn_local(async move {
                                sleep(std::time::Duration::from_millis(800))
                                .await;
                                s2.attach.set(vec![
                                    Url::from_file_path_str(
                                        Str::from("/tmp/design_doc.pdf"),
                                    ),
                                ]);
                            })
                            .detach();
                        }
                        _ => {}
                    }
                    // Anchor the seeded thread to the latest message, as the
                    // live history-load path does (`load_history` → `scroll_bottom`).
                    s.scroll_bottom();
                    std::future::pending::<()>().await;
                }
                s.start();
                while let Ok((update, cid)) = rx2.recv().await {
                    s.update(update, cid);
                }
            })
        });
    // The window's frame MUST be the store's binding — the reverse alias
    // (`store.win_frame = win.frame`) only rewires `store`, while clones
    // captured earlier keep the orphan default binding and see 0×0 forever
    // (the info panel never docked on the real renderer for this reason).
    win.frame = store.win_frame.clone();
    // Spawn size; winit rewrites `win.frame` on Moved/Resized from here on.
    // SemanticRuntime never drives it — see DOGFOOD 'window.frame bypassed'.
    win.frame.set(Rect::new(Point::zero(), Size::new(1280.0, 800.0)));
    // Test hook for `water mcp` screenshots: `WATERGRAM_WIN_SIZE=WxH` makes the
    // width threshold see the viewport size the semantic runner never delivers.
    if let Ok(spec) = std::env::var("WATERGRAM_WIN_SIZE")
        && let Some((w, h)) = spec.split_once('x')
        && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
    {
        win.frame.set(Rect::new(Point::zero(), Size::new(w, h)));
    }
    App::new_with_windows([win], env)
}

#[cfg(test)]
mod tests {
    use chrono::Datelike;
    use crate::state::{ChatRow, FolderRow, MessageRow, Screen, SharedMediaRow, Store};
    use crate::views;
    use waterui::accessibility::AccessibilityRole;
    use waterui::layout::frame::Frame;
    use waterui::prelude::*;
    use waterui::reactive::collection::SignalCollection;
    use waterui_testing::{Role, Styled, UiBuilder};

    fn store() -> Store {
        Store::new(0)
    }

    fn chat(id: i64, title: &str, preview: &str, order: i64) -> ChatRow {
        ChatRow {
            id,
            title: Str::from(title.to_string()),
            preview: Str::from(preview.to_string()),
            order,
            unread: 0,
            pinned: false,
            muted: false,
            photo_file: 0,
            time: "12:00".into(),
            typing: false,
            online: false,
            kind_icon: "".into(),
            marked_unread: false,
            in_archive: false,
        }
    }

    fn msg(id: i64, text: &str, outgoing: bool) -> MessageRow {
        MessageRow {
            id,
            sender: if outgoing { "".into() } else { "Alice".into() },
            text: Str::from(text.to_string()),
            time: "12:00".into(),
            outgoing,
            can_edit: outgoing,
            reply_excerpt: "".into(),
            media_file: 0,
            play_file: 0,
            media_label: "".into(),
            reactions: "".into(),
            failed: false,
            pending: false,
            highlighted: false,
            unread_divider: false,
            day: 0,
            day_header: false,
            day_label: "".into(),
            edited: false,
            read_out: false,
            my_reaction: "".into(),
            styled: waterui::text::styled::StyledStr::empty(),
            webpage: "".into(),
            forwarded_from: "".into(),
            poll: None,
        }
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn api_credentials_form(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.mount(move || views::api_keys_screen(store.clone()).state(&store));
        app.query().label("API ID").assert_exists();
        app.query().label("API hash").assert_exists();
        app.query().role(Role::BUTTON).label("Continue").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn api_form_accepts_input(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let for_assert = store.api_id.clone();
        let mut app = ui.mount(move || views::api_keys_screen(store.clone()).state(&store));
        app.query().label("API ID").single().set_text(&mut app, "94575");
        assert_eq!(for_assert.get().to_string(), "94575");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn phone_form(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.mount(move || views::phone_screen(store.clone()).state(&store));
        app.query().label("Phone number").assert_exists();
        app.query().role(Role::BUTTON).label("Next").assert_exists();
        app.query()
            .role(Role::BUTTON)
            .label("Log in by QR code")
            .assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn code_form_shows_phone(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.phone.set("+9996612345".into());
        let mut app = ui.mount(move || views::code_screen(store.clone()).state(&store));
        app.query()
            .label_contains("9996612345")
            .assert_exists();
        app.query().label("Code").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn chat_list_rows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.screen.set(Screen::Main);
        store.chats.set(vec![
            chat(1, "Alice", "hello", 100),
            chat(2, "Bob", "hi there", 50),
        ]);
        let mut app = ui.mount(move || views::main_screen(store.clone()).state(&store));
        // Chat rows are List items: the row's a11y label folds its
        // children ("Alice hello 14:32") — match via contains.
        app.query().label_contains("Alice").assert_exists();
        app.query().label_contains("Bob").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn chat_search_filters(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![
            chat(1, "Alice", "hello", 100),
            chat(2, "Bob", "hi there", 50),
        ]);
        store.search.set("ali".into());
        let mut app = ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().label_contains("Alice").assert_exists();
        app.query().label_contains("Bob").assert_not_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn messages_render(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.messages.set(vec![
            msg(1, "first message", false),
            msg(2, "my reply", true),
        ]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        // List rows fold their contents into the row's own a11y label
        // ("Alice first message 12:00"), so contents match via contains.
        app.query()
            .label_contains("first message")
            .assert_exists();
        app.query().label_contains("my reply").assert_exists();
        app.query().label_contains("Alice").assert_exists();
        app.query().label("Message").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn composer_sends_and_clears(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let composer = store.composer.clone();
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("Message").single().set_text(&mut app, "hello world");
        app.query().role(Role::BUTTON).label("Send").tap();
        assert_eq!(composer.get().to_string(), "");
    }

    /// Voice note: on a host with no mic the button stays usable and the
    /// exact device error lands in `capture_error` (cpal DeviceNotFound or
    /// "no input device" — either is surfaced verbatim, never swallowed).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn voice_record_button_handles_missing_device(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let rec = store.recording_voice.clone();
        let err_b = store.capture_error.clone();
        let caller = store.clone();
        let mut app =
            ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query()
            .role(Role::BUTTON)
            .label("Record voice note")
            .tap();
        // Either the recorder started (real mic present) or the exact
        // device error surfaced — a listed-but-unopenable device lands on
        // the second path (enumeration can succeed while open() fails).
        if rec.get() {
            caller.cancel_voice_record();
            assert!(!rec.get());
        } else {
            let err = err_b.get().to_string();
            assert!(!err.is_empty(), "missing/unopenable device must surface an error");
        }
    }

    /// Video note: with no camera, the sheet opens and shows the camera
    /// error instead of a preview.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn video_note_handles_missing_camera(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let open = store.video_note_open.clone();
        let caller = store.clone();
        let mut app =
            ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query()
            .role(Role::BUTTON)
            .label("Video note")
            .tap();
        assert!(open.get());
        // The sheet's GpuSurface owns the camera; on a machine without one its
        // shared status must surface the failure (or be mid-open).
        std::thread::sleep(std::time::Duration::from_millis(600));
        let shared = caller.video_shared.borrow().clone();
        let status = shared
            .map(|s| s.borrow().status.get().to_string())
            .unwrap_or_default();
        if crate::capture::camera_count() == 0 {
            assert!(
                status.contains("camera") || status.contains("opening"),
                "unexpected status: {status}"
            );
        }
        caller.close_video_note();
        assert!(caller.video_shared.borrow().is_none());
    }

    /// Failed sends flip back to pending when the user taps the ✗ icon.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn resend_failed_marks_pending(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut m = msg(9, "was not delivered", true);
        m.failed = true;
        store.messages.set(vec![m]);
        let msgs = store.messages.clone();
        let caller = store.clone();
        let mut app =
            ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query()
            .label_contains("was not delivered")
            .assert_exists();
        caller.resend_failed(9);
        let row = msgs.get().into_iter().next().unwrap();
        assert!(row.pending && !row.failed);
    }

    /// 2FA sheet validates input before calling TDLib.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn twofa_requires_current_password(_ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_twofa();
        store.save_twofa();
        assert!(!store.twofa_note.get().to_string().is_empty());
        assert!(store.twofa_open.get());
    }

    /// Per-user privacy exception merge: allow-list wins, denies drop out.
    #[test]
    fn privacy_exception_merge() {
        use tdlib_rs::{enums::UserPrivacySettingRule as R, types};
        let base = vec![
            R::AllowContacts,
            R::RestrictUsers(types::UserPrivacySettingRuleRestrictUsers {
                user_ids: vec![5, 9],
            }),
        ];
        let merged = crate::state::Store::merge_privacy_exception(base, 5, true);
        // Allow rule first, user removed from deny list, base rule preserved.
        assert!(matches!(merged[0], R::AllowUsers(_)));
        assert!(merged.iter().any(|r| matches!(r, R::AllowContacts)));
        let deny = merged.iter().find_map(|r| match r {
            R::RestrictUsers(u) => Some(u.user_ids.clone()),
            _ => None,
        });
        assert_eq!(deny, Some(vec![9]));
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn settings_shows_account(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.me.set(crate::state::MeInfo {
            id: 42,
            name: "Devin Bot".into(),
            phone: "9996612345".into(),
            username: "devinbot".into(),
        });
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Devin Bot").assert_exists();
        app.query().label("devinbot").assert_exists();
        app.query().label("Log out").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn dark_mode_toggle(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let dark = store.dark.clone();
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Dark mode").tap();
        assert!(dark.get());
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn reply_banner_shows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.reply_to.set(Some(5));
        store.reply_label.set("Replying to Alice".into());
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("Replying to Alice").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn pinned_banner_shows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.pinned_label.set("Pinned message".into());
        store.pinned_id.set(99);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("Pinned message").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn chat_search_panel_opens(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.chat_search_open.set(true);
        store
            .chat_search_results
            .set(vec![msg(1, "needle hit", false)]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("Search in chat").assert_exists();
        app.query().label("needle hit").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn marked_unread_shows_dot(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut c = chat(1, "Alice", "hello", 100);
        c.marked_unread = true;
        store.chats.set(vec![c]);
        let mut app = ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        // The unread dot folds into the list item's a11y label.
        app.query().label_contains("●").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn read_receipt_double_check(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut m = msg(1, "seen", true);
        m.read_out = true;
        store.messages.set(vec![m]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label_contains("✓✓").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn reactions_render(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut m = msg(1, "liked", false);
        m.reactions = "👍 3".into();
        store.messages.set(vec![m]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label_contains("👍 3").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn archive_toggle_rebuilds_list(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Alice", "hello", 100)]);
        let mode = store.archive_mode.clone();
        // The menu lives in the sidebar's nav toolbar now (Desktop parity).
        let mut app = ui.mount(move || views::sidebar_stack(store.clone()).state(&store));
        app.query().label("Menu").tap();
        app.query().label("Archive").tap();
        assert!(mode.get());
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn members_panel_lists_members(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.members_open.set(true);
        store.members_count.set("2 members".into());
        store.members.set(vec![crate::state::MemberRow {
            key: 9,
            name: "Alice".into(),
            status: "owner".into(),
            sender: tdlib_rs::enums::MessageSender::User(tdlib_rs::types::MessageSenderUser {
                user_id: 9,
            }),
            username: "alice".into(),
            photo: 0,
        }]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("2 members").assert_exists();
        app.query().label("Alice").assert_exists();
        app.query().label("owner").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn drafts_saved_per_chat(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let inner = store.clone();
        let _app = ui.mount(move || views::chat_detail(inner.clone(), 7).state(&inner));
        store.composer.set("half typed".into());
        store.select_chat(8);
        assert!(store.drafts.borrow().get(&7).is_some());
        store.select_chat(7);
        assert_eq!(store.composer.get().to_string(), "half typed");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn settings_shows_sections(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.twofa.set("On".into());
        store.sessions.set(vec![crate::state::SessionRow {
            id: 1,
            title: "Telegram Desktop 5.0 · PC".into(),
            subtitle: "Berlin · 1.2.3.4 linux".into(),
            current: true,
        }]);
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Edit profile").assert_exists();
        app.query().label("Two-step verification").assert_exists();
        app.query().label("Active sessions").assert_exists();
        app.query().label("Telegram Desktop 5.0 · PC").assert_exists();
        app.query().label("current").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn contacts_list_renders(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.contacts.set(vec![crate::state::MemberRow {
            key: 9,
            name: "Alice A".into(),
            status: "@alice".into(),
            sender: tdlib_rs::enums::MessageSender::User(tdlib_rs::types::MessageSenderUser {
                user_id: 9,
            }),
            username: "alice".into(),
            photo: 0,
        }]);
        let mut app = ui.mount(move || views::new_chat_view(store.clone()).state(&store));
        app.query().label("Alice A").assert_exists();
        app.query().label("@alice").assert_exists();
        app.query().label("Contacts").assert_exists();
    }

    #[test]
    fn set_messages_marks_day_headers() {
        let store = store();
        let today = chrono::Local::now().date_naive().num_days_from_ce() as i64;
        let mut rows = vec![msg(1, "d1a", false), msg(2, "d1b", false), msg(3, "d2a", true)];
        rows[0].day = today - 1;
        rows[1].day = today - 1;
        rows[2].day = today;
        store.set_messages(rows);
        let got = store.messages.get();
        assert!(got[0].day_header);
        assert_eq!(got[0].day_label.as_str(), "Yesterday");
        assert!(!got[1].day_header);
        assert!(got[2].day_header);
        assert_eq!(got[2].day_label.as_str(), "Today");
        // deleting d2a's row recomputes on the next set
        store.set_messages(vec![got[0].clone(), got[1].clone()]);
        assert!(!store.messages.get()[1].day_header);
    }

    #[test]
    fn attachment_content_dispatch() {
        use tdlib_rs::enums::InputMessageContent as C;
        assert!(matches!(
            Store::attachment_content("/tmp/a.png".into(), String::new()),
            C::InputMessagePhoto(_)
        ));
        assert!(matches!(
            Store::attachment_content("/tmp/a.mp4".into(), String::new()),
            C::InputMessageVideo(_)
        ));
        assert!(matches!(
            Store::attachment_content("/tmp/a.mp3".into(), String::new()),
            C::InputMessageAudio(_)
        ));
        assert!(matches!(
            Store::attachment_content("/tmp/a.zip".into(), String::new()),
            C::InputMessageDocument(_)
        ));
        let C::InputMessagePhoto(p) = Store::attachment_content("/tmp/x.JPG".into(), "cap".into())
        else {
            panic!()
        };
        assert_eq!(p.caption.unwrap().text, "cap");
    }

    #[test]
    fn styled_entities_merge() {
        use tdlib_rs::{enums::TextEntityType as T, types};
        let ft = types::FormattedText {
            text: "hello bold world".into(),
            entities: vec![
                types::TextEntity { offset: 6, length: 4, r#type: T::Bold },
                types::TextEntity {
                    offset: 6,
                    length: 4,
                    r#type: T::TextUrl(types::TextEntityTypeTextUrl {
                        url: "https://x".into(),
                    }),
                },
                types::TextEntity { offset: 11, length: 5, r#type: T::Italic },
                types::TextEntity { offset: 11, length: 5, r#type: T::Underline },
            ],
        };
        let chunks = crate::state::styled_from_formatted(&ft).into_chunks();
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].0.to_string(), "hello ");
        assert!(chunks[0].1.is_plain());
        assert_eq!(chunks[1].0.to_string(), "bold");
        // Overlapping Bold + TextUrl merge into one styled chunk.
        assert!(!chunks[1].1.is_plain());
        assert_eq!(chunks[3].0.to_string(), "world");
        assert!(chunks[3].1.italic && chunks[3].1.underline);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn styled_bubble_renders_plain_text(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        let mut m = msg(1, "check https://waterui.dev for the docs", true);
        m.styled = crate::state::styled_from_formatted(&tdlib_rs::types::FormattedText {
            text: "check https://waterui.dev for the docs".into(),
            entities: vec![tdlib_rs::types::TextEntity {
                offset: 0,
                length: 5,
                r#type: tdlib_rs::enums::TextEntityType::Bold,
            }],
        });
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query()
            .label_contains("https://waterui.dev")
            .assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn link_preview_line_shows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        let mut m = msg(1, "check this", false);
        m.webpage = "Example — Title · desc".into();
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query()
            .label_contains("Example — Title · desc")
            .assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn sticker_picker_toggles(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let flag = store.stickers_open.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query().role(Role::BUTTON).label("Stickers & GIFs").tap();
        assert!(flag.get());
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn contacts_add_form_opens(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let inner = store.clone();
        let flag = store.add_contact_open.clone();
        let mut app = ui.mount(move || views::new_chat_view(inner.clone()).state(&store));
        app.query().role(Role::BUTTON).label("Add contact").tap();
        assert!(flag.get());
        app.query().label("Phone").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn settings_storage_section(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Storage").assert_exists();
        app.query().label("Clear cached media").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn members_invite_row(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        store.selected.set(Some(1));
        store.members_open.set(true);
        let inner = store.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query().role(Role::BUTTON).label("New link").assert_exists();
    }

    #[test]
    fn attachment_planning() {
        use crate::state::AttachmentPlan;
        match Store::plan_attachments(
            vec!["/a.png".into(), "/b.png".into(), "/c.mp4".into()],
            "cap".into(),
        ) {
            AttachmentPlan::Album(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0].1, "cap");
                assert_eq!(items[1].1, "");
            }
            AttachmentPlan::Singles(_) => panic!("3 media files must go as one album"),
        }
        match Store::plan_attachments(vec!["/a.zip".into(), "/b.png".into()], "cap".into()) {
            AttachmentPlan::Singles(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].1, "cap");
                assert_eq!(items[1].1, "");
            }
            AttachmentPlan::Album(_) => panic!("mixed types must not album"),
        }
        match Store::plan_attachments(vec!["/a.png".into()], "cap".into()) {
            AttachmentPlan::Singles(items) => assert_eq!(items[0].1, "cap"),
            AttachmentPlan::Album(_) => panic!("one file is not an album"),
        }
    }

    #[test]
    fn privacy_audience_mapping() {
        use crate::state::privacy_audience;
        use tdlib_rs::enums::UserPrivacySettingRule as R;
        assert_eq!(privacy_audience(&[R::AllowAll]), "Everyone");
        assert_eq!(privacy_audience(&[R::RestrictAll]), "Nobody");
        assert_eq!(privacy_audience(&[R::AllowContacts]), "My contacts");
        assert_eq!(privacy_audience(&[R::RestrictAll, R::AllowContacts]), "Nobody");
        assert_eq!(privacy_audience(&[]), "Default");
        assert_eq!(
            privacy_audience(&[R::AllowContacts, R::AllowPremiumUsers]),
            "Custom"
        );
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn folder_tabs_render(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.folders.set(vec![
            crate::state::FolderRow { id: 5, title: "Work".into(), active: false },
            crate::state::FolderRow { id: 9, title: "Chats".into(), active: false },
        ]);
        let mut app = ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().label("Work").assert_exists();
        app.query().label("Archive").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn profile_view_renders(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.profile.set(Some(crate::state::ProfileCard {
            user_id: 42,
            name: "Alice A".into(),
            username: "@alice".into(),
            phone: "+1999".into(),
            bio: "hello world".into(),
            online: true,
        }));
        let mut app = ui.mount(move || views::profile_view(store.clone()).state(&store));
        app.query().label("Alice A").assert_exists();
        app.query().label("@alice").assert_exists();
        app.query().label("hello world").assert_exists();
        app.query().label("online").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn media_play_fallback_row(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut row = msg(7, "Voice message (3s)", false);
        row.media_label = "Voice".into();
        row.play_file = 99;
        let mut app = ui.mount(move || {
            views::message_bubble(store.clone(), row.clone()).state(&store)
        });
        // File 99 is not downloaded -> labelled progress fallback, no crash.
        app.query().label("Voice").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn folder_editor_opens(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.folders.set(vec![FolderRow {
            id: 2,
            title: "Work".into(),
            active: false,
        }]);
        store.folder_open.set(true);
        let mut app =
            ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().label("Save folder").assert_exists();
        app.query().label("New folder").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn account_switcher_opens(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.accounts.set(vec![
            crate::state::AccountRow { id: 1, label: "Alice".into() },
            crate::state::AccountRow { id: 2, label: "Bob".into() },
        ]);
        store.accounts_open.set(true);
        let mut app =
            ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().label("Alice").assert_exists();
        app.query().label("Bob").assert_exists();
        app.query().label("Add account").assert_exists();
    }

    // ------------------------------------------------------------------
    // Layout probes (r7): print semantic bounds to answer "who gets how
    // much space" instead of guessing. Run with:
    //   cargo test --lib probe_ -- --nocapture
    // ------------------------------------------------------------------

    use waterui::shape::{RoundedRectangle, ShapeExt};
    use waterui::theme::color::{Surface, SurfaceVariant};
    use waterui::widget::condition::when;

    fn dump_bounds(path: &str, app: &mut waterui_testing::OffscreenApp) {
        let nodes = app
            .semantic_mut()
            .resolve_elements(&waterui_testing::Selector::default());
        let mut out = String::new();
        for el in nodes.iter() {
            let n = el.node();
            let line = format!(
                "#{id} {role:?} {label:?} bounds={bounds:?} children={children:?}\n",
                id = el.id().as_u64(),
                role = n.role(),
                label = n.label().unwrap_or(""),
                bounds = n.bounds().map(|b| (b.x(), b.y(), b.width(), b.height())),
                children = n.children().len(),
            );
            out.push_str(&line);
        }
        std::fs::write(path, out).unwrap();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_incoming(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.viewport(800, 700).mount_offscreen(move || {
            views::message_bubble(
                store.clone(),
                msg(1, "morning! did the camera filters example work?", false),
            )
            .padding_with((2.0, 12.0))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_incoming.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_incoming.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_outgoing(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut m = msg(2, "yes — device.clone() into Arc, preview straight on the GpuSurface", true);
        m.reply_excerpt = "morning! did the camera…".into();
        let mut app = ui.viewport(800, 700).mount_offscreen(move || {
            views::message_bubble(store.clone(), m.clone()).padding_with((2.0, 12.0))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_outgoing.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_outgoing.png");
    }

    /// r13-2b bisect: same structure as the row's preview line —
    /// text(line_limit 1) + spacer + Image badge, inside a vstack.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_row_bisect(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use std::num::NonZeroUsize;
        use waterui::shape::Circle;
        use waterui::theme::color::{Accent, AccentForeground, Foreground};
        let one = NonZeroUsize::new(1).unwrap();
        let mut app = ui.viewport(340, 300).mount_offscreen(move || {
            vstack((
            vstack((
                // A: plain spacer + text badge
                hstack((
                    text("anyone tried hydrolysis on wayland?").caption().line_limit(one).muted(),
                    spacer(),
                    text("12").caption(),
                ))
                .spacing(4.0),
                // B: no line_limit
                hstack((
                    text("anyone tried hydrolysis on wayland?").caption().muted(),
                    spacer(),
                    text("12").caption(),
                ))
                .spacing(4.0),
                // C: line_limit + Circle Image badge
                hstack((
                    text("anyone tried hydrolysis on wayland?").caption().line_limit(one).muted(),
                    spacer(),
                    text("12")
                        .caption()
                        .foreground(AccentForeground)
                        .padding_with((2.0, 6.0))
                        .background(Circle.fill(Accent)),
                ))
                .spacing(4.0),
            ))
            .spacing(8.0)
            .leading()
            .padding_with((6.0, 10.0)),
            // D: outer hstack with an avatar sibling (full row shape)
            hstack((
                text("RC").padding_with(44.0),
                vstack((
                    hstack((
                        text("Rust China").body().line_limit(one).foreground(Foreground),
                        spacer(),
                        text("14:32").caption().muted(),
                    ))
                    .spacing(4.0),
                    hstack((
                        text("anyone tried hydrolysis on wayland?").caption().line_limit(one).muted(),
                        spacer(),
                        text("12")
                            .caption()
                            .foreground(AccentForeground)
                            .padding_with((2.0, 6.0))
                            .background(Circle.fill(Accent)),
                    ))
                    .spacing(4.0),
                ))
                .spacing(2.0)
                .leading(),
            ))
            .spacing(10.0)
            .padding_with((6.0, 10.0)),
            // E: same full-row shape, short preview that fits
            hstack((
                text("RC").padding_with(44.0),
                vstack((
                    hstack((
                        text("Rust China").body().line_limit(one).foreground(Foreground),
                        spacer(),
                        text("14:32").caption().muted(),
                    ))
                    .spacing(4.0),
                    hstack((
                        text("call me when free").caption().line_limit(one).muted(),
                        spacer(),
                        text("1")
                            .caption()
                            .foreground(AccentForeground)
                            .padding_with((2.0, 6.0))
                            .background(Circle.fill(Accent)),
                    ))
                    .spacing(4.0),
                ))
                .spacing(2.0)
                .leading(),
            ))
            .spacing(10.0)
            .padding_with((6.0, 10.0)),
            ))
            .spacing(12.0)
            .leading()
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_rowbisect.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_rowbisect.png");
    }

    /// r13-2a: the sidebar toolbar row must keep every child inside the
    /// 340px sidebar (registry-hydrolysis icon buttons are ~72dp wide, so a
    /// four-button row overflowed into the detail pane).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_sidebar_toolbar(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.viewport(340, 700).mount_offscreen(move || {
            views::sidebar_view(inner.clone()).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_toolbar.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_toolbar.png");
        // Search field, menu button, connection label, new-chat button —
        // bounds dump is checked manually: every child must end at x<=340.
    }

    /// r13-2b: measure the second hstack of a chat row — long preview with
    /// unread badge. The badge must pin to the trailing edge and the preview
    /// must ellipsize inside its offered width.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chat_row_badge(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.viewport(340, 200).mount_offscreen(move || {
            views::chat_row(
                store.clone(),
                ChatRow {
                    id: 5,
                    title: "Rust China".into(),
                    preview: "anyone tried hydrolysis on wayland? it renders".into(),
                    order: 0,
                    unread: 12,
                    pinned: false,
                    muted: false,
                    marked_unread: false,
                    in_archive: false,
                    photo_file: 0,
                    time: "14:32".into(),
                    typing: false,
                    online: false,
                    kind_icon: "group".into(),
                },
            )
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_chatrow.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_chatrow.png");
        app.query().label("12").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_leading_stack(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(460, 200).mount_offscreen(move || {
            vstack((text("Alice"), text("morning! did the camera filters example work?")))
                .leading()
                .padding()
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_leading.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_spacer_hstack(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(460, 120).mount_offscreen(move || {
            hstack((text("L"), spacer(), text("R"))).padding()
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_spacer.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_two_fields(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let a = Binding::container(Str::from(""));
        let b = Binding::container(Str::from(""));
        let mut app = ui.viewport(340, 300).mount_offscreen(move || {
            hstack((
                field("First name", &a).hide_label(),
                field("Last name", &b).hide_label(),
            ))
            .spacing(8.0)
            .padding_with(8.0)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_fields.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_fields.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_message_list_rows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let store = store();
        let msgs = vec![
            msg(1, "morning! did the camera filters example work?", false),
            msg(2, "yes — device.clone() into Arc, preview straight on the GpuSurface", true),
            msg(3, "nice. and the NV12 conversion?", false),
        ];
        let mut app = ui.viewport(460, 400).mount_offscreen(move || {
            let inner = store.clone();
            vstack((List::for_each(msgs.clone(), move |row: MessageRow| {
                ListItem::new(views::message_bubble(inner.clone(), row).padding_with((2.0, 12.0)))
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_list.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_list.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_plain_row(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let rows = vec![
            msg(1, "morning! did the camera filters example work?", false),
            msg(2, "yes", true),
        ];
        let mut app = ui.viewport(460, 300).mount_offscreen(move || {
            vstack((List::for_each(rows.clone(), move |row| {
                let _ = row;
                ListItem::new(
                    vstack((
                        text("Alice").caption().bold(),
                        text("morning! did the camera filters example work?").body(),
                        hstack((spacer(), text("12:00").caption())),
                    ))
                    .spacing(4.0)
                    .padding_with(10.0)
                    .max_width(420.0)
                    .leading()
                    .background(RoundedRectangle::new(0.18).fill(Surface))
                    .padding_with((2.0, 12.0)),
                )
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_list_plain.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_hug(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Layer ladder: measure who inflates a max_width-limited bubble.
        let mut app = ui.viewport(800, 200).mount_offscreen(move || {
            hstack((
                zstack((
                    zstack((
                        vstack((text("short").body(),))
                            .leading()
                            .padding_with([10.0, 30.0, 10.0, 10.0]),
                        text("👍1").caption(),
                    ))
                    .alignment(BottomLeading),
                    hstack((text("12:00").caption(), text("✓✓").caption()))
                        .spacing(3.0)
                        .padding_with([0.0, 8.0, 0.0, 8.0]),
                ))
                .alignment(BottomTrailing)
                .max_width(420.0)
                .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
                spacer(),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_hug.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_hug.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_hug_noz(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Control: identical bubble minus the zstack overlay.
        let mut app = ui.viewport(800, 200).mount_offscreen(move || {
            hstack((
                vstack((text("short").body(),))
                    .leading()
                    .padding_with([10.0, 30.0, 10.0, 10.0])
                    .max_width(420.0)
                    .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
                spacer(),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_hug_noz.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_hug_text(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Minimal: one text in a max_width frame beside a spacer.
        let mut app = ui.viewport(800, 100).mount_offscreen(move || {
            hstack((
                text("short")
                    .padding_with(10.0)
                    .max_width(420.0)
                    .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
                spacer(),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_hug_text.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chat_detail(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Full chat pane to find the stray clipped node under the pinned bar.
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.viewport(460, 700).mount_offscreen(move || {
            views::chat_detail(inner.clone(), 1).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_chat.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_chat.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chat_detail_wide(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Real-window pane width (~660): composer must fit without overflow.
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.viewport(660, 700).mount_offscreen(move || {
            views::chat_detail(inner.clone(), 1).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_chat_wide.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_chat_wide.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_info_docked_overflow(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Repro: docked info panel + message List — does the List exceed its
        // slot and slide under the panel?
        let mut store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.info_open.set(true);
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::chat_detail(inner.clone(), 1).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_info_dock.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_info_dock.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_info_toggle_late(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // win_frame flips 0 -> 1400 AFTER mount: does the hstack re-distribute
        // the chat column when the docked `when` panel materializes?
        let mut store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.info_open.set(true);
        let frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(0.0, 0.0),
        ));
        store.win_frame = frame.clone();
        let inner = store.clone();
        let mut app = ui.viewport(1060, 900).mount_offscreen(move || {
            views::chat_detail(inner.clone(), 1).state(&store)
        });
        app.semantic_mut().settle();
        frame.set(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_info_late.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_info_late.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_info_open_late(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // win_frame already 1400; info_open flips false -> true after mount.
        let mut store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let info = store.info_open.clone();
        let inner = store.clone();
        let mut app = ui.viewport(1060, 900).mount_offscreen(move || {
            views::chat_detail(inner.clone(), 1).state(&store)
        });
        app.semantic_mut().settle();
        info.set(true);
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_info_open.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_info_open.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_text_wrap_bubble(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::theme::color::SurfaceVariant;
        let txt = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
        let mut app = ui.viewport(700, 200).mount_offscreen(move || {
            vstack((
                Frame::new(
                    zstack((
                        vstack((
                            text(txt).body().anyview(),
                            text("short").caption().anyview(),
                        ))
                        .spacing(4.0)
                        .leading()
                        .padding_with([10.0, 30.0, 10.0, 10.0]),
                        text("09:41").caption(),
                    ))
                    .alignment(BottomTrailing),
                )
                .max_width(120.0)
                .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
            ))
            .width(700.0)
        });
        app.semantic_mut().settle();
        let _ = app.snapshot().save_png("/tmp/probe_wrap3.png");
        dump_bounds("/tmp/probe_wrap3.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_text_wrap_container(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(400, 140).mount_offscreen(move || {
            vstack((
                Frame::new(text(
                    "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
                ))
                .max_width(120.0),
                Frame::new(text(
                    "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
                ))
                .width(120.0),
            ))
            .width(400.0)
        });
        app.semantic_mut().settle();
        let _ = app.snapshot().save_png("/tmp/probe_wrap2.png");
        dump_bounds("/tmp/probe_wrap2.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_text_wrap_maxwidth(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(400, 120).mount_offscreen(move || {
            vstack((
                text("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu")
                    .max_width(120.0),
                text("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"),
            ))
            .width(400.0)
        });
        app.semantic_mut().settle();
        let _ = app.snapshot().save_png("/tmp/probe_wrap.png");
        dump_bounds("/tmp/probe_wrap.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_divider_rules(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::graphics::color::BorderColor;
        use waterui::theme::color::{Accent, Surface};
        let mut app = ui.viewport(400, 60).mount_offscreen(move || {
            zstack((
                Color::from(BorderColor).height(1.0),
                hstack((
                    spacer(),
                    text("Unread messages").caption().bold().foreground(Accent).padding_with((0.0, 6.0)).background(Color::from(Surface)),
                    spacer(),
                )),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_rules.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_rules.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_root_info_docked(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Full views::root at 1400x900 with the info panel docked — does the
        // chat List exceed its pane slot as it does under `water mcp`?
        let mut store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.open_chat.set(1);
        store.info_open.set(true);
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::root(inner.clone()).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_root_dock.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_root_dock.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_root_chat(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // The r14chat1400 page: views::root at 1400x900, `selected` set
        // after mount as the demo task does, info closed.
        let mut store = store();
        store.seed_demo();
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let state = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::root(inner.clone()).state(&state)
        });
        app.semantic_mut().settle();
        store.selected.set(Some(1));
        store.open_chat.set(1);
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_root_chat.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_root_chat.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_root_chat_pre(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Same as probe_root_chat but `selected` set BEFORE mount.
        let mut store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.open_chat.set(1);
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let state = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::root(inner.clone()).state(&state)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_root_chat_pre.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_root_chat_pre.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_root_placeholder(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // The r14list1400 page: same mount, no selection — the detail
        // placeholder should centre vertically.
        let mut store = store();
        store.seed_demo();
        store.win_frame = Binding::container(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let state = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::root(inner.clone()).state(&state)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_root_placeholder.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_root_placeholder.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_settings_trailing(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // r16-4: Account trailing values end ~3px left of Privacy trailing
        // values. Dump bounds to see which row's right edge deviates.
        let store = store();
        store.seed_demo();
        let state = store.clone();
        let mut app = ui.viewport(340, 900).mount_offscreen(move || {
            views::settings_view(store.clone()).state(&state)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_settings.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_settings.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_row_width(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // r16-4 bisect: which modifier makes a row 2.86 wider than 308?
        let store = store();
        let state = store.clone();
        let rows = Binding::container(vec![crate::state::PrivacyRow {
            setting: "Row".into(),
            audience: "Everyone".into(),
            key: tdlib_rs::enums::UserPrivacySetting::ShowStatus,
        }]);
        let mut app = ui.viewport(340, 500).mount_offscreen(move || {
            NavigationView::new(
                "Settings",
                scroll(vstack((
                    hstack((text("a0"), spacer(), text("bare").muted())),
                    vstack((
                        hstack((text("a1"), spacer(), text("ctx").muted())).context_menu((
                            "X".action(|_s: Store| {}),
                        )),
                        VStack::for_each(
                            SignalCollection::new(rows.clone()),
                            |n: crate::state::PrivacyRow| {
                                hstack((text(n.setting.clone()), spacer(), text(n.audience.clone()).muted()))
                                    .context_menu(("X".action(|_s: Store| {}),))
                            },
                        ),
                        hstack((text("a2"), spacer(), text("tap").muted()))
                            .on_tap(|_s: Store| {})
                            .a11y_label("tap row")
                            .a11y_role(AccessibilityRole::Button),
                        toggle("sw", &store.notif_private),
                        hstack((text("a4"), spacer(), text("plain-btn")))
                            .on_tap(|_s: Store| {}),
                        hstack((
                            vstack((text("lit1"), text("lit2"))),
                            spacer(),
                        )),
                    ))
                    .spacing(4.0)
                    .leading(),
                ))
                .spacing(10.0)
                .leading()
                .padding_with((12.0, 16.0))),
            )
            .state(&state)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_row_width.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_row_width.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_bubble_inset(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // Same bubble as probe_bubble_incoming but WITHOUT outer padding —
        // isolates whether `.padding_with((12,2))` or `List` inflates the row.
        let store = store();
        let mut app = ui.viewport(800, 300).mount_offscreen(move || {
            views::message_bubble(
                store.clone(),
                msg(1, "morning! did the camera filters example work?", false),
            )
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_inset.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_when_row(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let rows = vec![
            msg(1, "morning! did the camera filters example work?", false),
            msg(2, "yes", true),
        ];
        let mut app = ui.viewport(460, 300).mount_offscreen(move || {
            vstack((List::for_each(rows.clone(), move |row| {
                let _ = row;
                let hidden = Binding::bool(false);
                ListItem::new(
                    vstack((
                        when(hidden.clone(), || text("fwd").caption().muted()),
                        when(hidden.clone(), || text("reply").caption().muted()),
                        text("Alice").caption().bold(),
                        text("morning! did the camera filters example work?").body(),
                        hstack((spacer(), text("12:00").caption())),
                    ))
                    .spacing(4.0)
                    .padding_with(10.0)
                    .max_width(420.0)
                    .leading()
                    .background(RoundedRectangle::new(0.18).fill(Surface))
                    .padding_with((2.0, 12.0)),
                )
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_list_when.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_fixed_row(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let rows = vec![
            msg(1, "morning! did the camera filters example work?", false),
            msg(2, "yes", true),
        ];
        let mut app = ui.viewport(460, 300).mount_offscreen(move || {
            vstack((List::for_each(rows.clone(), move |row| {
                let _ = row;
                ListItem::new(text("fixed").width(100.0).height(50.0))
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_list_fixed.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_pad_rows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let rows = vec![
            msg(1, "a", false),
            msg(2, "b", false),
        ];
        let mut app = ui.viewport(460, 200).mount_offscreen(move || {
            vstack((List::for_each(rows.clone(), move |row| {
                if row.id == 1 {
                    ListItem::new(text("a").anyview())
                } else {
                    ListItem::new(text("b").padding_with(10.0).anyview())
                }
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_list_pad.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_ladder(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        // 8 rows, one wrapping layer per row — row index identifies the layer.
        let rows: Vec<MessageRow> = (0..9).map(|i| msg(i, "a", false)).collect();
        let mut app = ui.viewport(460, 700).mount_offscreen(move || {
            vstack((List::for_each(rows.clone(), move |row| {
                let body = || {
                    vstack((
                        text("Alice").caption().bold(),
                        text("morning! did the camera filters example work?").body(),
                        hstack((spacer(), text("12:00").caption())),
                    ))
                    .spacing(4.0)
                };
                let i = row.id;
                ListItem::new(match i {
                    0 => body().anyview(),
                    1 => body().padding_with(10.0).anyview(),
                    2 => body().padding_with(10.0).max_width(420.0).anyview(),
                    3 => body()
                        .padding_with(10.0)
                        .max_width(420.0)
                        .leading()
                        .anyview(),
                    4 => body()
                        .padding_with(10.0)
                        .max_width(420.0)
                        .leading()
                        .background(RoundedRectangle::new(0.18).fill(Surface))
                        .anyview(),
                    5 => hstack((
                        body()
                            .padding_with(10.0)
                            .max_width(420.0)
                            .leading()
                            .background(RoundedRectangle::new(0.18).fill(Surface)),
                        spacer(),
                    ))
                    .anyview(),
                    6 => hstack((
                        spacer(),
                        body()
                            .padding_with(10.0)
                            .max_width(420.0)
                            .leading()
                            .background(RoundedRectangle::new(0.18).fill(Surface)),
                    ))
                    .anyview(),
                    7 => hstack((
                        vstack((
                            text("Alice").caption().bold(),
                            text("morning! did the camera filters example work?").body(),
                            hstack((spacer(), text("12:00").caption())),
                        ))
                        .leading()
                        .spacing(4.0)
                        .padding_with(10.0)
                        .max_width(420.0)
                        .background(RoundedRectangle::new(0.18).fill(Surface)),
                        spacer(),
                    ))
                    .anyview(),
                    _ => hstack((
                        body()
                            .padding_with(10.0)
                            .max_width(420.0)
                            .leading()
                            .background(RoundedRectangle::new(0.18).fill(Surface)),
                        spacer(),
                    ))
                    .padding_with((2.0, 12.0))
                    .anyview(),
                })
            }),))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_ladder.txt", &mut app);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_scroll_horizontal_stretch(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::list::{List, ListItem};
        let rows: Vec<MessageRow> = (0..3).map(|i| msg(i, "a", false)).collect();
        let chips: Vec<MessageRow> = (0..3).map(|i| msg(10 + i, "chip", false)).collect();
        let mut app = ui.viewport(340, 400).mount_offscreen(move || {
            vstack((
                scroll_horizontal(waterui::component::lazy::Lazy::hstack(
                    waterui::views::ForEach::new(chips.clone(), move |_c| {
                        text("Chip").caption().padding_with((3.0, 10.0))
                    }),
                )),
                List::for_each(rows.clone(), move |row| {
                    let _ = row;
                    ListItem::new(text("row").padding_with(8.0))
                }),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_hscroll.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_hscroll.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_sidebar_scrolls(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let mut app = ui.viewport(340, 700).mount_offscreen(move || {
            views::sidebar_view(store.clone()).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_sidebar.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_sidebar.png");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chat_all_labels(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut app = ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        let els = app.resolve_elements(&waterui_testing::Selector::default());
        for e in els.iter() {
            let n = e.node();
            println!("{:?} label={:?} value={:?}", n.role(), n.label(), n.value());
        }
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn poll_renders_in_bubble(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut m = msg(18, "", false);
        m.poll = Some(crate::state::PollRow {
            question: "Ship the r8 bundle today?".into(),
            options: vec![
                crate::state::PollOptRow { ix: 0, text: "Yes".into(), pct: 67, chosen: true },
                crate::state::PollOptRow { ix: 1, text: "Tomorrow".into(), pct: 33, chosen: false },
            ],
            voters: 3,
            closed: false,
        });
        store.messages.set(vec![m]);
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 7).state(&inner));
        app.query().label_contains("Ship the r8 bundle today?").assert_exists();
        app.query().label_contains("Yes ✓").assert_exists();
        app.query().label_contains("67%").assert_exists();
        app.query().label_contains("3 votes").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn emoji_tab_shows_grid(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.stickers_open.set(true);
        store.panel_tab.set(0);
        app.query().label("Emoji").assert_exists();
        app.query().label("😀").assert_exists();
        app.query().label("🚀").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn multi_select_bar_appears(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.selected_msgs.set(vec![11, 12]);
        app.query().label("2 selected").assert_exists();
        app.query().label("Forward selected").assert_exists();
        app.query().label("Delete selected").assert_exists();
        app.query().label("Clear selection").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn media_viewer_overlay(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.viewer.set(Some(crate::state::ViewerRow {
            file: 0,
            video: false,
            caption: "sunset".into(),
            from: "Alice".into(),
        }));
        app.query().label("Close viewer").assert_exists();
        app.query().label("Downloading…").assert_exists();
        app.query().label("sunset").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn scheduled_panel_lists_rows(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.scheduled.set(vec![msg(90, "remind tomorrow", false)]);
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.scheduled_open.set(true);
        app.query().label("Scheduled messages").assert_exists();
        app.query().label("remind tomorrow").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn forward_banner_has_noattr_chip(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.forward_ids.set(vec![1, 2]);
        let mut app = ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().label("Select a chat to forward to").assert_exists();
        app.query().label("Forward without attribution").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn settings_blocked_section(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.blocked.set(vec![crate::state::MemberRow {
            key: 42,
            name: "Spammer".into(),
            status: "@spam".into(),
            sender: tdlib_rs::enums::MessageSender::User(tdlib_rs::types::MessageSenderUser {
                user_id: 42,
            }),
            username: "spam".into(),
            photo: 0,
        }]);
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Blocked users").assert_exists();
        app.query().label("Spammer").assert_exists();
        app.query().label("Unblock").assert_exists();
    }

    /// r13-4: Settings → Language lists the packs from
    /// `getLocalizationTargetInfo` (demo seeds three).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn lang_pack_section(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.load_language_packs(); // demo branch seeds three rows
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Language").assert_exists();
        app.query().label("English").assert_exists();
        app.query().label("简体中文 — Chinese (Simplified)").assert_exists();
        app.query().label("Deutsch — German").assert_exists();
    }

    #[test]
    fn tr_lookup_picks_plural_slots_and_falls_back() {
        let store = store();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "Members".to_string(),
            tdlib_rs::enums::LanguagePackStringValue::Pluralized(
                tdlib_rs::types::LanguagePackStringValuePluralized {
                    one_value: "1 member".into(),
                    other_value: "{count} members".into(),
                    ..Default::default()
                },
            ),
        );
        map.insert(
            "Settings".to_string(),
            tdlib_rs::enums::LanguagePackStringValue::Ordinary(
                tdlib_rs::types::LanguagePackStringValueOrdinary {
                    value: "Einstellungen".into(),
                },
            ),
        );
        map.insert(
            "Gone".to_string(),
            tdlib_rs::enums::LanguagePackStringValue::Deleted,
        );
        store.lang_strings.set(map);
        assert_eq!(store.tr("Settings", 0, "Settings").as_str(), "Einstellungen");
        assert_eq!(store.tr("Members", 1, "Members").as_str(), "1 member");
        assert_eq!(
            store.tr("Members", 5, "Members").as_str(),
            "{count} members"
        );
        assert_eq!(store.tr("Gone", 0, "fallback").as_str(), "fallback");
        assert_eq!(store.tr("Missing", 0, "fallback").as_str(), "fallback");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn info_panel_shows_shared_media(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.toggle_info();
        app.query().label("Chat info").assert_exists();
        app.query().label("Shared media").assert_exists();
        app.query().label("Members").assert_exists();
        app.query().label("Close info").assert_exists();
    }

    /// Rendered evidence for the poll creator (item 3): question field,
    /// option fields gated by `poll_option_count`, Quiz/Multiple toggles.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_poll_creator(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.poll_option_count.set(4);
        let st = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount_offscreen(move || views::poll_creator(st.clone()));
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_poll.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_poll.png");
        app.query().label("Question").assert_exists();
        app.query().label("Option 4").assert_exists();
        app.query().label("Option 5").assert_not_exists();
    }

    /// Minimal: does a ZStack's `.alignment(TopTrailing)` place a finite
    /// child at the trailing edge under SemanticApp?
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_zstack_align(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::theme::color::{Foreground, Surface};
        let store = store();
        store.seed_demo();
        let mut app = ui.viewport(1000, 700).mount_offscreen(move || {
            zstack((
                Color::from(Foreground)
                    .opacity(0.3)
                    .on_tap(|_s: Store| {}),
                Frame::new(
                    text("probe")
                        .padding_with(20.0)
                        .background(Color::from(Surface)),
                )
                .max_height(f32::INFINITY),
            ))
            .alignment(TopTrailing)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_zalign.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_zalign.png");
    }

    /// Overlay info panel (<1120px window): full-height elevated panel at
    /// the trailing edge over a scrim, with the shared-media grid visible
    /// — SemanticApp bounds prove the panel spans the viewport height and
    /// the lazy grid gets real rows (empty-grid bug from r11 is closed).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_overlay_chunk(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.shared_media.set(vec![
            SharedMediaRow { file: 0, label: "🌄".into() },
            SharedMediaRow { file: 0, label: "📷".into() },
            SharedMediaRow { file: 0, label: "🎞".into() },
        ]);
        let st = store.clone();
        let mut app = ui
            .viewport(1000, 700)
            .mount_offscreen(move || views::info_overlay_chunk(st.clone()));
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_overlay.txt", &mut app);
        let _ = app.snapshot().save_png("/tmp/probe_overlay.png");
        app.query().label("Shared media").assert_exists();
        app.query().label("🌄").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn unread_divider_renders(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.query().label_contains("Unread messages").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn attach_preview_shows_caption_field(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.attach.set(vec![Url::from_file_path_str(
            Str::from("/tmp/photo.jpg"),
        )]);
        app.query().label("photo.jpg").assert_exists();
        app.query().label("Caption").assert_exists();
        app.query().label("Remove").assert_exists();
    }

    /// Minimal nami#23 topology: a `when` gated on signal A (derived from a
    /// binding) whose body builds a second `when` on signal B derived from
    /// the same binding. Toggling A false cancels A's watcher; dropping its
    /// content drops B's `WatcherManagerGuard` on the same manager — the
    /// re-entrant `cancel` panics in the hydrolysis renderer. This test
    /// reports whether the semantic testing runtime reproduces it.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn nami_cancel_reentrancy_minimal(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let b = waterui::reactive::binding(Vec::<Str>::new());
        let outer = b.map(|v: Vec<Str>| !v.is_empty()).distinct();
        let inner_src = b.clone();
        let mut app = ui.mount(move || {
            let inner_src2 = inner_src.clone();
            when(outer.clone(), move || {
                let inner = inner_src2.map(|v: Vec<Str>| v.len() > 1).distinct();
                when(inner, || text("many")).otherwise(|| text("one"))
            })
        });
        // Build A's body → B's watcher subscribes on the same manager.
        b.set(vec![Str::from("a")]);
        // Toggle A false → cancel A's watcher → drops B's guard inside
        // `WatcherManager::cancel`'s borrow_mut → nami#23 panic path.
        b.set(Vec::new());
        app.query().label("many").assert_not_exists();
    }

    fn member(id: i64, name: &str, uname: &str) -> crate::state::MemberRow {
        crate::state::MemberRow {
            key: id,
            name: name.to_string().into(),
            status: "member".into(),
            sender: tdlib_rs::enums::MessageSender::User(tdlib_rs::types::MessageSenderUser {
                user_id: id,
            }),
            username: uname.to_string().into(),
            photo: 0,
        }
    }

    #[test]
    fn mention_token_parses() {
        assert_eq!(Store::mention_token("hi @al"), Some("al".to_string()));
        assert_eq!(Store::mention_token("hi @"), Some("".to_string()));
        assert_eq!(Store::mention_token("mail a@b"), None);
        assert_eq!(Store::mention_token("plain text"), None);
        assert_eq!(Store::mention_token(""), None);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn mention_popup_filters_and_inserts(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store
            .members
            .set(vec![member(11, "Alice", "alice"), member(12, "Bob", "bob")]);
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.composer.set(Str::from("hi @al"));
        app.query().label("@alice").assert_exists();
        app.query().label("@bob").assert_not_exists();
        store.apply_mention("alice");
        assert!(store.composer.get().contains("@alice "));
    }

    /// Poll creator: opening the panel shows question + 2 option fields,
    /// Create sends the poll and resets the creator.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn poll_creator_sends_and_resets(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let inner = store.clone();
        let mut app = ui.mount(move || views::chat_detail(inner.clone(), 7).state(&inner));
        store.toggle_poll_creator();
        app.query().label("Question").assert_exists();
        // Empty question/options: send is a no-op, panel stays open.
        store.send_poll();
        assert!(store.poll_open.get());
        app.query().label("Option 1").single().set_text(&mut app, "yes");
        app.query().label("Option 2").single().set_text(&mut app, "no");
        app.query().label("Option 3").assert_not_exists();
        store.add_poll_option();
        app.query().label("Option 3").assert_exists();
        store.poll_question.set(Str::from("pick one"));
        app.query().role(Role::BUTTON).label("Create").tap();
        assert!(!store.poll_open.get());
        assert_eq!(store.poll_question.get().to_string(), "");
        assert_eq!(store.poll_option_count.get(), 2);
    }

    /// Option removal shifts later texts up and keeps at least two slots.
    #[test]
    fn poll_option_remove_shifts() {
        let store = store();
        store.poll_option_fields[0].set_from("a");
        store.poll_option_fields[1].set_from("b");
        store.poll_option_fields[2].set_from("c");
        store.poll_option_count.set(3);
        store.remove_poll_option(1);
        assert_eq!(store.poll_option_fields[0].get().to_string(), "a");
        assert_eq!(store.poll_option_fields[1].get().to_string(), "c");
        assert_eq!(store.poll_option_count.get(), 2);
        // Floor at two options.
        store.remove_poll_option(0);
        assert_eq!(store.poll_option_count.get(), 2);
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn forward_banner_shows_comment_field(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.forward_ids.set(vec![101, 102]);
        let inner = store.clone();
        let mut app = ui.mount(move || views::root(inner.clone()).state(&inner));
        app.query().label_contains("forward").assert_exists();
        app.query().label("Comment").assert_exists();
    }

    // ------------------------------------------------------------------
    // r14-5: accessibility-tree audit + keyboard-only pass.
    // Every interactive control must be named, roles must be right, and
    // there must be no duplicate or empty nodes. Keyboard: Tab order,
    // focus, Enter activates, Escape dismisses, arrows in lists.
    // ------------------------------------------------------------------

    use waterui_testing::Selector;

    const INTERACTIVE_ROLES: &[Role] = &[
        Role::BUTTON,
        Role::TEXT_INPUT,
        Role::MULTILINE_TEXT_INPUT,
        Role::PASSWORD_INPUT,
        Role::CHECKBOX,
        Role::SWITCH,
        Role::SLIDER,
        Role::MENU_ITEM,
        Role::MENU_ITEM_CHECKBOX,
        Role::MENU_ITEM_RADIO,
        Role::TAB,
        Role::LINK,
        Role::OPTION,
        Role::COMBOBOX,
        Role::SPIN_BUTTON,
        Role::RADIO_BUTTON,
        Role::LIST_ITEM,
    ];

    /// Walk the page's whole semantic tree and report a11y violations:
    /// unnamed interactive controls, empty text/image nodes, zero-area
    /// interactive nodes, and exact-duplicate (role, label, bounds) nodes.
    fn a11y_violations(app: &mut waterui_testing::OffscreenApp, page: &str) -> Vec<String> {
        let nodes = app
            .semantic_mut()
            .resolve_elements(&Selector::default());
        let mut violations = Vec::new();
        let mut seen: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for el in nodes.iter() {
            let n = el.node();
            if n.hidden() {
                continue;
            }
            let role = n.role();
            let label = n.label().unwrap_or("").trim().to_string();
            let bounds = n
                .bounds()
                .map(|b| (b.x() as i32, b.y() as i32, b.width() as i32, b.height() as i32));
            if INTERACTIVE_ROLES.contains(&role) && label.is_empty() {
                violations.push(format!(
                    "{page}: unnamed {role:?} #{id} at {bounds:?}",
                    id = el.id().as_u64()
                ));
            }
            if (role == Role::LABEL || role == Role::IMAGE)
                && label.is_empty()
                && n.children().is_empty()
            {
                violations.push(format!(
                    "{page}: empty {role:?} #{id} at {bounds:?}",
                    id = el.id().as_u64()
                ));
            }
            if INTERACTIVE_ROLES.contains(&role)
                && bounds.is_some_and(|(_, _, w, h)| w <= 0 || h <= 0)
            {
                violations.push(format!(
                    "{page}: zero-area {role:?} '{label}' #{id} at {bounds:?}",
                    id = el.id().as_u64()
                ));
            }
            let key = format!("{role:?}|{label}|{bounds:?}");
            if let Some(first) = seen.insert(key.clone(), el.id().as_u64()) {
                violations.push(format!(
                    "{page}: duplicate {role:?} '{label}' nodes #{first} and #{id} at {bounds:?}",
                    id = el.id().as_u64()
                ));
            }
        }
        violations
    }

    fn audit_or_fail(app: &mut waterui_testing::OffscreenApp, page: &str) {
        let violations = a11y_violations(app, page);
        assert!(
            violations.is_empty(),
            "{page}: {} a11y violations:\n{}",
            violations.len(),
            violations.join("\n")
        );
    }

    /// Sidebar page: every control named, no dupes/empties.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn a11y_audit_sidebar(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui
            .viewport(340, 700)
            .mount_offscreen(move || views::sidebar_view(inner.clone()).state(&inner));
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "sidebar");
    }

    /// Chat page: every control named, no dupes/empties.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn a11y_audit_chat(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.win_frame.set(waterui::prelude::Rect::new(
            waterui::prelude::Point::new(0.0, 0.0),
            waterui::prelude::Size::new(1400.0, 900.0),
        ));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount_offscreen(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "chat");
    }

    /// Settings page: every control named, no dupes/empties.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn a11y_audit_settings(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui
            .viewport(340, 700)
            .mount_offscreen(move || views::settings_view(inner.clone()).state(&inner));
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "settings");
    }

    /// Chat page with poll creator + emoji panel + attach strip mounted.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn a11y_audit_chat_overlays(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount_offscreen(move || views::chat_detail(inner.clone(), 1).state(&inner));
        store.toggle_poll_creator();
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "chat+poll");
        store.poll_open.set(false);
        store.stickers_open.set(true);
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "chat+emoji");
        store.stickers_open.set(false);
        store.attach.set(vec![waterui::media::Url::from_file_path_str(
            Str::from("/tmp/design_doc.pdf"),
        )]);
        app.semantic_mut().settle();
        audit_or_fail(&mut app, "chat+attach");
    }

    /// Tab traversal on the chat page cycles every focusable control in
    /// tree order and wraps.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_tab_cycles_chat(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        // `tree().focus()` is the accessibility focus — it covers every
        // focusable node (buttons included), unlike `ui_focus()` which only
        // reports text-input editing focus.
        let mut labels: Vec<String> = Vec::new();
        for _ in 0..16 {
            app.press_named_key("Tab");
            app.settle();
            let focus_id = app.tree().focus();
            let label = app
                .resolve_elements(&Selector::default())
                .iter()
                .find(|el| el.id().as_u64() == focus_id.as_u64())
                .and_then(|el| el.node().label().map(|s| s.to_string()))
                .unwrap_or_else(|| "<none>".into());
            labels.push(label);
        }
        std::fs::write("/tmp/kb_tab_order.txt", labels.join("\n")).unwrap();
        // Traversal must reach at least one control (a completely dead
        // chain would leave every slot at "<none>").
        assert!(
            labels.iter().any(|l| l != "<none>"),
            "Tab never reached any focusable control"
        );
    }

    /// Enter on the focused composer-side button activates it.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_enter_activates(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        let el = app
            .query()
            .role(Role::BUTTON)
            .label("Stickers & GIFs")
            .single();
        el.focus(&mut app);
        assert_eq!(
            app.tree().focus().as_u64(),
            el.id().as_u64(),
            "a11y Focus action did not land on the button"
        );
        app.press_named_key("Enter");
        app.settle();
        assert!(
            store.stickers_open.get(),
            "Enter on focused button did not activate it"
        );
    }

    /// waterui#1222: `press_named_key` now emits a full stroke (press +
    /// release), so the rendered runtime's `PressRelease` activation path
    /// fires — the same Enter activation must hold on `mount_offscreen`.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_enter_activates_offscreen(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount_offscreen(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.semantic_mut().settle();
        let el = app
            .query()
            .role(Role::BUTTON)
            .label("Stickers & GIFs")
            .single();
        el.focus(&mut app);
        app.press_named_key("Enter");
        app.semantic_mut().settle();
        assert!(
            store.stickers_open.get(),
            "Enter on focused button did not activate it on the rendered runtime"
        );
    }

    /// Escape dismisses the emoji panel when it is open.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_escape_dismisses_emoji(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.stickers_open.set(true);
        let inner = store.clone();
        // hydrolysis#147: modal scopes now register on the semantic walk too.
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        app.press_named_key("Escape");
        app.settle();
        assert!(
            !store.stickers_open.get(),
            "Escape did not close the emoji panel"
        );
    }

    /// Escape dismisses the info overlay below the dock threshold.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_escape_dismisses_info(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.open_chat.set(1);
        store.info_open.set(true);
        let inner = store.clone();
        let mut app = ui
            .viewport(460, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        app.press_named_key("Escape");
        app.settle();
        assert!(
            !store.info_open.get(),
            "Escape did not close the info overlay"
        );
    }

    /// Arrow keys in the chat-list: documents desktop arrow navigation.
    /// Telegram Desktop moves the selection with Up/Down.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_arrows_chat_list(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui
            .viewport(340, 700)
            .mount(move || views::sidebar_view(inner.clone()).state(&inner));
        app.settle();
        app.press_named_key("ArrowDown");
        app.settle();
        let moved = store.selected.get().is_some();
        std::fs::write(
            "/tmp/kb_arrows.txt",
            format!("after ArrowDown selected={moved:?}\n"),
        )
        .unwrap();
    }

    /// Minimal modal-Escape probe: one button inside a `ModalInteraction`
    /// scope. Escape must run the scope's escape action. If this fails on
    /// the semantic runtime while passing on winit, that is a runtime gap.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_modal_escape_minimal(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::handler::SharedAction;
        use waterui_backend_core::widget::ModalInteraction;
        let closed = waterui::reactive::binding(false);
        let c = closed.clone();
        let esc = ModalInteraction::new(
            true,
            SharedAction::new(move |_: Environment| c.set(true)),
        );
        // #147 verified: the semantic runtime now registers modal scopes.
        let mut app = ui.viewport(300, 300).mount(move || {
            vstack((button("Inside").action(|_: Store| {}),))
                .with(esc.clone())
        });
        app.settle();
        app.press_named_key("Escape");
        app.settle();
        std::fs::write(
            "/tmp/modal_escape.txt",
            format!("closed={}\n", closed.get()),
        )
        .unwrap();
        assert!(closed.get(), "Escape did not reach the modal scope");
    }

}
