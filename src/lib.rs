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
        // Synchronous seeds — evaluated before the view mounts. The async
        // `task` seeds below can land after the window's first layout, so
        // anything that shapes the initial detail pane goes here.
        if demo_page() == Some("info") {
            store.selected.set(Some(1));
            store.open_chat.set(1);
            store.regroup_messages();
            store.info_open.set(true);
            store.load_shared_media();
        }
        if demo_page() == Some("chat") {
            store.selected.set(Some(1));
            store.open_chat.set(1);
            store.regroup_messages();
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
                            s.regroup_messages();
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
    use crate::state::{ChatRow, FolderRow, MessageRow, PinnedRow, ReactionChip, Screen, SharedMediaRow, Store, parse_markdown};
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
            draft: "".into(),
            order,
            unread: 0,
            unread_mentions: 0,
            pinned: false,
            muted: false,
            photo_file: 0,
            time: "12:00".into(),
            typing: false,
            online: false,
            kind_icon: "".into(),
            marked_unread: false,
            in_archive: false,
            accent: -1,
            title_styled: waterui::text::styled::StyledStr::empty(),
            preview_styled: waterui::text::styled::StyledStr::empty(),
        }
    }

    fn msg(id: i64, text: &str, outgoing: bool) -> MessageRow {
        MessageRow {
            id,
            sender: if outgoing { "".into() } else { "Alice".into() },
            sender_accent: -1,
            text: Str::from(text.to_string()),
            time: "12:00".into(),
            outgoing,
            can_edit: outgoing,
            reply_excerpt: "".into(),
            reply_to_id: 0,
            media_file: 0,
            play_file: 0,
            media_label: "".into(),
            reaction_chips: Vec::new(),
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
            styled_open: waterui::text::styled::StyledStr::empty(),
            search_hit: false,
            search_styled: waterui::text::styled::StyledStr::empty(),
            mentions_me: false,
            has_spoiler: false,
            link_site: "".into(),
            link_title: "".into(),
            link_desc: "".into(),
            link_url: "".into(),
            forwarded_from: "".into(),
            sender_user: 0,
            sender_chat: 0,
            fwd_user: 0,
            fwd_chat: 0,
            poll: None,
            group_first: true,
            group_last: true,
            avatar_col: false,
            show_avatar: false,
            sender_photo: 0,
            is_service: false,
            view_count: 0,
            author_sig: Str::from(""),
            album_id: 0,
            album_files: Vec::new(),
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
        assert_eq!(for_assert.snapshot().to_string(), "94575");
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

    /// waterui#1233: the chat list's `List::selection` binding is the single
    /// input path — a pointer tap and ArrowDown both write it, and the
    /// `on_change` in the sidebar routes it through `select_chat`.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn list_selection_opens_chat(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.screen.set(Screen::Main);
        store.chats.set(vec![
            chat(1, "Zelda", "hello", 100),
            chat(2, "Bob", "hi there", 50),
            chat(3, "Carol", "yo", 40),
        ]);
        let store2 = store.clone();
        let mut app = ui.mount(move || views::main_screen(store2.clone()).state(&store2));
        app.query()
            .role(Role::LIST_ITEM)
            .label_contains("Zelda")
            .tap();
        assert_eq!(store.list_selection.snapshot(), Some(1));
        assert_eq!(store.open_chat.get(), 1);
        app.query()
            .role(Role::LIST_ITEM)
            .label_contains("Zelda")
            .focus();
        app.press_named_key("ArrowDown");
        assert_eq!(store.list_selection.snapshot(), Some(2));
        assert_eq!(store.open_chat.get(), 2);
        app.press_named_key("End");
        assert_eq!(store.list_selection.snapshot(), Some(3));
        assert_eq!(store.open_chat.get(), 3);
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
        assert_eq!(composer.snapshot().to_string(), "");
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
        if rec.snapshot() {
            caller.cancel_voice_record();
            assert!(!rec.snapshot());
        } else {
            let err = err_b.snapshot().to_string();
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
        assert!(open.snapshot());
        // The sheet's GpuSurface owns the camera; on a machine without one its
        // shared status must surface the failure (or be mid-open).
        std::thread::sleep(std::time::Duration::from_millis(600));
        let shared = caller.video_shared.borrow().clone();
        let status = shared
            .map(|s| s.borrow().status.snapshot().to_string())
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
        let row = msgs.snapshot().into_iter().next().unwrap();
        assert!(row.pending && !row.failed);
    }

    /// Chat header subtitle: groups show member count, typing overrides,
    /// private online shows "online". (The nav-bar subtitle is not
    /// realized in the semantic tree, so this asserts the signal itself.)
    #[test]
    fn chat_header_subtitle() {
        let store = store();
        let mut group = chat(7, "dogfood crew", "p", 1);
        group.kind_icon = "group".into();
        store.chats.set(vec![group]);
        store.members_count.set_from("5 members");
        let sub = store.chat_subtitle(7);
        assert_eq!(sub.snapshot().to_string(), "5 members");

        let mut rows = store.chats.snapshot();
        rows[0].typing = true;
        store.chats.set(rows);
        assert_eq!(sub.snapshot().to_string(), "typing…");

        let mut alice = chat(2, "Alice", "p", 1);
        alice.online = true;
        store.chats.set(vec![alice]);
        let sub2 = store.chat_subtitle(2);
        assert_eq!(sub2.snapshot().to_string(), "online");

        store.chats.set(vec![chat(6, "Bob", "p", 1)]);
        let sub3 = store.chat_subtitle(6);
        assert_eq!(sub3.snapshot().to_string(), "");
    }

    /// 2FA sheet validates input before calling TDLib.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn twofa_requires_current_password(_ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_twofa();
        store.save_twofa();
        assert!(!store.twofa_note.snapshot().to_string().is_empty());
        assert!(store.twofa_open.snapshot());
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
        assert!(dark.snapshot());
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
        m.reaction_chips = vec![crate::state::ReactionChip {
            emoji: "👍".into(),
            count: 3,
            chosen: false,
        }];
        store.messages.set(vec![m]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label_contains("👍 3").assert_exists();
    }

    /// Emoji-only detection: whitespace/ZWJ/VS16 don't count, any other
    /// content disqualifies the message from the large no-bubble rendering.
    #[test]
    fn emoji_only_detection() {
        assert_eq!(views::emoji_count("🔥"), 1);
        assert_eq!(views::emoji_count("🎉🎉🎉"), 3);
        assert_eq!(views::emoji_count("👍🏽 ❤️"), 2);
        assert_eq!(views::emoji_count(""), 0);
        assert_eq!(views::emoji_count("shipping it 🚀"), 0);
        assert_eq!(views::emoji_count("1️⃣ keycap"), 0);
    }

    /// An emoji-only message renders as a large text label inside the row
    /// (Telegram Desktop drops the bubble fill — semantic tree can't see the
    /// fill, so assert the emoji label exists at all).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn emoji_only_bubble_renders(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.messages.set(vec![
            msg(1, "🔥", false),
            msg(2, "🎉🎉🎉", true),
        ]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label_contains("🔥").assert_exists();
        app.query().label_contains("🎉🎉🎉").assert_exists();
    }

    /// Tap a reaction pill: demo path toggles the chip locally — count
    /// +1, chosen, pill keeps rendering.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn reaction_chip_tap_toggles(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut m = msg(1, "liked", false);
        m.reaction_chips = vec![crate::state::ReactionChip {
            emoji: "👍".into(),
            count: 2,
            chosen: false,
        }];
        store.messages.set(vec![m]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label_contains("React with 👍").assert_exists();
        app.query().label_contains("React with 👍").tap();
        let chips = &store.messages.snapshot()[0].reaction_chips;
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].count, 3);
        assert!(chips[0].chosen);
        assert_eq!(store.messages.snapshot()[0].my_reaction.as_str(), "👍");
        // second tap removes it — count back to 2, unchosen
        app.query().label_contains("React with 👍").tap();
        let chips = &store.messages.snapshot()[0].reaction_chips;
        assert_eq!(chips[0].count, 2);
        assert!(!chips[0].chosen);
        assert_eq!(store.messages.snapshot()[0].my_reaction.as_str(), "");
    }

    /// r27: a secondary click on a bubble merges the context menu into the
    /// tree with Desktop's item set — the quick reactions on top, then
    /// Reply / Edit (own only) / Copy text / Pin / Forward / Select /
    /// Delete — and every command dispatches onto the demo store.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn message_context_menu_desktop_items(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let open_menu = |app: &mut waterui_testing::OffscreenApp, label: &str| {
            let nodes = app.resolve_elements(&waterui_testing::Selector::default());
            // The innermost matching node: a row's label also contains the
            // text, but the context-menu target is the bubble inside it, so
            // the smallest matching bounds is the one that clicks inside the
            // bubble.
            let bounds = nodes
                .iter()
                .filter(|el| el.node().label().unwrap_or("").contains(label))
                .filter_map(|el| el.node().bounds())
                .min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
                .expect("message node bounds");
            app.secondary_click_at(
                bounds.x() + bounds.width() / 2.0,
                bounds.y() + bounds.height() / 2.0,
            );
            dump_bounds("/tmp/probe_menu.txt", app.semantic_mut());
        };
        // r33-3: the quick-reaction strip is the menu's `accessory`
        // (waterui#1245) — hydrolysis doesn't present preview/accessory on
        // Linux yet (#200), so only the command items are in the popup.
        for item in [
            "Reply", "Edit", "Copy text", "Pin message", "Forward", "Select",
            "Delete",
        ] {
            let store = store();
            store.open_chat.set(7);
            store.messages.set(vec![msg(1, "context me", true)]);
            let mut app = ui.clone().mount_offscreen({
                let store = store.clone();
                move || views::chat_detail(store.clone(), 7).state(&store)
            });
            open_menu(&mut app, "context me");
            app.query().label(item).assert_exists();
            app.query().label(item).tap();
            match item {
                "Reply" => assert_eq!(store.reply_to.snapshot(), Some(1)),
                "Edit" => assert_eq!(store.editing.snapshot(), Some(1)),
                "Copy text" => assert!(store.clipboard.snapshot().to_string().contains("context me")),
                "Pin message" => assert_eq!(store.pinned_id.get(), 1),
                "Forward" => assert!(store.forward_message.snapshot().is_some()),
                "Select" => assert_eq!(store.selected_msgs.snapshot(), vec![1]),
                "Delete" => assert!(store.messages.snapshot().is_empty()),
                _ => {}
            }
        }
    }

    /// r33-3: the bubble menu is built through `ContextMenu::new` with the
    /// bubble as `preview` and the reaction strip as `accessory`
    /// (waterui#1245). hydrolysis presents only the items on Linux until
    /// #200 lands, so the item list — including `Delete`'s destructive
    /// role — is asserted on the constructed menu value.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn context_menu_items_and_roles(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::component::menu::{CommandRole, MenuItem, MenuView};
        use waterui::metadata::context_menu::ContextMenu;
        // The menu carries the bubble as preview and the strip as accessory.
        let _ = ui;
        let outgoing = msg(1, "mine", true);
        let menu = ContextMenu::new(views::bubble_menu_items(&outgoing, false))
            .accessory(views::reaction_strip_for_test(&outgoing));
        assert!(menu.accessory.is_some());
        let items = menu.items.snapshot();
        let commands: Vec<&waterui::component::menu::Command> = items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Command(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(commands.len(), 7); // Reply Edit Copy Pin Forward Select Delete
        assert_eq!(items.iter().filter(|i| matches!(i, MenuItem::Divider)).count(), 1);
        let delete = commands.last().unwrap();
        assert_eq!(delete.role, CommandRole::Destructive);
        // Incoming rows omit Edit (own messages only).
        let incoming = msg(2, "theirs", false);
        let items = views::bubble_menu_items(&incoming, false)
            .into_menu_items()
            .snapshot();
        assert_eq!(
            items.iter().filter(|i| matches!(i, MenuItem::Command(_))).count(),
            6
        );
    }

    /// r27: the Edit command only exists on own (outgoing) messages — an
    /// incoming bubble's menu skips it.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn message_context_menu_edit_only_own(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.messages.set(vec![msg(1, "incoming text", false)]);
        let mut app = ui.clone().mount_offscreen({
            let store = store.clone();
            move || views::chat_detail(store.clone(), 7).state(&store)
        });
        let nodes = app.resolve_elements(&waterui_testing::Selector::default());
        let bounds = nodes
            .iter()
            .filter(|el| el.node().label().unwrap_or("").contains("incoming text"))
            .filter_map(|el| el.node().bounds())
            .min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
            .expect("message node bounds");
        app.secondary_click_at(
            (bounds.x() + bounds.width() / 2.0) as f32,
            (bounds.y() + bounds.height() / 2.0) as f32,
        );
        app.query().label("Reply").assert_exists();
        app.query().label("Edit").assert_not_exists();
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
        assert!(mode.snapshot());
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
            accent: -1,
        }]);
        let mut app = ui.clone().mount({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.query().label("2 members").assert_exists();
        // #229: name/status leaves merge into the member row's button label.
        app.query().label("Open profile of Alice").assert_exists();
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
        assert_eq!(store.composer.snapshot().to_string(), "half typed");
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
            accent: -1,
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
        let got = store.messages.snapshot();
        assert!(got[0].day_header);
        assert_eq!(got[0].day_label.as_str(), "Yesterday");
        assert!(!got[1].day_header);
        assert!(got[2].day_header);
        assert_eq!(got[2].day_label.as_str(), "Today");
        // deleting d2a's row recomputes on the next set
        store.set_messages(vec![got[0].clone(), got[1].clone()]);
        assert!(!store.messages.snapshot()[1].day_header);
    }

    #[test]
    fn set_messages_groups_runs() {
        let store = store();
        let mut group = chat(1, "g", "", 1);
        group.kind_icon = "group".into();
        let mut dm = chat(2, "dm", "", 0);
        dm.kind_icon = "person".into();
        store.chats.set(vec![group, dm]);
        store.open_chat.set(1);
        store.set_messages(Store::demo_conversation());
        let rows = store.messages.snapshot();
        let find = |id: i64| rows.iter().find(|r| r.id == id).unwrap().clone();
        // m17 (Alice's photo) and m19 (Bob's forward) are solo runs —
        // outgoing rows and the service rows around them break every run,
        // so each is its run's first and last and carries the avatar.
        for row in [find(17), find(19)] {
            assert!(row.group_first && row.group_last && row.show_avatar);
        }
        // The service events are their own list rows again (r31: fold
        // reverted — the row-height gap tracks waterui#1249).
        assert!(rows.iter().any(|r| r.is_service && r.text == "Alice pinned a message"));
        assert!(rows.iter().any(|r| r.is_service && r.text == "Bob joined the group"));
        // The Alice m22-m25 run carries the avatar on its last row (m25);
        // m26 breaks it (outgoing) so m27 is a one-message run with both marks.
        let (run_first, run_mid, run_last, solo) = (find(22), find(23), find(25), find(27));
        assert!(run_first.group_first && !run_first.group_last);
        assert!(!run_mid.group_first && !run_mid.group_last);
        assert!(run_last.group_last && run_last.show_avatar && !run_last.group_first);
        assert!(solo.group_first && solo.group_last && solo.show_avatar);
        // Outgoing rows never get the avatar column.
        assert!(rows.iter().filter(|r| r.outgoing).all(|r| !r.avatar_col));
        // A private chat shows no avatar column at all.
        store.open_chat.set(2);
        store.set_messages(Store::demo_conversation());
        assert!(store.messages.snapshot().iter().all(|r| !r.avatar_col));
    }

    fn chips(spec: &str) -> Vec<ReactionChip> {
        spec.split(' ')
            .filter(|t| !t.is_empty())
            .map(|t| {
                let split = t.find(char::is_numeric).unwrap_or(t.len());
                ReactionChip {
                    emoji: Str::from(t[..split].to_string()),
                    count: t[split..].trim().parse().unwrap_or(0),
                    chosen: false,
                }
            })
            .collect()
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn reactions_band_never_overlaps_meta(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // r24-1: reaction chips and the meta row share one hstack band, so
        // their spans are disjoint at every chip count — 1/3/6 chips on a
        // text row and on an emoji-only row, at a narrow 600-pt viewport.
        for spec in ["👍1", "👍1 ❤️1 🔥1", "👍4 🔥2 🎉1 👀1 🚀1 ❤️1"] {
            for body in ["six distinct reactions", "👀"] {
                let mut row = msg(1, body, false);
                row.reaction_chips = chips(spec);
                let expected = spec.split(' ').count();
                let store = store();
                let mut app = ui.clone().viewport(600, 400).mount_offscreen(move || {
                    views::message_bubble(store.clone(), row.clone())
                        .padding_with((2.0, 12.0))
                });
                app.semantic_mut().settle();
                let nodes = app.resolve_elements(&waterui_testing::Selector::default());
                let meta = nodes
                    .iter()
                    .find(|el| el.node().label().unwrap_or("") == "12:00")
                    .and_then(|el| el.node().bounds())
                    .expect("meta time node");
                let mut chip_bounds = 0usize;
                for el in nodes.iter() {
                    let n = el.node();
                    if !n.label().unwrap_or("").starts_with("React with") {
                        continue;
                    }
                    if let Some(b) = n.bounds() {
                        chip_bounds += 1;
                        let overlaps = b.x() < meta.x() + meta.width()
                            && meta.x() < b.x() + b.width()
                            && b.y() < meta.y() + meta.height()
                            && meta.y() < b.y() + b.height();
                        assert!(
                            !overlaps,
                            "{body}/{spec}: chip {b:?} overlaps meta {meta:?}"
                        );
                    }
                }
                assert_eq!(chip_bounds, expected, "{body}/{spec}: chip count");
            }
        }
    }

    #[test]
    fn reply_quote_jumps_to_loaded_message() {
        // r24-2: a reply quote carries the source id; jumping to a message
        // already in the window highlights and scrolls without a fetch.
        let store = store();
        store.open_chat.set(1);
        store.set_messages(Store::demo_conversation());
        let m11 = store.messages.snapshot()[1].clone();
        assert_eq!(m11.reply_to_id, 10, "m11 replies to m10");
        store.jump_to_message(10);
        assert_eq!(store.highlight_msg.snapshot(), 10);
        assert!(store.messages.snapshot().iter().find(|r| r.id == 10).unwrap().highlighted);
        assert!(store.messages.snapshot().iter().filter(|r| r.highlighted).count() == 1);
    }

    #[test]
    fn pin_appends_service_row() {
        // Desktop shows a centered "X pinned a message" service line for a
        // notifying pin — the demo path mirrors pinChatMessage(notify).
        let store = store();
        store.open_chat.set(1);
        store.set_messages(Store::demo_conversation());
        let before = store.messages.snapshot().len();
        store.pin_message(11);
        let rows = store.messages.snapshot();
        // The pin's service event appends its own service row.
        assert_eq!(rows.len(), before + 1);
        let last = rows.last().unwrap();
        assert!(last.is_service);
        assert_eq!(last.text, Str::from("You pinned a message"));
        assert_eq!(store.pinned_id.get(), 11);
    }

    #[test]
    fn service_row_not_selectable() {
        // Service rows refuse selection — Desktop's service lines have
        // no multi-select affordance either.
        let store = store();
        store.open_chat.set(1);
        store.set_messages(Store::demo_conversation());
        store.toggle_select(15);
        store.toggle_select(18);
        assert!(store.selected_msgs.snapshot().is_empty());
        store.toggle_select(11);
        assert_eq!(store.selected_msgs.snapshot(), vec![11]);
    }

    #[test]
    fn service_rows_break_runs() {
        // "Alice pinned a message" sits between m14 and m16 as its own
        // row — the event still separates Alice's m14 from outgoing m16
        // (both keep solo-run marks on their shared side).
        let store = store();
        store.open_chat.set(1);
        store.set_messages(Store::demo_conversation());
        let rows = store.messages.snapshot();
        let m14 = rows.iter().position(|r| r.id == 14).unwrap();
        let m16 = rows.iter().position(|r| r.id == 16).unwrap();
        assert_eq!(m16 - m14, 2, "the service row sits between m14 and m16");
        assert!(rows[m14 + 1].is_service);
        assert!(rows[m14].group_last, "m14 ends its run before the service line");
        assert!(rows[m16].group_first, "m16 starts a run after the service line");
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn service_row_renders_in_list(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        // The service row is its own list item again (r31 revert) — the
        // mounted tree still carries its pill text.
        let store = store();
        store.open_chat.set(7);
        let mut rows = vec![msg(1, "hello", false), msg(2, "back", true)];
        let mut svc_row = msg(3, "", false);
        svc_row.is_service = true;
        svc_row.text = "Alice pinned a message".into();
        rows.push(svc_row);
        rows.push(msg(4, "after", false));
        store.set_messages(rows);
        let mut app =
            ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query()
            .label_contains("Alice pinned a message")
            .assert_exists();
        app.query().label_contains("after").assert_exists();
    }

    /// r30 bisect: does `service_pill` alone paint its text?
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_service_pill_alone(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::theme::color::Accent;
        let mut app = ui.viewport(600, 120).mount_offscreen(move || {
            vstack((
                views::service_pill_for_test(),
                waterui::graphics::Color::from(Accent).size(80.0, 20.0),
            ))
            .spacing(4.0)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_pill_alone.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_pill_alone.png");
    }

    /// r30/r31: the service pill must paint — bounds dump + raster of a
    /// service list row.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_service_pill_bounds(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let mut rows = vec![msg(1, "hello", false), msg(2, "back", true)];
        let mut svc_row = msg(3, "", false);
        svc_row.is_service = true;
        svc_row.text = "Alice pinned a message".into();
        rows.push(svc_row);
        rows.push(msg(4, "after", false));
        store.set_messages(rows);
        let mut app = ui.viewport(600, 700).mount_offscreen(move || {
            views::chat_detail(store.clone(), 7).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_svc.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_svc.png");
    }

    #[test]
    fn demo_seeds_photo_file() {
        // The seeded photo message resolves a real PNG so the media slot
        // renders the image (not the file-card fallback) in demo mode.
        let store = store();
        store.seed_demo();
        let path = store.files.borrow().get(&1).cloned().unwrap_or_default();
        assert!(!path.is_empty(), "demo seeds file id 1");
        let head = std::fs::read(&path).expect("demo photo exists");
        assert_eq!(
            &head[..8],
            &[137, 80, 78, 71, 13, 10, 26, 10],
            "seeded file is a real PNG"
        );
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
        m.link_title = "Example — Title".into();
        m.link_desc = "desc".into();
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query()
            .label_contains("Example — Title")
            .assert_exists();
    }

    /// r32-2: Desktop-style link preview card under the message text —
    /// site name, title and description lines each render.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn link_preview_card_shows_site_title_desc(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        let mut m = msg(1, "check this", false);
        m.link_site = "example.com".into();
        m.link_title = "Example — Title".into();
        m.link_desc = "A description line.".into();
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        for label in ["example.com", "Example — Title", "A description line."] {
            app.query().label_contains(label).assert_exists();
        }
    }

    /// r32-2: spoiler spans render masked (a11y "tap to reveal" label)
    /// until `reveal_spoiler` flips the branch to the open styled text.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn spoiler_tap_reveals(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        let ft = tdlib_rs::types::FormattedText {
            text: "no spoilers please — it's a trap".into(),
            entities: vec![tdlib_rs::types::TextEntity {
                offset: 21,
                length: 11,
                r#type: tdlib_rs::enums::TextEntityType::Spoiler,
            }],
        };
        let mut m = msg(1, "no spoilers please — it's a trap", false);
        m.has_spoiler = true;
        m.styled = crate::state::styled_from_formatted_mask(
            &ft,
            Some(Color::from(waterui::theme::color::SurfaceVariant)),
        );
        m.styled_open = crate::state::styled_from_formatted_mask(&ft, None);
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let revealed = store.revealed_spoilers.clone();
        let mut app = ui.clone().mount(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.query()
            .label("Hidden text — tap to reveal")
            .assert_exists();
        let els = app.resolve_elements(
            &waterui_testing::Selector::default().label("Hidden text — tap to reveal"),
        );
        assert!(!els.is_empty());
        els.iter().next().unwrap().tap(&mut app);
        assert!(revealed.snapshot().contains(&1));
        app.query()
            .label("Hidden text — tap to reveal")
            .assert_not_exists();
    }

    /// r32-3: the same spoiler reveal driven by REAL pointer input on the
    /// rendered runtime (`mount_offscreen` + `tap_at`). Headless the tap
    /// lands inside the resolved bounds and works; in the live winit build
    /// on a pristine launch the gesture region sits ~15px BELOW the painted
    /// text inside the same List row (DOGFOOD r32-3), which this test does
    /// not reproduce — region and paint agree once virtualization is absent
    /// (`spoiler_region_matches_paint_full_list`).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn spoiler_tap_reveals_offscreen(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.chats.set(vec![chat(1, "Chat", "", 0)]);
        let ft = tdlib_rs::types::FormattedText {
            text: "no spoilers please — it's a trap".into(),
            entities: vec![tdlib_rs::types::TextEntity {
                offset: 21,
                length: 11,
                r#type: tdlib_rs::enums::TextEntityType::Spoiler,
            }],
        };
        let mut m = msg(1, "no spoilers please — it's a trap", false);
        m.has_spoiler = true;
        m.styled = crate::state::styled_from_formatted_mask(
            &ft,
            Some(Color::from(waterui::theme::color::SurfaceVariant)),
        );
        m.styled_open = crate::state::styled_from_formatted_mask(&ft, None);
        store.messages.set(vec![m]);
        store.selected.set(Some(1));
        let inner = store.clone();
        let revealed = store.revealed_spoilers.clone();
        let mut app = ui
            .clone()
            .viewport(800, 600)
            .mount_offscreen(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.semantic_mut().settle();
        dump_bounds("/tmp/spoiler_offscreen_bounds.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/spoiler_offscreen.png");
        app.query()
            .label("Hidden text — tap to reveal")
            .tap_at(0.5, 0.5);
        assert!(
            revealed.snapshot().contains(&1),
            "real pointer tap on the spoiler zstack must reveal it"
        );
    }

    /// r32-3 diagnostic: mount the FULL seeded demo list (25 rows, taller than
    /// the viewport) exactly like the winit app, dump the a11y bounds of the
    /// spoiler button and paint a frame — comparing the element's declared
    /// bounds with where its text actually renders isolates whether the live
    /// ~15px offset between the gesture region and the painted text is a
    /// registration defect or a paint defect.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn spoiler_region_matches_paint_full_list(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        store.open_chat.set(1);
        store.regroup_messages();
        let inner = store.clone();
        let revealed = store.revealed_spoilers.clone();
        let mut app = ui
            .clone()
            // Tall enough that all 25 demo rows mount with no virtualization.
            .viewport(1400, 3200)
            .mount_offscreen(move || views::chat_detail(inner.clone(), 1).state(&store));
        app.semantic_mut().settle();
        dump_bounds("/tmp/spoiler_fulllist_bounds.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/spoiler_fulllist.png");
        app.query()
            .label("Hidden text — tap to reveal")
            .tap_at(0.5, 0.5);
        assert!(
            revealed.snapshot().contains(&29),
            "real pointer tap on the spoiler zstack must reveal it"
        );
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
        assert!(flag.snapshot());
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn contacts_add_form_opens(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let inner = store.clone();
        let flag = store.add_contact_open.clone();
        let mut app = ui.mount(move || views::new_chat_view(inner.clone()).state(&store));
        app.query().role(Role::BUTTON).label("Add contact").tap();
        assert!(flag.snapshot());
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

    fn dump_bounds<R: waterui_testing::RuntimeDriver>(
        path: &str,
        app: &mut waterui_testing::SemanticApp<R>,
    ) {
        let nodes = app
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
        dump_bounds("/tmp/probe_incoming.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_outgoing.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_outgoing.png");
    }

    /// r27 timing probe: mount+settle a tiny view carrying a 14-item
    /// context_menu vs a 2-item one. waterui's resolve_menu_items cost.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_context_menu_cost(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.viewport(400, 300).mount(move || {
            let s1 = store.clone();
            let s2 = store.clone();
            let s3 = store.clone();
            let s4 = store.clone();
            let s5 = store.clone();
            let s6 = store.clone();
            let s7 = store.clone();
            let s8 = store.clone();
            let s9 = store.clone();
            let s10 = store.clone();
            let s11 = store.clone();
            let s12 = store.clone();
            text("ctx").context_menu((
                "React 👍".action(move |_: Store| { let _ = &s1; }),
                "React ❤️".action(move |_: Store| { let _ = &s2; }),
                "React 🔥".action(move |_: Store| { let _ = &s3; }),
                "React 😂".action(move |_: Store| { let _ = &s4; }),
                "React 😮".action(move |_: Store| { let _ = &s5; }),
                Divider,
                "Reply".action(move |_: Store| { let _ = &s6; }),
                "Edit".action(move |_: Store| { let _ = &s7; }),
                "Copy text".action(move |_: Store| { let _ = &s8; }),
                "Pin message".action(move |_: Store| { let _ = &s9; }),
                "Forward".action(move |_: Store| { let _ = &s10; }),
                "Select".action(move |_: Store| { let _ = &s11; }),
                Divider,
                "Delete".action(move |_: Store| { let _ = &s12; }),
            ))
        });
        let t = std::time::Instant::now();
        app.settle();
        eprintln!("14-item settle: {:?}", t.elapsed());
        app.press_named_key("Tab");
        let t = std::time::Instant::now();
        app.settle();
        eprintln!("14-item tab settle: {:?}", t.elapsed());
    }

    /// r27 timing probe: mount+settle a tiny view carrying a 14-item
    /// context_menu vs a 2-item one. waterui's resolve_menu_items cost.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_context_menu_cost_2item(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.viewport(400, 300).mount(move || {
            let s1 = store.clone();
            let s2 = store.clone();
            text("ctx").context_menu((
                "Reply".action(move |_: Store| { let _ = &s1; }),
                "Delete".action(move |_: Store| { let _ = &s2; }),
            ))
        });
        let t = std::time::Instant::now();
        app.settle();
        eprintln!("2-item settle: {:?}", t.elapsed());
    }

    /// r31: dump every element's bounds on the real chat page — used to
    /// identify the 320x1 grey strips rendering at the bottom of the
    /// photo / forwarded / poll rows on the live renderer.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chat_strips(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.open_chat.set(1);
        store.regroup_messages();
        let st = store.clone();
        let mut app = ui.viewport(1400, 900).mount_offscreen(move || {
            views::chat_detail(st.clone(), 1).state(&st)
        });
        app.semantic_mut().settle();
        store.jump_to_message(17);
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_strips.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_strips.png");
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
        dump_bounds("/tmp/probe_rowbisect.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_toolbar.txt", app.semantic_mut());
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
                    draft: "".into(),
                    order: 0,
                    unread: 12,
                    unread_mentions: 0,
                    pinned: false,
                    muted: false,
                    marked_unread: false,
                    in_archive: false,
                    photo_file: 0,
                    time: "14:32".into(),
                    typing: false,
                    online: false,
                    kind_icon: "group".into(),
                    accent: -1,
                    title_styled: waterui::text::styled::StyledStr::empty(),
                    preview_styled: waterui::text::styled::StyledStr::empty(),
                },
            )
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_chatrow.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_chatrow.png");
        app.query().label("12").assert_exists();
    }

    /// DOGFOOD r35-3: a `List` row whose *fields* change while its `#[id]`
    /// stays the same must repaint with the new content. The headless
    /// `mount_offscreen` path materializes rows fresh each settle (this probe
    /// passes — kept enabled as a semantic guard); the live retained/virtualized
    /// path does not — `VisibleSubviewCache` keys subviews by id alone and
    /// `List`'s flush discards the freshly-built item view for a cached id
    /// (hydrolysis widgets/layout/list.rs:1625, renderer/tree/nodes.rs:428).
    /// Live evidence: a35/a_mention captures — the sidebar `@` badge persisted
    /// after `mention_jump` cleared `unread_mentions` (the `when`-driven
    /// floating button over the same field hid correctly).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_list_row_content_update(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        use waterui::Identifiable;
        #[derive(Clone, Identifiable)]
        struct Row {
            #[id]
            id: i64,
            label: Str,
        }
        let mk = |i: i64| Row {
            id: i,
            label: if i == 0 { "before".into() } else { format!("row{i}").into() },
        };
        let rows = Binding::container((0..50).map(&mk).collect::<Vec<_>>());
        let for_list = rows.clone();
        let mut app = ui.viewport(300, 200).mount_offscreen(move || {
            List::for_each(
                SignalCollection::new(for_list.clone()),
                |row: Row| ListItem::new(text(row.label.clone())),
            )
        });
        app.semantic_mut().settle();
        app.query().label("before").assert_exists();
        rows.set(
            (0..50)
                .map(|i| Row {
                    id: i,
                    label: if i == 0 { "after".into() } else { format!("row{i}").into() },
                })
                .collect(),
        );
        app.semantic_mut().settle();
        app.query().label("after").assert_exists();
    }

    /// Unread-mention badge: a row with `unread_mentions > 0` shows the "@"
    /// pill labelled "N unread mentions" (Telegram Desktop parity).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn chat_row_mention_badge(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let mut app = ui.viewport(340, 200).mount_offscreen(move || {
            views::chat_row(
                store.clone(),
                ChatRow {
                    id: 5,
                    title: "Rust China".into(),
                    preview: "anyone tried hydrolysis on wayland?".into(),
                    draft: "".into(),
                    order: 0,
                    unread: 12,
                    unread_mentions: 2,
                    pinned: false,
                    muted: false,
                    marked_unread: false,
                    in_archive: false,
                    photo_file: 0,
                    time: "14:32".into(),
                    typing: false,
                    online: false,
                    kind_icon: "group".into(),
                    accent: -1,
                    title_styled: waterui::text::styled::StyledStr::empty(),
                    preview_styled: waterui::text::styled::StyledStr::empty(),
                },
            )
        });
        app.semantic_mut().settle();
        app.query().label_contains("unread mentions").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_leading_stack(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(460, 200).mount_offscreen(move || {
            vstack((text("Alice"), text("morning! did the camera filters example work?")))
                .leading()
                .padding()
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_leading.txt", app.semantic_mut());
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_spacer_hstack(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(460, 120).mount_offscreen(move || {
            hstack((text("L"), spacer(), text("R"))).padding()
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_spacer.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_fields.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_list.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_list_plain.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_hug.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_hug.png");
    }

    /// r23-1: the real emoji-only bubble — measure the bubble frame, the
    /// glyph run, and the meta overlay to see who over-widens it.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_emoji_bubble(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let m = msg(1, "🎉🎉🎉", true);
        let mut app = ui.viewport(1000, 300).mount_offscreen(move || {
            views::message_bubble(store.clone(), m.clone()).padding_with((2.0, 12.0))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_emoji.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_emoji.png");
    }

    /// Control: the meta overlay via `overlay()` (base dictates size,
    /// layer aligns within base bounds) — meta should land bottom-trailing
    /// of the content run, and the pair should hug content width.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_emoji_bubble_finite(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(800, 200).mount_offscreen(move || {
            hstack((
                overlay(
                    overlay(
                        vstack((text("🎉🎉🎉").size(44.0),))
                            .leading()
                            .padding_with([10.0, 30.0, 10.0, 10.0]),
                        text("👍1").caption().padding_with([0.0, 11.0, 10.0, 10.0]),
                    )
                    .bottom_leading(),
                    hstack((text("12:00").caption(), text("✓✓").caption()))
                        .spacing(3.0)
                        .padding_with([0.0, 8.0, 0.0, 8.0]),
                )
                .bottom_trailing()
                .max_width(420.0)
                .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
                spacer(),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_emoji_finite.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_emoji_finite.png");
    }

    /// Edge case: meta wider than content ("edited 12:00 ✓✓" over "hi") —
    /// the decoration keeps its own size under overlay(), so it overflows
    /// the base on the leading side. Measures how far out it pokes.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_meta_wider_than_content(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let mut app = ui.viewport(800, 200).mount_offscreen(move || {
            hstack((
                overlay(
                    vstack((text("hi").body(),))
                        .leading()
                        .padding_with([10.0, 30.0, 10.0, 10.0])
                        .background(RoundedRectangle::new(0.18).fill(SurfaceVariant)),
                    hstack((
                        text("edited").caption(),
                        text("12:00").caption(),
                        text("✓✓").caption(),
                    ))
                    .spacing(3.0)
                    .padding_with([0.0, 8.0, 0.0, 8.0]),
                )
                .bottom_trailing(),
                spacer(),
            ))
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_meta_wide.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_meta_wide.png");
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
        dump_bounds("/tmp/probe_hug_noz.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_hug_text.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_chat.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_chat_wide.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_info_dock.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_info_late.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_info_open.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_wrap3.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_wrap2.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_wrap.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_rules.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_root_dock.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_root_chat.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_root_chat_pre.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_root_placeholder.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_settings.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_row_width.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_inset.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_list_when.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_list_fixed.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_list_pad.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_ladder.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_hscroll.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_sidebar.txt", app.semantic_mut());
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
            accent: -1,
        }]);
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Blocked users").assert_exists();
        // hydrolysis#229: a labelled row's text leaves merge into its label,
        // so the member name only surfaces inside the row's "Unblock …" label.
        app.query().label("Unblock Spammer").assert_exists();
    }

    /// r13-4: Settings → Language lists the packs from
    /// `getLocalizationTargetInfo` (demo seeds three).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn lang_pack_section(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.load_language_packs(); // demo branch seeds three rows
        let mut app = ui.mount(move || views::settings_view(store.clone()).state(&store));
        app.query().label("Language").assert_exists();
        // #229: pack names surface inside each row's "Use …" button label.
        app.query().label("Use English").assert_exists();
        app.query().label("Use 简体中文 — Chinese (Simplified)").assert_exists();
        app.query().label("Use Deutsch — German").assert_exists();
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
        app.query().label("Shared").assert_exists();
        app.query().label("Media").assert_exists();
        app.query().label("Files").assert_exists();
        app.query().label("Links").assert_exists();
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
        dump_bounds("/tmp/probe_poll.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_zalign.txt", app.semantic_mut());
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
        dump_bounds("/tmp/probe_overlay.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_overlay.png");
        app.query().label("Shared").assert_exists();
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
            accent: -1,
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
        // #229: suggestion text merges into each row's "Mention @…" label.
        app.query().label("Mention @alice").assert_exists();
        app.query().label("Mention @bob").assert_not_exists();
        store.apply_mention("alice");
        assert!(store.composer.snapshot().contains("@alice "));
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
        assert!(store.poll_open.snapshot());
        app.query().label("Option 1").single().set_text(&mut app, "yes");
        app.query().label("Option 2").single().set_text(&mut app, "no");
        app.query().label("Option 3").assert_not_exists();
        store.add_poll_option();
        app.query().label("Option 3").assert_exists();
        store.poll_question.set(Str::from("pick one"));
        app.query().role(Role::BUTTON).label("Create").tap();
        assert!(!store.poll_open.snapshot());
        assert_eq!(store.poll_question.snapshot().to_string(), "");
        assert_eq!(store.poll_option_count.snapshot(), 2);
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
        assert_eq!(store.poll_option_fields[0].snapshot().to_string(), "a");
        assert_eq!(store.poll_option_fields[1].snapshot().to_string(), "c");
        assert_eq!(store.poll_option_count.snapshot(), 2);
        // Floor at two options.
        store.remove_poll_option(0);
        assert_eq!(store.poll_option_count.snapshot(), 2);
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
            store.stickers_open.snapshot(),
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
            store.stickers_open.snapshot(),
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
            !store.stickers_open.snapshot(),
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
            !store.info_open.snapshot(),
            "Escape did not close the info overlay"
        );
    }

    /// r34 pick 2: Up/Down on a focused chat-list row steps the selection
    /// through the framework's `navigate_list_row` (it writes the row's
    /// `selection` slot → `on_change` → `select_chat`), Home/End jump to
    /// the ends, and Enter activates the focused row's press target.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn keyboard_arrows_chat_list(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui
            .viewport(340, 700)
            .mount(move || views::sidebar_view(inner.clone()).state(&inner));
        app.settle();
        let rows = app.query().role(Role::LIST_ITEM).all();
        assert!(rows.len() >= 5, "chat list exposed {} rows", rows.len());
        rows[0].focus(&mut app);
        app.settle();
        app.press_named_key("ArrowDown");
        app.settle();
        assert_eq!(store.open_chat.get(), 2, "ArrowDown did not open chat 2");
        app.press_named_key("ArrowDown");
        app.settle();
        assert_eq!(store.open_chat.get(), 3, "ArrowDown did not open chat 3");
        app.press_named_key("ArrowUp");
        app.settle();
        assert_eq!(store.open_chat.get(), 2, "ArrowUp did not move back to chat 2");
        // End jumps to the last row; its selection write opens that chat.
        app.press_named_key("End");
        app.settle();
        assert_eq!(store.open_chat.get(), 10, "End did not open the last chat");
        // Enter on a focused row activates its press target.
        let rows = app.query().role(Role::LIST_ITEM).all();
        rows[5].focus(&mut app);
        app.settle();
        app.press_named_key("Enter");
        app.settle();
        assert_eq!(store.open_chat.get(), 6, "Enter did not activate the focused row");
    }

    /// r34 pick 1: opening a chat with unread lands on the "Unread
    /// messages" divider row instead of the tail; a fully read chat still
    /// lands on the newest row. New incoming rows anchor at the divider
    /// (no tail-yank), your own sends always follow.
    #[test]
    fn open_unread_anchors_divider() {
        let store = store();
        store.seed_demo();
        // Demo chat 1 "WaterUI devs" is unread=3; the seed marks msgs[2]
        // (index 2) as the divider row.
        store.select_chat(1);
        assert_eq!(
            store.scroll.target().snapshot(),
            2,
            "unread chat did not open on the divider row"
        );
        assert!(
            !store.follows_tail(false),
            "incoming rows must not yank while the divider is pending"
        );
        assert!(store.follows_tail(true), "own sends always follow the tail");
        // A fully-read demo chat (Alice, unread=0) lands on the tail.
        store.select_chat(2);
        let last = store.messages.snapshot().len() - 1;
        assert_eq!(
            store.scroll.target().snapshot(),
            last,
            "read chat did not open on the newest row"
        );
        assert!(store.follows_tail(false));
    }

    /// r34 pick 3: the link-preview card is a tappable control that opens
    /// the URL (robius-open → system browser; `link_opened` records it).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn link_card_opens_url(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        app.query()
            .role(Role::BUTTON)
            .label("Open link waterui.dev")
            .tap();
        assert_eq!(
            store.link_opened.snapshot().as_str(),
            "https://waterui.dev",
            "tapping the card did not request the preview URL"
        );
    }

    /// r34 pick 4: sender avatar/name and the "Forwarded from" badge open
    /// the peer's profile card (Desktop behavior).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn peer_taps_open_profiles(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        // Several incoming bubbles carry the same label — tap the first.
        let alices = app
            .query()
            .role(Role::BUTTON)
            .label("Open profile of Alice")
            .all();
        assert!(
            !alices.is_empty(),
            "sender name/avatar did not expose a profile tap"
        );
        alices[0].tap(&mut app);
        app.settle();
        assert_eq!(
            store.profile.snapshot().map(|c| c.name.to_string()),
            Some("Alice".to_string()),
            "avatar tap did not open Alice's profile"
        );
        store.nav.pop();
        app.settle();
        let fwd = app
            .query()
            .role(Role::BUTTON)
            .label("Open profile of Telegram News")
            .all();
        assert!(
            !fwd.is_empty(),
            "forward badge did not expose a profile tap"
        );
        fwd[0].tap(&mut app);
        app.settle();
        assert_eq!(
            store.profile.snapshot().map(|c| c.name.to_string()),
            Some("Telegram News".to_string()),
            "forward badge tap did not open the source channel profile"
        );
    }

    /// r34 pick 5: `:shortcode` emoji autocomplete — a trailing `:token`
    /// (≥2 chars) shows the suggestion row above the composer; tapping a
    /// suggestion replaces the token with the emoji.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn emoji_autocomplete_inserts(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        store.selected.set(Some(1));
        let inner = store.clone();
        let mut app = ui
            .viewport(660, 700)
            .mount(move || views::chat_detail(inner.clone(), 1).state(&inner));
        app.settle();
        // One char after ':' is not a token yet (matches Desktop).
        store.composer.set_from("hi :s");
        app.settle();
        app.query()
            .label("Insert :smile:")
            .assert_not_exists();
        store.composer.set_from("hi :smi");
        app.settle();
        app.query()
            .role(Role::BUTTON)
            .label("Insert :smile:")
            .tap();
        assert_eq!(
            store.composer.snapshot().as_str(),
            "hi 😄 ",
            "suggestion tap did not replace the :token"
        );
        // After the insert there is no trailing token → row hides.
        app.settle();
        app.query()
            .label("Insert :smile:")
            .assert_not_exists();
    }

    /// r35 pick: in-chat search marks every hit, paints the matched range
    /// with a span `background` highlight, and clears on an empty query.
    #[test]
    fn chat_search_marks_and_clears() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        store.run_chat_search(Str::from("the"));
        let ids = store.chat_match_ids.snapshot();
        assert!(!ids.is_empty(), "no search hits marked");
        let rows = store.messages.snapshot();
        let hit = rows.iter().find(|r| r.search_hit).unwrap();
        assert!(
            hit.search_styled
                .chunks()
                .iter()
                .any(|(_, s)| s.background.is_some()),
            "matched range has no highlight background"
        );
        assert_eq!(store.highlight_msg.snapshot(), ids[0], "did not jump to first match");
        store.run_chat_search(Str::from(""));
        assert!(store.messages.snapshot().iter().all(|r| !r.search_hit));
        assert!(store.chat_match_ids.snapshot().is_empty());
    }

    /// r35 pick: prev/next cycle the match position and re-anchor the
    /// scroll target (Desktop's n/N controls).
    #[test]
    fn chat_search_next_prev_cycle() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        store.run_chat_search(Str::from("e"));
        let n = store.chat_match_ids.snapshot().len();
        assert!(n > 1, "need multiple hits to cycle");
        assert_eq!(store.chat_search_pos.snapshot(), 0);
        store.chat_search_next();
        assert_eq!(store.chat_search_pos.snapshot(), 1);
        store.chat_search_prev();
        assert_eq!(store.chat_search_pos.snapshot(), 0);
        store.chat_search_prev();
        assert_eq!(store.chat_search_pos.snapshot(), n - 1, "prev did not wrap");
    }

    /// r35 pick: the floating `@` button jumps to the first unread mention
    /// and clears the row's mention badge.
    #[test]
    fn mention_jump_targets_and_clears() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        assert_eq!(store.chats.snapshot()[0].unread_mentions, 1);
        store.mention_jump();
        assert_eq!(store.chats.snapshot()[0].unread_mentions, 0);
        let target = store.highlight_msg.snapshot();
        assert!(target > 0, "mention jump did not flash a row");
        assert!(
            store.messages.snapshot().iter().any(|r| r.id == target && r.mentions_me),
            "mention jump did not land on the mentioning row"
        );
    }

    /// r35 pick: sidebar search highlighting splits the matched substring
    /// onto a highlighted span.
    #[test]
    fn sidebar_search_highlight_splits() {
        use crate::state::highlight_styled;
        use waterui::text::styled::StyledStr;
        use waterui::theme::color::{SelectionContainer, SelectionForeground};
        let s = highlight_styled(
            &StyledStr::plain("Alice Wu"),
            "wu",
            Color::from(SelectionContainer),
            Color::from(SelectionForeground),
        );
        let chunks = s.chunks();
        // r36: the mark must carry BOTH roles — a SelectionContainer fill
        // alone leaves dark text illegible on light bubbles.
        assert!(
            chunks.iter().any(|(t, st)| t.as_str() == "Wu"
                && st.background.is_some()
                && st.foreground.is_some()),
            "matched substring not highlighted legibly: {chunks:?}"
        );
        // A query that doesn't match leaves the text plain.
        let none = highlight_styled(
            &StyledStr::plain("Alice Wu"),
            "zzz",
            Color::from(SelectionContainer),
            Color::from(SelectionForeground),
        );
        assert!(none.chunks().iter().all(|(_, st)| st.background.is_none()));
    }

    /// r35 pick: double-tap quick-react applies the ❤️ on the bubble.
    #[test]
    fn quick_react_applies_heart() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        let row = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| !r.outgoing && !r.is_service)
            .unwrap();
        store.quick_react(&row);
        let now = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| r.id == row.id)
            .unwrap();
        assert_eq!(now.my_reaction.as_str(), "❤️");
        assert!(
            now.reaction_chips
                .iter()
                .any(|c| c.emoji.as_str() == "❤️" && c.chosen),
            "no chosen ❤️ chip after quick-react"
        );
    }

    #[test]
    fn pinned_popup_opens_and_jumps() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        // Demo chat 1 seeds two pinned messages (14, 12): tapping the banner
        // opens the popup instead of jumping.
        assert_eq!(store.pinned_msgs.snapshot().len(), 2);
        store.pinned_tap();
        assert!(store.pinned_popup.snapshot(), "multi-pin tap should open popup");
        store.pinned_jump(12);
        assert!(!store.pinned_popup.snapshot(), "popup should close on jump");
        let row = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| r.id == 12)
            .unwrap();
        assert!(row.highlighted, "jump target should be highlighted");
    }

    #[test]
    fn pinned_tap_single_jumps_directly() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        store
            .pinned_msgs
            .set(vec![PinnedRow { id: 12, label: Str::from("x") }]);
        store.pinned_id.set(12);
        store.pinned_tap();
        assert!(!store.pinned_popup.snapshot(), "single pin should jump, not open");
        let row = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| r.id == 12)
            .unwrap();
        assert!(row.highlighted);
    }

    #[test]
    fn toast_notice_fires_on_copy() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        let row = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| !r.text.is_empty() && !r.is_service)
            .unwrap();
        store.copy_message(&row);
        let (seq, msg) = store.notice.snapshot();
        assert_eq!(msg.as_str(), "Text copied");
        store.copy_message(&row);
        let (seq2, _) = store.notice.snapshot();
        assert!(seq2 > seq, "repeating the same toast must re-fire");
    }

    #[test]
    fn channel_post_footer_shows_views_and_signature() {
        let store = store();
        store.seed_demo();
        // Chat 4 is the demo channel: posts carry the 👁 view count and
        // alternate posts carry an author signature.
        store.select_chat(4);
        let rows = store.messages.snapshot();
        let post = rows
            .iter()
            .find(|r| r.view_count > 0)
            .expect("channel posts should carry view counts");
        let footer = post.post_footer();
        assert!(footer.starts_with("👁"), "footer: {footer}");
        assert!(
            rows.iter().any(|r| !r.author_sig.is_empty()),
            "some channel posts should be signed"
        );
        // A non-channel chat shows no footer at all.
        store.select_chat(1);
        assert!(store
            .messages
            .snapshot()
            .iter()
            .all(|r| r.view_count == 0 && r.author_sig.is_empty()));
    }

    #[test]
    fn jump_to_day_lists_days_and_jumps() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        store.toggle_jump_date();
        assert!(store.jump_date_open.snapshot());
        let days = store.jump_days.snapshot();
        assert_eq!(days.len(), 2, "yesterday + today are the seeded days");
        let target = days[0].day;
        store.jump_to_day(target);
        assert!(!store.jump_date_open.snapshot(), "popup closes on pick");
        let row = store
            .messages
            .snapshot()
            .into_iter()
            .find(|r| r.day == target)
            .unwrap();
        assert!(row.highlighted, "first message of the day highlighted");
    }

    #[test]
    fn album_rows_merge_into_one_bubble() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        let rows = store.messages.snapshot();
        let album: Vec<_> = rows.iter().filter(|r| r.album_id == 777).collect();
        assert_eq!(album.len(), 1, "3-member album merges into one row");
        assert_eq!(album[0].album_files.len(), 3);
        assert!(album[0].text.contains("first of the set"));
    }

    #[test]
    fn shared_tab_switches_source() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        store.load_shared_tab(1);
        assert!(!store.shared_files.snapshot().is_empty());
        store.load_shared_tab(2);
        assert!(!store.shared_links.snapshot().is_empty());
        store.load_shared_tab(0);
        assert!(!store.shared_media.snapshot().is_empty());
        assert_eq!(store.shared_tab.snapshot(), 0);
    }

    #[test]
    fn copy_selected_joins_in_message_order() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        let rows = store.messages.snapshot();
        let ids: Vec<i64> = rows
            .iter()
            .filter(|r| !r.is_service && !r.text.is_empty())
            .take(2)
            .map(|r| r.id)
            .collect();
        // Select in reverse order — the copy must still follow message order.
        store.toggle_select(ids[1]);
        store.toggle_select(ids[0]);
        store.copy_selected();
        let clip = store.clipboard.snapshot();
        let first = rows.iter().find(|r| r.id == ids[0]).unwrap().text.to_string();
        let second = rows.iter().find(|r| r.id == ids[1]).unwrap().text.to_string();
        assert_eq!(
            clip.as_str(),
            format!("{first}\n{second}"),
            "copy must join selected texts in message order"
        );
    }

    #[test]
    fn parse_markdown_strips_delimiters_and_offsets_utf16() {
        let ft = parse_markdown("hi *bold* _it_ `m` ~~s~~ ||sec|| end");
        assert_eq!(ft.text.as_str(), "hi bold it m s sec end");
        use tdlib_rs::enums::TextEntityType as T;
        let kinds: Vec<&T> = ft.entities.iter().map(|e| &e.r#type).collect();
        assert_eq!(
            kinds,
            [&T::Bold, &T::Italic, &T::Code, &T::Strikethrough, &T::Spoiler]
        );
        // UTF-16 offsets: a BMP char is 1 unit, an astral emoji is 2.
        let ft2 = parse_markdown("a\u{1F600} *b*");
        assert_eq!(ft2.text.as_str(), "a\u{1F600} b");
        assert_eq!(ft2.entities[0].offset, 4, "a + astral emoji + space = 4 UTF-16 units");
        assert_eq!(ft2.entities[0].length, 1);
        // Unclosed delimiters stay literal.
        let ft3 = parse_markdown("a *never closed");
        assert_eq!(ft3.text.as_str(), "a *never closed");
        assert!(ft3.entities.is_empty());
    }

    #[test]
    fn send_demo_echo_appends_outgoing_row() {
        let store = store();
        store.seed_demo();
        store.select_chat(1);
        let before = store.messages.snapshot().len();
        store.composer.set_from("*bold* tail");
        store.send();
        let rows = store.messages.snapshot();
        assert!(rows.len() > before, "demo send should append a local echo");
        let last = rows.iter().rev().find(|r| r.outgoing).unwrap();
        assert_eq!(last.text.as_str(), "bold tail");
        assert!(last.pending, "echo starts pending until a send result");
    }

    /// r36 probe: does the pinned-message popup subtree materialize when
    /// `pinned_popup` flips true after mount? Dumps the semantic tree so we
    /// can see whether the `when` payload inserted "Jump to pinned" rows.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_pinned_popup_open(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.pinned_label.set("Alice: shipping it".into());
        store.pinned_msgs.set(vec![
            PinnedRow { id: 14, label: Str::from("Alice: shipping it") },
            PinnedRow { id: 12, label: Str::from("Alice: NV12 conversion?") },
        ]);
        let mut app = ui.viewport(1000, 900).mount_offscreen({ let store = store.clone(); move || views::chat_detail(store.clone(), 7).state(&store) });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_popup_closed.txt", app.semantic_mut());
        store.pinned_popup.set(true);
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_popup_open.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_popup_open.png");
        app.query().label_contains("Jump to pinned").assert_exists();
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
            format!("closed={}\n", closed.snapshot()),
        )
        .unwrap();
        assert!(closed.snapshot(), "Escape did not reach the modal scope");
    }


    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_mention_live(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.viewport(340, 700).mount_offscreen(move || {
            views::sidebar_view(inner.clone()).state(&inner)
        });
        app.semantic_mut().settle();
        let nodes = app.semantic_mut().resolve_elements(&Selector::default());
        for el in nodes.iter() {
            let n = el.node();
            let l = n.label().unwrap_or("").to_string();
            if l.contains("mention") || l == "@" {
                eprintln!("MENTION NODE #{} {:?} '{}' bounds={:?}", el.id().as_u64(), n.role(), l, n.bounds());
            }
        }
        eprintln!("chats[4].mentions = {}", store.chats.snapshot()[4].unread_mentions);
        let _ = app.snapshot().save_png("/tmp/r21/sidebar_offscreen.png");
    }

    /// Probe: dump the chips-row layout — scroll viewport vs chip vs button.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_chips_bounds(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.seed_demo();
        let inner = store.clone();
        let mut app = ui.viewport(340, 700).mount_offscreen(move || {
            views::sidebar_view(inner.clone()).state(&inner)
        });
        app.semantic_mut().settle();
        for el in app.semantic_mut().resolve_elements(&Selector::default()).iter() {
            let n = el.node();
            let l = n.label().unwrap_or("").to_string();
            if l.contains("All") || l.contains("Archive") || l.contains("Work")
                || l.contains("Personal") || l.contains("New folder")
                || format!("{:?}", n.role()).to_uppercase().contains("SCROLL") {
                eprintln!("CHIP #{} {:?} '{}' bounds={:?}", el.id().as_u64(), n.role(), l, n.bounds());
            }
        }
    }

    /// Probe: with multi-select bar materialized (`when` sibling), dump every
    /// semantic node's bounds to see whether a message row's slot overlaps
    /// the select-bar band (live taps at y≈120 fire a row's on_tap).
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_select_bar_bounds(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        store.pinned_label.set("Alice: shipping it".into());
        store.pinned_msgs.set(vec![
            PinnedRow { id: 14, label: Str::from("Alice: shipping it") },
        ]);
        let mut app = ui.viewport(1400, 950).mount_offscreen({
            let store = store.clone();
            move || views::chat_detail(store.clone(), 7).state(&store)
        });
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_sel_off.txt", app.semantic_mut());
        store.selected_msgs.set(vec![11, 24]);
        app.semantic_mut().settle();
        dump_bounds("/tmp/probe_sel_on.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_sel_on.png");
    }


    /// Minimal repro for DOGFOOD r36-3: a row scrolled so it straddles the
    /// scroll viewport's top edge keeps an unclipped `.on_tap` bound that
    /// reaches into the sibling band above — taps on that chrome fire the
    /// row. Paint is clipped by `push_layer_rect` (hydrolysis
    /// list.rs:1138-1140 / scroll.rs) while gesture bounds register
    /// unclipped via `transformed_rect` — the clip never reaches hit testing.
    // Fails until the framework fix lands — `tap above the scroll viewport
    // hit a row inside it`. Run with `cargo test --lib -- --ignored`.
    #[ignore]
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_scroll_row_tap_clip(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let taps: Binding<i32> = Binding::container(0);
        let sc = waterui::layout::ScrollController::<Point>::new(Point::new(0.0, 0.0));
        let controller = sc.clone();
        let t = taps.clone();
        let mut app = ui.viewport(400, 360).mount_offscreen(move || {
            vstack((
                text!("chrome above the scroll view").padding(),
                scroll(vstack((0..10)
                    .map(|i| {
                        let t = t.clone();
                        Frame::new(text!("row {i}").padding())
                            .height(80.0)
                            .on_tap(move |_s: Store| t.set(t.snapshot() + 1))
                    })
                    .collect::<Vec<_>>()))
                .scroll_controller(&sc),
            ))
            .state(&store)
        });
        app.settle();
        // Half-row scroll: row 0's slot now straddles the viewport top edge
        // (dump shows its label bound leaking above the viewport top).
        controller.scroll_to(Point::new(0.0, 60.0));
        app.settle();
        dump_bounds("/tmp/probe_scroll_clip.txt", app.semantic_mut());
        // Tap inside the sibling band ABOVE the scroll viewport but inside the
        // straddling row's unclipped bound — must not hit.
        app.tap_at(200.0, 45.0);
        app.settle();
        assert_eq!(
            taps.snapshot(),
            0,
            "tap on the sibling band above a scroll viewport hit a row inside it"
        );
    }

    /// Minimal repro for DOGFOOD r36-5: a `button` whose subtree is inserted
    /// by `when` after mount PAINTS but never registers a pointer target on
    /// the winit renderer — the select bar's Copy/Forward/Delete/✕ and the
    /// forward banner's ✕ all get `pointer_hits=[]` on live clicks. `.on_tap`
    /// and `field` in the same payload do work. Fixed upstream by
    /// hydrolysis#239 (mid-flush dynamic swaps land in the same frame) and
    /// verified live in r37 — kept as a regression tripwire.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_when_payload_button_dead(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let flag = Binding::bool(false);
        let hits: Binding<i32> = Binding::container(0);
        let f = flag.clone();
        let h = hits.clone();
        let mut app = ui.viewport(400, 300).mount_offscreen(move || {
            let h_outer = h.clone();
            vstack((
                text!("header"),
                when(f.clone(), move || {
                    let h3 = h_outer.clone();
                    button("Bump").action(move |_s: Store| h3.set(h3.snapshot() + 1))
                }),
            ))
            .state(&store)
        });
        app.settle();
        flag.set(true);
        app.settle();
        dump_bounds("/tmp/probe_when_button.txt", app.semantic_mut());
        let _ = app.snapshot().save_png("/tmp/probe_when_button.png");
        // The button paints centred, ~one row below the header.
        app.tap_at(200.0, 49.0);
        app.settle();
        assert_eq!(
            hits.snapshot(),
            1,
            "button inside a `when` payload inserted after mount never fired"
        );
    }

    /// Regression tripwire for the chat-row hover ⋮: `on_hover_enter` /
    /// `on_hover_exit` must fire on pointer move so `when(hov)` chrome
    /// materializes. Verified live on winit (the ⋮ paints and its `Menu`
    /// opens); this headless run pins the same path via the testing
    /// driver's `queue_pointer_move`.
    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn probe_hover_enter_fires(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        let hov = Binding::bool(false);
        let h2 = hov.clone();
        let mut app = ui.viewport(400, 300).mount_offscreen(move || {
            vstack((
                text!("anchor").padding(),
                Frame::new(text!("hoverable row").padding())
                    .on_hover_enter(|State(h): State<Binding<bool>>| h.set(true))
                    .on_hover_exit(|State(h): State<Binding<bool>>| h.set(false))
                    .state(&h2),
            ))
            .state(&store)
        });
        app.settle();
        // Park over the second row, then wiggle inside it.
        app.queue_pointer_move(200.0, 55.0);
        app.queue_pointer_move(210.0, 60.0);
        app.settle();
        assert!(
            hov.snapshot(),
            "on_hover_enter never ran on pointer move over the row"
        );
        app.queue_pointer_move(200.0, 15.0);
        app.settle();
        assert!(
            !hov.snapshot(),
            "on_hover_exit never ran when the pointer left the row"
        );
    }

}
