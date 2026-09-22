#![allow(unknown_lints)]

//! Watergram — a Telegram client built on WaterUI + TDLib.
//!
//! `td` owns the TDLib receive thread and the update channel, `state` owns all
//! application state and TDLib request calls, and `views` is the pure view
//! layer. The receive thread cannot touch WaterUI state (nami bindings are
//! `!Send`), so updates flow through an `async-channel` and are drained by a
//! `.task` future running on the UI executor.

mod state;
mod td;
mod views;

use state::Store;
use waterui::app::App;
use waterui::prelude::*;
use waterui::preview;
use waterui::theme::Theme;

#[preview]
fn main() -> impl View {
    let (client_id, rx) = td::spawn_client();
    let store = Store::new(client_id);
    let s = store.clone();
    views::root(store).task(async move {
        s.start();
        while let Ok(update) = rx.recv().await {
            s.update(update);
        }
    })
}

pub fn app(mut env: Environment) -> App {
    let (client_id, rx) = td::spawn_client();
    let store = Store::new(client_id);
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
                s.start();
                while let Ok(update) = rx2.recv().await {
                    s.update(update);
                }
            })
        },
        env,
    )
}

#[cfg(test)]
mod tests {
    use crate::state::{ChatRow, MessageRow, Screen, Store};
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
            media_label: "".into(),
            reactions: "".into(),
            failed: false,
            pending: false,
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
        let mut app = ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query().label("first message").assert_exists();
        app.query().label("my reply").assert_exists();
        app.query().label("Alice").assert_exists();
        app.query().label("Message").assert_exists();
    }

    #[waterui::test(theme = hydrolysis_m3::Material3::defaults())]
    fn composer_sends_and_clears(ui: UiBuilder<Styled<hydrolysis_m3::Material3>>) {
        let store = store();
        store.open_chat.set(7);
        let composer = store.composer.clone();
        let mut app = ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query().label("Message").single().set_text(&mut app, "hello world");
        app.query().role(Role::BUTTON).label("Send").tap();
        assert_eq!(composer.get().to_string(), "");
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
        let mut app = ui.mount(move || views::chat_detail(store.clone(), 7).state(&store));
        app.query().label("Replying to Alice").assert_exists();
    }
}
