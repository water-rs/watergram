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
use waterui::preview;
use waterui::theme::Theme;

/// `WATERGRAM_DEMO=1` mounts the UI on `seed_demo` data with no TDLib
/// connection — for rendering checks on device-less VMs.
/// `WATERGRAM_DEMO_PAGE` picks the page: `list` (default), `chat`,
/// `settings`.
fn demo_page() -> Option<&'static str> {
    std::env::var("WATERGRAM_DEMO_PAGE").ok().map(|p| match p.as_str() {
        "chat" => "chat",
        "settings" => "settings",
        _ => "list",
    })
}

#[preview]
fn main() -> impl View {
    if std::env::var_os("WATERGRAM_DEMO").is_some() {
        let store = Store::new(0);
        store.seed_demo();
        let page = demo_page();
        return views::root(store.clone()).task(async move {
            match page {
                Some("chat") => store.selected.set(Some(1)),
                Some("settings") => store.nav.push(state::Route::Settings),
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
    }
    env.install(
        Theme::new().color_scheme(
            store
                .dark
                .select(ColorScheme::Dark, ColorScheme::Light),
        ),
    );
    App::new(
        move || {
            let s = store.clone();
            let rx2 = rx.clone();
            views::root(store.clone()).task(async move {
                if demo {
                    match demo_page() {
                        Some("chat") => s.selected.set(Some(1)),
                        Some("settings") => s.nav.push(state::Route::Settings),
                        _ => {}
                    }
                    std::future::pending::<()>().await;
                }
                s.start();
                while let Ok((update, cid)) = rx2.recv().await {
                    s.update(update, cid);
                }
            })
        },
        env,
    )
}

#[cfg(test)]
mod tests {
    use crate::state::{ChatRow, FolderRow, MessageRow, Screen, Store};
    use crate::views;
    use waterui::prelude::*;
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
            read_out: false,
            my_reaction: "".into(),
            styled: waterui::text::styled::StyledStr::empty(),
            webpage: "".into(),
            forwarded_from: "".into(),
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
        app.query().label("Alice").assert_exists();
        app.query().label("Bob").assert_exists();
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
        app.query().label("Alice").assert_exists();
        app.query().label("Bob").assert_not_exists();
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
        app.query().label("●").assert_exists();
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
        let mut app = ui.mount(move || views::sidebar_view(store.clone()).state(&store));
        app.query().role(Role::BUTTON).label("Archive").tap();
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
        }]);
        let mut app = ui.mount(move || views::new_chat_view(store.clone()).state(&store));
        app.query().label("Alice A").assert_exists();
        app.query().label("@alice").assert_exists();
        app.query().label("Contacts").assert_exists();
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
}
