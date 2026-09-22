//! Live end-to-end test against Telegram's *test* data centers.
//!
//! Telegram disabled the self-service `99966XYYYY` test accounts in 2024
//! (see <https://github.com/tdlib/td/issues/3083>), so this test is
//! `#[ignore]` by default and needs a real account registered on the test
//! DCs. To run it:
//!
//! 1. Register a real phone number on the test DC (official iOS app:
//!    Settings icon x10 > Accounts > Login to another account > Test).
//! 2. Run with env vars:
//!
//! ```sh
//! WATERGRAM_API_ID=... WATERGRAM_API_HASH=... \
//! WATERGRAM_TEST_PHONE=+<real number> WATERGRAM_TEST_CODE=<code sent to that device> \
//! cargo test --test tdlib_e2e -- --ignored --nocapture
//! ```
//!
//! The test drives the complete client path this app uses: parameters,
//! phone-code auth, `load_chats`, opening Saved Messages, send, edit,
//! delete, and log out.

use std::time::{Duration, Instant};
use tdlib_rs::{enums, functions, types};

/// tdlib-rs resolves every `functions::*` future through the shared
/// `td_receive` queue — nothing responds unless some thread keeps pumping
/// `receive()`. Spawns that pump; non-response updates are dropped.
fn spawn_pump() {
    std::thread::spawn(|| loop {
        let _ = tdlib_rs::receive();
    });
}

fn auth_state(client: i32) -> enums::AuthorizationState {
    pollster::block_on(functions::get_authorization_state(client))
        .expect("get_authorization_state failed")
}

#[test]
#[ignore = "needs a real account on Telegram test DCs; see module docs"]
fn login_send_receive_on_test_dc() {
    let api_id: i32 = std::env::var("WATERGRAM_API_ID")
        .expect("set WATERGRAM_API_ID")
        .parse()
        .expect("WATERGRAM_API_ID must be an integer");
    let api_hash = std::env::var("WATERGRAM_API_HASH").expect("set WATERGRAM_API_HASH");
    let phone = std::env::var("WATERGRAM_TEST_PHONE").expect("set WATERGRAM_TEST_PHONE");
    let code = std::env::var("WATERGRAM_TEST_CODE").expect("set WATERGRAM_TEST_CODE");

    let client = tdlib_rs::create_client();
    spawn_pump();
    let deadline = Instant::now() + Duration::from_secs(120);

    // A first request is required before tdlib-rs delivers any updates.
    assert!(matches!(
        auth_state(client),
        enums::AuthorizationState::WaitTdlibParameters
    ));

    let dir = std::env::temp_dir().join(format!("watergram-e2e-tdlib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    pollster::block_on(functions::set_tdlib_parameters(
        true, // test DC
        dir.to_string_lossy().into(),
        dir.to_string_lossy().into(),
        String::new(),
        true,
        true,
        true,
        true,
        api_id,
        api_hash,
        "en".into(),
        "watergram-e2e".into(),
        "test".into(),
        "0.1.0".into(),
        client,
    ))
    .expect("set_tdlib_parameters failed");

    assert!(matches!(
        auth_state(client),
        enums::AuthorizationState::WaitPhoneNumber
    ));

    pollster::block_on(functions::set_authentication_phone_number(phone, None, client))
        .expect("set_authentication_phone_number failed");

    assert!(matches!(
        auth_state(client),
        enums::AuthorizationState::WaitCode(_)
    ));

    pollster::block_on(functions::check_authentication_code(code, client))
        .expect("check_authentication_code failed");

    loop {
        if matches!(auth_state(client), enums::AuthorizationState::Ready) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for Ready");
        std::thread::sleep(Duration::from_millis(300));
    }

    pollster::block_on(functions::load_chats(
        Some(enums::ChatList::Main),
        20,
        client,
    ))
    .expect("load_chats failed");

    let chats = pollster::block_on(functions::get_chats(Some(enums::ChatList::Main), 50, client))
        .expect("get_chats failed");
    let enums::Chats::Chats(chats) = chats;

    let mut saved_id = 0i64;
    for id in chats.chat_ids {
        let c = pollster::block_on(functions::get_chat(id, client)).expect("get_chat failed");
        let enums::Chat::Chat(chat) = c;
        if chat.title == "Saved Messages" {
            saved_id = chat.id;
            break;
        }
    }
    assert_ne!(saved_id, 0, "Saved Messages chat not found");

    let text = format!(
        "watergram e2e {}",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_secs()
    );
    let sent = pollster::block_on(functions::send_message(
        saved_id,
        None,
        None,
        None,
        enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: types::FormattedText {
                text: text.clone(),
                entities: Vec::new(),
            },
            link_preview_options: None,
            clear_draft: true,
        }),
        client,
    ))
    .expect("send_message failed");
    let enums::Message::Message(sent_msg) = sent;

    let history = pollster::block_on(functions::get_chat_history(
        saved_id,
        sent_msg.id,
        -5,
        10,
        false,
        client,
    ))
    .expect("get_chat_history failed");
    let enums::Messages::Messages(msgs) = history;
    let found = msgs.messages.iter().flatten().any(|m| {
        matches!(&m.content, enums::MessageContent::MessageText(t) if t.text.text == text)
    });
    assert!(found, "sent message not found in history");

    pollster::block_on(functions::edit_message_text(
        saved_id,
        sent_msg.id,
        enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: types::FormattedText {
                text: format!("{text} (edited)"),
                entities: Vec::new(),
            },
            link_preview_options: None,
            clear_draft: true,
        }),
        client,
    ))
    .expect("edit_message_text failed");

    pollster::block_on(functions::delete_messages(
        saved_id,
        vec![sent_msg.id],
        true,
        client,
    ))
    .expect("delete_messages failed");

    pollster::block_on(functions::log_out(client)).expect("log_out failed");
}
