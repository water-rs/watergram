//! Application state and the TDLib update dispatch.
//!
//! All state lives in [`Store`]: `Binding`s for scalars and `Binding<Vec<_>>`s for
//! rows. Every mutation happens on the UI executor — the TDLib receive thread
//! only feeds an `async_channel` that `Store::run` drains here, and TDLib
//! request futures are spawned with `spawn_local`. That keeps every `nami`
//! type (which is `Rc`-backed and `!Send`) on one thread.

use chrono::Datelike;
use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use tdlib_rs::{enums, functions, types};
use waterui::color::Srgb;
use waterui::form::secure::Secure;
use waterui::text::styled::{Style, StyledStr};
use waterui::layout::{Rect, ScrollController, Size};
use waterui::media::Url;
use waterui::prelude::*;
use waterui::task::spawn_local;

use waterui::Identifiable;
use waterui::log::error;
use waterui::task::sleep;

/// Top-level screen switch. Auth screens swap wholesale (a legitimate
/// `Dynamic::watch` case — the screens are genuinely different view types).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Loading,
    ApiKeys,
    Phone,
    Code,
    Password,
    Register,
    Qr,
    Main,
}

#[derive(Clone, Default)]
pub struct MeInfo {
    pub id: i64,
    pub name: Str,
    pub phone: Str,
    pub username: Str,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default)]
    pub test_dc: bool,
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram")
            .join("config.json")
    }

    /// Saved credentials, overridden by `WATERGRAM_API_ID` / `WATERGRAM_API_HASH`
    /// / `WATERGRAM_TEST_DC` environment variables.
    pub fn load() -> Option<Self> {
        let mut cfg = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok());
        if let Ok(id) = std::env::var("WATERGRAM_API_ID") {
            let mut c = cfg.unwrap_or_default();
            c.api_id = id.parse().unwrap_or(0);
            cfg = Some(c);
        }
        if let Ok(hash) = std::env::var("WATERGRAM_API_HASH") {
            let mut c = cfg.unwrap_or_default();
            c.api_hash = hash;
            cfg = Some(c);
        }
        if let Ok(v) = std::env::var("WATERGRAM_TEST_DC")
            && let Some(c) = cfg.as_mut()
        {
            c.test_dc = matches!(v.as_str(), "1" | "true" | "yes");
        }
        cfg.filter(|c| c.api_id != 0 && !c.api_hash.is_empty())
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// One row in the chat list. `Ord` is inverted so `Vec::sort` orders
/// by `position.order` descending — pinned-first ordering is already encoded
/// in the order value TDLib assigns.
#[derive(Clone, Identifiable)]
pub struct ChatRow {
    #[id]
    pub id: i64,
    pub title: Str,
    pub preview: Str,
    /// Unsent draft text kept in `drafts` (empty = no draft); the row shows
    /// "Draft: <text>" like Telegram Desktop instead of the last message.
    pub draft: Str,
    pub order: i64,
    pub unread: i32,
    pub pinned: bool,
    pub muted: bool,
    pub marked_unread: bool,
    /// Has a position in the Archive list.
    pub in_archive: bool,
    pub photo_file: i32,
    pub time: Str,
    pub typing: bool,
    pub online: bool,
    pub kind_icon: Str,
}

impl PartialEq for ChatRow {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}
impl Eq for ChatRow {}
impl PartialOrd for ChatRow {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for ChatRow {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.order.cmp(&self.order)
    }
}

#[derive(Clone, Identifiable)]
pub struct MessageRow {
    #[id]
    pub id: i64,
    pub sender: Str,
    pub text: Str,
    pub time: Str,
    pub outgoing: bool,
    /// Outgoing message the peer has read (double check).
    pub read_out: bool,
    pub can_edit: bool,
    pub reply_excerpt: Str,
    pub media_file: i32,
    /// Playable media file id (video/voice/audio/animation payload, as
    /// opposed to `media_file` which may hold a thumbnail).
    pub play_file: i32,
    pub media_label: Str,
    pub reactions: Str,
    /// Emoji the current user has chosen on this message, if any.
    pub my_reaction: Str,
    /// Rich-text body; empty when the message has no formatting entities.
    pub styled: StyledStr,
    /// Link-preview card line (site — title · description).
    pub webpage: Str,
    /// "Forwarded from X" attribution, empty when not forwarded.
    pub forwarded_from: Str,
    pub failed: bool,
    pub pending: bool,
    /// Flash-highlighted after a jump-to-message.
    pub highlighted: bool,
    /// Render the "Unread messages" divider above this row (first incoming
    /// message after `last_read_inbox_message_id`).
    pub unread_divider: bool,
    /// Local-calendar day (`NaiveDate::num_days_from_ce`) the message was
    /// sent; 0 when unknown.
    pub day: i64,
    /// First row of a distinct day — renders the date pill above the row.
    pub day_header: bool,
    /// Localized day label ("Today", "Yesterday", "Mar 15", …).
    pub day_label: Str,
    /// Message was edited (`edit_date` set) — meta shows "edited".
    pub edited: bool,
    /// Poll content (question/options/votes) when the message is a poll.
    pub poll: Option<PollRow>,
}

/// A single poll answer option as shown inside a poll bubble.
#[derive(Clone)]
pub struct PollOptRow {
    /// Option index (the `option_ids` value `setPollAnswer` expects).
    pub ix: usize,
    pub text: Str,
    pub pct: i32,
    pub chosen: bool,
}

/// Renderable form of `messagePoll`.
#[derive(Clone)]
pub struct PollRow {
    pub question: Str,
    pub options: Vec<PollOptRow>,
    pub voters: i32,
    pub closed: bool,
}

/// One cell in the info panel's shared-media grid (photo/video from
/// `searchChatMessages` with the PhotoAndVideo filter).
#[derive(Clone, Identifiable)]
pub struct SharedMediaRow {
    #[id]
    pub file: i32,
    /// Fallback label (emoji or "GIF") while the file downloads.
    pub label: Str,
}

/// A three-cell row of the shared-media grid.
#[derive(Clone, Identifiable)]
pub struct MediaChunkRow {
    #[id]
    pub ix: usize,
    pub cells: Vec<SharedMediaRow>,
}

/// Media opened in the in-pane viewer overlay (photo or playable file).
#[derive(Clone)]
pub struct ViewerRow {
    /// File id whose local path the viewer resolves via `file_signal`.
    pub file: i32,
    /// True for playable payloads (video/animation); false for photos.
    pub video: bool,
    pub caption: Str,
    pub from: Str,
}

impl PartialEq for MessageRow {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}
impl Eq for MessageRow {}
impl PartialOrd for MessageRow {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for MessageRow {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.id.cmp(&o.id)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Route {
    Settings,
    NewChat,
    Profile,
}

#[state]
#[derive(Clone)]
pub struct Store {
    pub client_id: Cell<i32>,
    pub screen: Binding<Screen>,
    pub dark: Binding<bool>,
    // auth form
    pub api_id: Binding<Str>,
    pub api_hash: Binding<Str>,
    pub test_dc: Binding<bool>,
    pub phone: Binding<Str>,
    pub code: Binding<Str>,
    pub password: Binding<Secure>,
    pub first_name: Binding<Str>,
    pub last_name: Binding<Str>,
    pub qr_link: Binding<Str>,
    pub password_hint: Binding<Str>,
    pub busy: Binding<bool>,
    pub auth_note: Binding<Str>,
    // main
    pub me: Binding<MeInfo>,
    pub connection: Binding<Str>,
    pub chats: Binding<Vec<ChatRow>>,
    pub server_results: Binding<Vec<ChatRow>>,
    pub search: Binding<Str>,
    pub selected: Binding<Option<i64>>,
    /// Selection the `List` owns (waterui#1233): pointer taps and arrow-key
    /// navigation write this binding; `on_change` routes writes through
    /// `select_chat` so opening a chat keeps its side effects, and it is
    /// snapped back in forwarding mode.
    pub list_selection: Binding<Option<i64>>,
    /// Guards the `list_selection` snap-back write inside `select_chat` so
    /// the `on_change` watcher does not re-enter it.
    pub syncing_selection: Cell<bool>,
    pub messages: Binding<Vec<MessageRow>>,
    pub scroll: ScrollController<usize>,
    pub composer: Binding<Str>,
    pub reply_to: Binding<Option<i64>>,
    pub editing: Binding<Option<i64>>,
    /// (from_chat_id, message_id) of the message being forwarded.
    pub forward_message: Binding<Option<(i64, i64)>>,
    pub attach: Binding<Vec<Url>>,
    pub clipboard: Binding<Str>,
    pub new_chat_input: Binding<Str>,
    pub new_chat_kind: Binding<i32>,
    // non-reactive caches (single UI thread)
    pub drafts: Rc<RefCell<HashMap<i64, Str>>>,
    pub chat_objs: Rc<RefCell<HashMap<i64, types::Chat>>>,
    pub users: Rc<RefCell<HashMap<i64, types::User>>>,
    pub files: Rc<RefCell<HashMap<i32, String>>>,
    pub files_version: Binding<u64>,
    pub typing_gen: Rc<RefCell<HashMap<i64, u64>>>,
    pub my_id: Rc<Cell<i64>>,
    pub nav: NavigationPath<Route>,
    pub open_chat: Rc<Cell<i64>>,
    pub oldest_message: Rc<Cell<i64>>,
    pub loading_history: Binding<bool>,
    pub no_more_history: Binding<bool>,
    /// Human-readable excerpt of the message being replied to.
    pub reply_label: Binding<Str>,
    pub last_typing_sent: Rc<Cell<Instant>>,
    /// Excerpt of the chat's pinned message (empty = none).
    pub pinned_label: Binding<Str>,
    pub pinned_id: Rc<Cell<i64>>,
    /// In-chat message search state.
    pub chat_search_open: Binding<bool>,
    pub chat_search: Binding<Str>,
    pub chat_search_results: Binding<Vec<MessageRow>>,
    /// file_id -> download progress 0-100 for in-flight downloads.
    pub file_progress: Rc<RefCell<HashMap<i32, i32>>>,
    /// Sidebar is showing the Archive list instead of Main.
    pub archive_mode: Binding<bool>,
    /// Members panel for the open group/channel.
    pub members_open: Binding<bool>,
    pub members: Binding<Vec<MemberRow>>,
    /// Total member count label for the panel header.
    pub members_count: Binding<Str>,
    /// Contacts shown in the New Chat screen.
    pub contacts: Binding<Vec<MemberRow>>,
    /// Active-session list on the Settings screen.
    pub sessions: Binding<Vec<SessionRow>>,
    /// Two-step verification status line ("On"/"Off").
    pub twofa: Binding<Str>,
    /// Profile-edit form fields + result line.
    pub edit_first: Binding<Str>,
    pub edit_last: Binding<Str>,
    pub edit_bio: Binding<Str>,
    pub edit_username: Binding<Str>,
    pub profile_note: Binding<Str>,
    /// Group-admin edit fields in the members panel.
    pub admin_title: Binding<Str>,
    pub admin_desc: Binding<Str>,
    /// Sticker/GIF picker panel + items.
    pub stickers_open: Binding<bool>,
    pub sticker_items: Binding<Vec<StickerItem>>,
    /// New-contact form on the contacts screen.
    pub add_contact_open: Binding<bool>,
    pub nc_phone: Binding<Str>,
    pub nc_first: Binding<Str>,
    pub nc_last: Binding<Str>,
    /// Invite link created for the open group/channel.
    pub invite_link: Binding<Str>,
    /// Storage-statistics summary line in Settings.
    pub storage_summary: Binding<Str>,
    /// Read-only privacy rule summary rows (setting label -> audience).
    pub privacy_rows: Binding<Vec<PrivacyRow>>,
    /// User's chat folders from `updateChatFolders` (sidebar tab strip).
    pub folders: Binding<Vec<FolderRow>>,
    /// Which list the sidebar shows: 0 = Main, -1 = Archive, n = folder id.
    /// Kept in sync with `archive_mode` (which views already read).
    pub active_folder: Binding<i32>,
    /// Peer profile card for the Profile route (None until loaded).
    pub profile: Binding<Option<ProfileCard>>,
    /// Picked file for the own-avatar upload on Settings.
    pub avatar_pick: Binding<Vec<Url>>,
    /// Chat avatar picker (group admin section).
    pub chat_avatar_pick: Binding<Vec<Url>>,
    /// Sticker picker emoji-query field.
    pub sticker_query: Binding<Str>,
    /// Media opened in the in-pane viewer overlay (`None` = closed).
    pub viewer: Binding<Option<ViewerRow>>,
    /// Picker panel tab: 0 Emoji, 1 Stickers, 2 GIFs.
    pub panel_tab: Binding<i32>,
    /// Privacy → blocked message senders.
    pub blocked: Binding<Vec<MemberRow>>,
    /// Scheduled messages of the open chat + panel visibility.
    pub scheduled: Binding<Vec<MessageRow>>,
    pub scheduled_open: Binding<bool>,
    /// Right info panel (Desktop's wide-layout details panel): chat
    /// profile + shared media grid.
    pub info_open: Binding<bool>,
    pub shared_media: Binding<Vec<SharedMediaRow>>,
    /// Caption typed in the attachment preview strip.
    pub attach_caption: Binding<Str>,
    /// Live window frame (cloned from the `Window`), used to switch the
    /// info panel between docked and overlay below a width threshold.
    pub win_frame: Binding<Rect>,
    /// Poll creator (composer → attach menu): open flag, question, option
    /// texts in fixed slots gated by `poll_option_count`, and poll flags.
    /// Telegram's `poll_answer_count_max` option is 10; TDLib lets the
    /// question go up to `poll_question_length_max` (255) chars.
    pub poll_open: Binding<bool>,
    pub poll_question: Binding<Str>,
    pub poll_option_fields: Vec<Binding<Str>>,
    pub poll_option_count: Binding<usize>,
    /// `PollTypeRegular::allow_multiple_answers` (regular polls only).
    pub poll_multiple: Binding<bool>,
    /// Regular poll vs quiz (`PollTypeQuiz::correct_option_id` =
    /// `poll_correct`).
    pub poll_quiz: Binding<bool>,
    pub poll_anonymous: Binding<bool>,
    pub poll_correct: Binding<usize>,
    /// Forward without author attribution (`forwardMessages send_copy`).
    pub forward_noattr: Binding<bool>,
    /// Batch of message ids awaiting a forward target.
    pub forward_ids: Binding<Vec<i64>>,
    /// Optional comment sent as a follow-up text message after a forward.
    pub forward_comment: Binding<Str>,
    /// Message multi-selection (batch forward/delete).
    pub selected_msgs: Binding<Vec<i64>>,
    /// Folder editor sheet state.
    pub folder_open: Binding<bool>,
    pub folder_name: Binding<Str>,
    pub folder_contacts: Binding<bool>,
    pub folder_groups: Binding<bool>,
    pub folder_channels: Binding<bool>,
    /// Folder being edited; 0 = creating a new one.
    pub editing_folder: Binding<i32>,
    /// Account switcher rows + dropdown state.
    pub accounts: Binding<Vec<AccountRow>>,
    pub accounts_open: Binding<bool>,
    /// Language packs from `getLocalizationTargetInfo` (Settings → Language).
    pub lang_packs: Binding<Vec<LangRow>>,
    /// Currently applied language pack id ("en" default).
    pub lang_id: Binding<Str>,
    /// Localization key -> value of the applied pack (LanguagePackStringValue
    /// enums so plural forms stay available to `tr`).
    pub lang_strings: Binding<HashMap<String, enums::LanguagePackStringValue>>,
    /// Installed sticker packs shown above the sticker cells.
    pub sticker_packs: Binding<Vec<PackRow>>,
    /// Live voice capture session (recorder + collector thread).
    pub voice_session: Rc<RefCell<Option<crate::capture::VoiceCapture>>>,
    /// Voice-recording state shown in the composer.
    pub recording_voice: Binding<bool>,
    /// "0:12" elapsed label while recording.
    pub voice_elapsed: Binding<Str>,
    /// Last capture error shown in the composer/sheet.
    pub capture_error: Binding<Str>,
    /// Video-note sheet state.
    pub video_note_open: Binding<bool>,
    pub video_recording: Binding<bool>,
    pub video_elapsed: Binding<Str>,
    /// Shared state with the sheet's GpuView (None while the sheet is closed).
    pub video_shared:
        Rc<RefCell<Option<Rc<RefCell<crate::capture::VideoNoteShared>>>>>,
    /// Encoder-thread result channel while a finish is in flight.
    #[allow(clippy::type_complexity)]
    pub video_done_rx:
        Rc<RefCell<Option<mpsc::Receiver<Result<crate::capture::VideoNoteDone, String>>>>>,
    /// Path of the mp4 currently being written.
    pub video_path: Rc<RefCell<String>>,
    /// Message id flash-highlighted after jump-to-message.
    pub highlight_msg: Binding<i64>,
    /// 2FA management sheet.
    pub twofa_open: Binding<bool>,
    pub twofa_old: Binding<Secure>,
    pub twofa_new: Binding<Secure>,
    pub twofa_hint_in: Binding<Str>,
    pub twofa_email: Binding<Str>,
    pub twofa_note: Binding<Str>,
    /// Cached user id of the @gif inline bot (resolved on first search).
    gif_bot: Cell<i64>,
    /// Per-user privacy exception picker: (setting, allow?).
    pub privacy_picker_open: Binding<bool>,
    privacy_target: RefCell<Option<(enums::UserPrivacySetting, bool)>>,
    /// Guards keeping notification-scope watchers alive.
    notif_watchers: Rc<RefCell<Vec<Box<dyn std::any::Any>>>>,
    /// Global notification toggles (per scope; true = notifications on).
    pub notif_private: Binding<bool>,
    pub notif_groups: Binding<bool>,
    pub notif_channels: Binding<bool>,
}

/// One chat folder tab in the sidebar strip.
#[derive(Clone, Identifiable)]
pub struct FolderRow {
    /// Folder id: 0 = All chats, -1 = Archive, n = TDLib folder id.
    #[id]
    pub id: i32,
    pub title: Str,
    /// Currently selected tab (drives accent styling).
    pub active: bool,
}

/// An installed sticker pack for the picker.
#[derive(Clone, Identifiable)]
pub struct PackRow {
    /// TDLib sticker-set id (row key).
    #[id]
    pub id: i64,
    pub title: Str,
}

/// A signed-in TDLib client in the account switcher.
#[derive(Clone, Identifiable)]
pub struct AccountRow {
    /// TDLib client id (also the row key).
    #[id]
    pub id: i32,
    pub label: Str,
}

/// A language pack from `getLocalizationTargetInfo`.
#[derive(Clone, Identifiable)]
pub struct LangRow {
    /// Language pack id (row key).
    #[id]
    pub id: Str,
    /// "native — English" name as shown by Telegram ("Deutsch — German").
    pub name: Str,
    /// Pack marked beta by Telegram.
    pub beta: bool,
    /// Currently applied pack.
    pub active: bool,
}

/// Peer profile card for the Profile route.
#[derive(Clone, Default)]
pub struct ProfileCard {
    pub user_id: i64,
    pub name: Str,
    pub username: Str,
    pub phone: Str,
    pub bio: Str,
    pub online: bool,
}

/// One row of the read-only privacy summary in Settings.
#[derive(Clone, Identifiable)]
pub struct PrivacyRow {
    /// Setting name as the row key.
    #[id]
    pub setting: Str,
    /// Who can see it: "Everyone" / "My contacts" / "Nobody" / "Custom".
    pub audience: Str,
    /// TDLib setting this row controls.
    pub key: enums::UserPrivacySetting,
}

/// How the composer's picked files are delivered.
pub(crate) enum AttachmentPlan {
    Album(Vec<(String, String)>),
    Singles(Vec<(String, String)>),
}

/// One cell in the sticker/GIF picker.
#[derive(Clone, Identifiable)]
pub struct StickerItem {
    /// TDLib file id of the sticker/animation (key).
    #[id]
    pub file_id: i32,
    /// Thumbnail file id (0 = none).
    pub thumb: i32,
    pub emoji: Str,
    /// true = GIF animation, false = sticker.
    pub gif: bool,
}

/// One row in the active-sessions list.
#[derive(Clone, Identifiable)]
pub struct SessionRow {
    #[id]
    pub id: i64,
    pub title: Str,
    pub subtitle: Str,
    pub current: bool,
}

fn fmt_time(ts: i32) -> Str {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
        .into()
}

/// Local calendar day index (`num_days_from_ce`) for a unix timestamp.
fn local_day(ts: i32) -> i64 {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .date_naive()
                .num_days_from_ce() as i64
        })
        .unwrap_or(0)
}

/// "Today" / "Yesterday" / "Mar 15" / "Mar 15, 2025" for a day index.
fn fmt_day_label(day: i64) -> Str {
    let Some(date) = chrono::NaiveDate::from_num_days_from_ce_opt(day as i32) else {
        return Str::from("");
    };
    let today = chrono::Local::now().date_naive();
    let s = if date == today {
        "Today".to_string()
    } else if date == today.pred_opt().unwrap_or(today) {
        "Yesterday".to_string()
    } else if date.year() == today.year() {
        date.format("%b %-d").to_string()
    } else {
        date.format("%b %-d, %Y").to_string()
    };
    Str::from(s)
}

/// Convert a TDLib `FormattedText` (UTF-16 entity offsets) into a
/// `StyledStr`: bold/italic/underline/strike/mono, spoiler as a black
/// block, links in accent blue, quotes on a light background.
pub(crate) fn styled_from_formatted(ft: &types::FormattedText) -> StyledStr {
    let mut styled = StyledStr::empty();
    if ft.entities.is_empty() {
        styled.push_str(ft.text.clone());
        return styled;
    }
    // UTF-16 unit index -> byte offset, mapping entity bounds exactly.
    let mut units: Vec<usize> = Vec::with_capacity(ft.text.len() + 1);
    let mut u = 0usize;
    for (bi, ch) in ft.text.char_indices() {
        while units.len() <= u {
            units.push(bi);
        }
        u += ch.len_utf16();
    }
    while units.len() <= u {
        units.push(ft.text.len());
    }
    let byte_at = |u16: i32| units[u16.max(0) as usize];

    let mut cuts: Vec<usize> = vec![0, ft.text.len()];
    for e in &ft.entities {
        cuts.push(byte_at(e.offset));
        cuts.push(byte_at(e.offset + e.length));
    }
    cuts.sort_unstable();
    cuts.dedup();

    for w in cuts.windows(2) {
        let (b0, b1) = (w[0], w[1]);
        if b0 == b1 {
            continue;
        }
        let seg = &ft.text[b0..b1];
        let (mut bold, mut italic, mut mono) = (false, false, false);
        let (mut under, mut strike, mut spoiler) = (false, false, false);
        let (mut link, mut quote) = (false, false);
        for e in &ft.entities {
            let (s, t) = (byte_at(e.offset), byte_at(e.offset + e.length));
            if s >= b1 || t <= b0 {
                continue;
            }
            match e.r#type {
                enums::TextEntityType::Bold => bold = true,
                enums::TextEntityType::Italic => italic = true,
                enums::TextEntityType::Underline => under = true,
                enums::TextEntityType::Strikethrough => strike = true,
                enums::TextEntityType::Spoiler => spoiler = true,
                enums::TextEntityType::Code
                | enums::TextEntityType::Pre
                | enums::TextEntityType::PreCode(_) => mono = true,
                enums::TextEntityType::BlockQuote
                | enums::TextEntityType::ExpandableBlockQuote => quote = true,
                enums::TextEntityType::Url
                | enums::TextEntityType::TextUrl(_)
                | enums::TextEntityType::EmailAddress
                | enums::TextEntityType::PhoneNumber
                | enums::TextEntityType::Mention
                | enums::TextEntityType::MentionName(_)
                | enums::TextEntityType::Hashtag
                | enums::TextEntityType::Cashtag
                | enums::TextEntityType::BotCommand
                | enums::TextEntityType::BankCardNumber
                | enums::TextEntityType::MediaTimestamp(_) => link = true,
                _ => {}
            }
        }
        let mut st = Style::new();
        if bold {
            st = st.bold();
        }
        if italic {
            st = st.italic();
        }
        if under {
            st = st.underline();
        }
        if strike {
            st = st.strikethrough();
        }
        if mono {
            st = st.monospaced();
        }
        if link {
            st = st.foreground(Srgb::try_from_hex("#1F6FC0").unwrap());
        }
        if spoiler {
            st = st
                .foreground(Srgb::BLACK)
                .background(Srgb::BLACK);
        }
        if quote {
            st = st.background(Srgb::try_from_hex("#E8E8E8").unwrap());
        }
        styled.push(seg.to_string(), st);
    }
    styled
}

fn position_in<'a>(chat: &'a types::Chat, list: &enums::ChatList) -> Option<&'a types::ChatPosition> {
    chat.positions.iter().find(|p| &p.list == list)
}

/// One row in the members panel.
#[derive(Clone, Identifiable)]
pub struct MemberRow {
    /// member_id key: user_id or chat_id.
    #[id]
    pub key: i64,
    pub name: Str,
    pub status: Str,
    /// TDLib sender id for admin actions (kick).
    pub sender: enums::MessageSender,
    /// Active @username for @mention completion (empty for chat senders).
    pub username: Str,
    /// TDLib small profile-photo file id; 0 renders the initials circle.
    pub photo: i32,
}

/// Collapse a TDLib privacy rule list into one audience label. The rule
/// semantics are allow-overlapping-until-restrict; this display version
/// reports the dominant audience only.
pub(crate) fn privacy_audience(rules: &[enums::UserPrivacySettingRule]) -> &'static str {
    use enums::UserPrivacySettingRule as R;
    if rules.iter().any(|r| matches!(r, R::RestrictAll)) {
        return "Nobody";
    }
    if rules.iter().any(|r| matches!(r, R::AllowAll)) {
        return "Everyone";
    }
    let allow_contacts = rules.iter().any(|r| matches!(r, R::AllowContacts));
    let restrict_contacts = rules.iter().any(|r| matches!(r, R::RestrictContacts));
    if allow_contacts && !restrict_contacts && rules.iter().all(|r| {
        matches!(r, R::AllowContacts | R::RestrictAll | R::RestrictUsers(_))
    }) {
        return "My contacts";
    }
    if rules.is_empty() {
        return "Default";
    }
    "Custom"
}

impl Store {
    /// The chat list currently shown in the sidebar.
    fn active_list(&self) -> enums::ChatList {
        let folder = self.active_folder.get();
        if self.archive_mode.get() {
            enums::ChatList::Archive
        } else if folder > 0 {
            enums::ChatList::Folder(types::ChatListFolder {
                chat_folder_id: folder,
            })
        } else {
            enums::ChatList::Main
        }
    }

    pub fn new(client_id: i32) -> Self {
        Self {
            client_id: Cell::new(client_id),
            screen: Binding::container(Screen::Loading),
            dark: Binding::bool(false),
            api_id: Binding::container(Str::from("")),
            api_hash: Binding::container(Str::from("")),
            test_dc: Binding::bool(false),
            phone: Binding::container(Str::from("")),
            code: Binding::container(Str::from("")),
            password: Binding::<Secure>::default(),
            first_name: Binding::container(Str::from("")),
            last_name: Binding::container(Str::from("")),
            qr_link: Binding::container(Str::from("")),
            password_hint: Binding::container(Str::from("")),
            busy: Binding::bool(false),
            auth_note: Binding::container(Str::from("")),
            me: Binding::<MeInfo>::default(),
            connection: Binding::container(Str::from("")),
            chats: Binding::<Vec<ChatRow>>::default(),
            server_results: Binding::<Vec<ChatRow>>::default(),
            search: Binding::container(Str::from("")),
            selected: Binding::default(),
            list_selection: Binding::default(),
            syncing_selection: Cell::new(false),
            messages: Binding::<Vec<MessageRow>>::default(),
            scroll: ScrollController::<usize>::new(0),
            composer: Binding::container(Str::from("")),
            reply_to: Binding::default(),
            editing: Binding::default(),
            forward_message: Binding::default(),
            attach: Binding::<Vec<Url>>::default(),
            clipboard: Binding::container(Str::from("")),
            new_chat_input: Binding::container(Str::from("")),
            new_chat_kind: Binding::i32(0),
            drafts: Rc::new(RefCell::new(HashMap::new())),
            chat_objs: Rc::new(RefCell::new(HashMap::new())),
            users: Rc::new(RefCell::new(HashMap::new())),
            files: Rc::new(RefCell::new(HashMap::new())),
            files_version: Binding::u64(0),
            typing_gen: Rc::new(RefCell::new(HashMap::new())),
            my_id: Rc::new(Cell::new(0)),
            nav: NavigationPath::new(),
            open_chat: Rc::new(Cell::new(0)),
            oldest_message: Rc::new(Cell::new(0)),
            loading_history: Binding::bool(false),
            no_more_history: Binding::bool(false),
            reply_label: Binding::container(Str::from("")),
            last_typing_sent: Rc::new(Cell::new(Instant::now())),
            pinned_label: Binding::container(Str::from("")),
            pinned_id: Rc::new(Cell::new(0)),
            chat_search_open: Binding::bool(false),
            chat_search: Binding::container(Str::from("")),
            chat_search_results: Binding::<Vec<MessageRow>>::default(),
            file_progress: Rc::new(RefCell::new(HashMap::new())),
            archive_mode: Binding::bool(false),
            members_open: Binding::bool(false),
            members: Binding::<Vec<MemberRow>>::default(),
            members_count: Binding::container(Str::from("")),
            contacts: Binding::<Vec<MemberRow>>::default(),
            sessions: Binding::<Vec<SessionRow>>::default(),
            twofa: Binding::container(Str::from("")),
            edit_first: Binding::container(Str::from("")),
            edit_last: Binding::container(Str::from("")),
            edit_bio: Binding::container(Str::from("")),
            edit_username: Binding::container(Str::from("")),
            profile_note: Binding::container(Str::from("")),
            admin_title: Binding::container(Str::from("")),
            admin_desc: Binding::container(Str::from("")),
            stickers_open: Binding::bool(false),
            sticker_items: Binding::<Vec<StickerItem>>::default(),
            add_contact_open: Binding::bool(false),
            nc_phone: Binding::container(Str::from("")),
            nc_first: Binding::container(Str::from("")),
            nc_last: Binding::container(Str::from("")),
            invite_link: Binding::container(Str::from("")),
            storage_summary: Binding::container(Str::from("")),
            privacy_rows: Binding::<Vec<PrivacyRow>>::default(),
            folders: Binding::<Vec<FolderRow>>::default(),
            active_folder: Binding::i32(0),
            profile: Binding::default(),
            avatar_pick: Binding::<Vec<Url>>::default(),
            chat_avatar_pick: Binding::<Vec<Url>>::default(),
            sticker_query: Binding::<Str>::default(),
            // Media viewer overlay (photo/video opened from a bubble).
            viewer: Binding::<Option<ViewerRow>>::default(),
            // Sticker/emoji/GIF panel active tab: 0 Emoji, 1 Stickers, 2 GIFs.
            panel_tab: Binding::i32(0),
            // Privacy → blocked users.
            blocked: Binding::<Vec<MemberRow>>::default(),
            // Scheduled messages of the open chat.
            scheduled: Binding::<Vec<MessageRow>>::default(),
            scheduled_open: Binding::bool(false),
            // Right info panel: profile + shared media.
            info_open: Binding::bool(false),
            shared_media: Binding::<Vec<SharedMediaRow>>::default(),
            // Attachment preview caption.
            attach_caption: Binding::<Str>::default(),
            win_frame: Binding::container(Rect::from_size(Size::zero())),
            // Poll creator (attach menu → Poll).
            poll_open: Binding::bool(false),
            poll_question: Binding::<Str>::default(),
            poll_option_fields: (0..Self::POLL_MAX_OPTIONS)
                .map(|_| Binding::<Str>::default())
                .collect(),
            poll_option_count: Binding::usize(2),
            poll_multiple: Binding::bool(false),
            poll_quiz: Binding::bool(false),
            poll_anonymous: Binding::bool(true),
            poll_correct: Binding::usize(0),
            // Forward "without attribution" (forwardMessages send_copy).
            forward_noattr: Binding::bool(false),
            forward_ids: Binding::<Vec<i64>>::default(),
            forward_comment: Binding::container(Str::from("")),
            // Message multi-selection (Select → batch forward/delete).
            selected_msgs: Binding::<Vec<i64>>::default(),
            folder_open: Binding::bool(false),
            folder_name: Binding::<Str>::default(),
            folder_contacts: Binding::bool(true),
            folder_groups: Binding::bool(true),
            folder_channels: Binding::bool(true),
            editing_folder: Binding::i32(0),
            accounts: Binding::<Vec<AccountRow>>::default(),
            accounts_open: Binding::bool(false),
            lang_packs: Binding::<Vec<LangRow>>::default(),
            lang_id: Binding::container(Str::from("en")),
            lang_strings:
                Binding::<HashMap<String, enums::LanguagePackStringValue>>::default(),
            sticker_packs: Binding::<Vec<PackRow>>::default(),
            voice_session: Rc::new(RefCell::new(None)),
            recording_voice: Binding::bool(false),
            voice_elapsed: Binding::container(Str::from("")),
            capture_error: Binding::container(Str::from("")),
            video_note_open: Binding::bool(false),
            video_recording: Binding::bool(false),
            video_elapsed: Binding::container(Str::from("")),
            video_shared: Rc::new(RefCell::new(None)),
            video_done_rx: Rc::new(RefCell::new(None)),
            video_path: Rc::new(RefCell::new(String::new())),
            highlight_msg: Binding::i64(0),
            twofa_open: Binding::bool(false),
            twofa_old: Binding::<Secure>::default(),
            twofa_new: Binding::<Secure>::default(),
            twofa_hint_in: Binding::container(Str::from("")),
            twofa_email: Binding::container(Str::from("")),
            twofa_note: Binding::container(Str::from("")),
            privacy_picker_open: Binding::bool(false),
            gif_bot: Cell::new(0),
            privacy_target: RefCell::new(None),
            notif_watchers: Rc::new(RefCell::new(Vec::new())),
            notif_private: Binding::bool(true),
            notif_groups: Binding::bool(true),
            notif_channels: Binding::bool(true),
        }
    }

    /// Populate the store with fake data for screenshots/demos on
    /// device-less VMs (`WATERGRAM_DEMO=1`). Never runs on the real path.
    pub fn seed_demo(&self) {
        self.me.set(MeInfo {
            id: 1,
            name: "Lexo".into(),
            phone: "8613800000000".into(),
            username: "lexoliu".into(),
        });
        // Ready carries no label — the sidebar only shows a row while
        // reconnecting, matching the real Update::ConnectionState mapping.
        self.connection.set_from("");
        self.dark.set(true);
        self.screen.set(Screen::Main);
        let mk = |id: i64, title: &str, preview: &str, order: i64, unread: i32, pinned: bool, muted: bool, typing: bool, online: bool, icon: &str| {
            ChatRow {
                id,
                title: Str::from(title.to_string()),
                preview: Str::from(preview.to_string()),
                draft: "".into(),
                order,
                unread,
                pinned,
                muted,
                marked_unread: false,
                in_archive: false,
                photo_file: 0,
                time: Str::from("14:32"),
                typing,
                online,
                kind_icon: Str::from(icon.to_string()),
            }
        };
        // kind_icon mirrors the real path's values (state.rs `kind_icon`
        // mapping): saved/person/group/channel — never an emoji.
        self.chats.set(vec![
            mk(1, "WaterUI devs", "Lexo: preview lands on GpuSurface now", 100, 3, true, false, false, false, "group"),
            mk(2, "Alice", "typing…", 90, 0, false, false, true, true, "person"),
            mk(3, "Saved Messages", "git bundle sha256 a6d3c8…", 80, 0, false, false, false, false, "saved"),
            mk(4, "Telegram News", "Stories are now available for…", 70, 0, false, true, false, false, "channel"),
            mk(5, "Rust China", "anyone tried hydrolysis on wayland?", 60, 12, false, false, false, false, "group"),
            mk(6, "Bob", "see you at the rust meetup", 50, 0, false, false, false, false, "person"),
            mk(7, "dogfood crew", "heap corruption is upstream", 40, 0, false, false, false, false, "group"),
            mk(8, "Mom", "call me when free", 30, 1, false, false, false, false, "person"),
            mk(9, "nokhwa nokhwa", "camera frames stream borrows &Camera", 20, 0, false, false, false, false, "channel"),
            mk(10, "TDLib", "updateAuthorizationState received", 10, 0, false, false, false, false, "person"),
        ]);
        // Demo draft on a visible row (Telegram Desktop shows "Draft: …").
        self.set_draft(3, Some("release notes proofread".into()));
        // Mirrors TDLib `chatFolders`: only user-created folders — the
        // built-in All/Archive lists are synthesized by the sidebar itself.
        self.folders.set(vec![
            FolderRow { id: 2, title: "Work".into(), active: false },
            FolderRow { id: 3, title: "Personal".into(), active: false },
        ]);
        self.set_messages(Self::demo_conversation());
        self.pinned_label.set_from("Alice: shipping it 🚀");
        self.sessions.set(vec![
            SessionRow { id: 1, title: "Watergram · Linux".into(), subtitle: "this device".into(), current: true },
            SessionRow { id: 2, title: "Telegram Desktop · macOS".into(), subtitle: "Shanghai · 2 hours ago".into(), current: false },
        ]);
        self.privacy_rows.set(vec![
            PrivacyRow { setting: "Phone number".into(), audience: "My contacts".into(), key: enums::UserPrivacySetting::ShowPhoneNumber },
            PrivacyRow { setting: "Last seen & online".into(), audience: "Everyone".into(), key: enums::UserPrivacySetting::ShowStatus },
            PrivacyRow { setting: "Profile photos".into(), audience: "Everyone".into(), key: enums::UserPrivacySetting::ShowProfilePhoto },
        ]);
        self.contacts.set(vec![
            MemberRow { key: 11, name: "Alice".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 11 }), username: "alice".into(), photo: 0 },
            MemberRow { key: 12, name: "Bob".into(), status: "last seen recently".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 12 }), username: "bob".into(), photo: 0 },
        ]);
        // Chat 1 (WaterUI devs) is a group — the info panel's member list.
        self.members.set(vec![
            MemberRow { key: 11, name: "Alice".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 11 }), username: "alice".into(), photo: 0 },
            MemberRow { key: 12, name: "Bob".into(), status: "last seen recently".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 12 }), username: "bob".into(), photo: 0 },
            MemberRow { key: 13, name: "Lexo".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 13 }), username: "lexoliu".into(), photo: 0 },
            MemberRow { key: 14, name: "Carol".into(), status: "last seen 1 hour ago".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 14 }), username: "".into(), photo: 0 },
            MemberRow { key: 15, name: "Dan".into(), status: "last seen yesterday".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 15 }), username: "".into(), photo: 0 },
        ]);
        self.members_count.set_from("5 members");
        self.twofa.set_from("enabled");
        self.sticker_packs.set(vec![
            PackRow { id: 1, title: "Hot Cherry".into() },
        ]);
        self.accounts.set(vec![AccountRow { id: 1, label: "Lexo · current".into() }]);
    }

    /// The seeded conversation `seed_demo` installs and `select_chat`
    /// reinstalls for a demo store (`client_id == 0`) — there is no TDLib
    /// history to load, so clicking chats would otherwise empty the pane.
    pub(crate) fn demo_conversation() -> Vec<MessageRow> {
        let styled = styled_from_formatted(&types::FormattedText {
            text: "check https://waterui.dev for the docs".into(),
            entities: vec![types::TextEntity {
                offset: 0,
                length: 5,
                r#type: enums::TextEntityType::Bold,
            }],
        });
        let m = |id: i64, sender: &str, text: &str, time: &str, outgoing: bool, read_out: bool, reply: &str, reactions: &str, fwd: &str, media: &str| MessageRow {
            id,
            sender: Str::from(sender.to_string()),
            text: Str::from(text.to_string()),
            time: Str::from(time.to_string()),
            outgoing,
            read_out,
            can_edit: outgoing,
            reply_excerpt: Str::from(reply.to_string()),
            media_file: 0,
            play_file: 0,
            media_label: Str::from(media.to_string()),
            reactions: Str::from(reactions.to_string()),
            failed: false,
            pending: false,
            highlighted: false,
            unread_divider: false,
            day: 0,
            day_header: false,
            day_label: Str::from(""),
            edited: false,
            my_reaction: Str::from(""),
            styled: StyledStr::empty(),
            webpage: Str::from(""),
            forwarded_from: Str::from(fwd.to_string()),
            poll: None,
        };
        let mut msgs = vec![
            m(10, "Alice", "morning! did the camera filters example work?", "09:41", false, false, "", "", "", ""),
            m(11, "", "yes — device.clone() into Arc, preview straight on the GpuSurface", "09:42", true, true, "morning! did the camera…", "", "", ""),
            m(12, "Alice", "nice. and the NV12 conversion?", "09:42", false, false, "", "👍2 ❤️1", "", ""),
            m(13, "", "compute pass now — only the encoder input buffer ever leaves the GPU", "09:43", true, true, "", "", "", ""),
        ];
        msgs[3].styled = styled;
        // real rows carry plain text alongside the styled body
        msgs[3].text = Str::from("check https://waterui.dev for the docs");
        msgs[3].edited = true;
        msgs.push(m(14, "Alice", "shipping it 🚀", "09:44", false, false, "", "", "", ""));
        msgs.push(m(15, "", "deploying the bundle round 6", "09:45", true, false, "", "", "", ""));
        msgs.push(m(16, "Alice", "📷 photo.jpg", "09:46", false, false, "", "", "", "photo · 182 KB"));
        msgs[2].unread_divider = true;
        msgs.push(m(17, "Alice", "last one from the forwarded channel", "09:47", false, false, "", "", "Telegram News", ""));
        {
            let mut poll_msg = m(18, "Alice", "", "09:48", false, false, "", "", "", "");
            poll_msg.poll = Some(PollRow {
                question: "Ship the r8 bundle today?".into(),
                options: vec![
                    PollOptRow { ix: 0, text: "Yes".into(), pct: 67, chosen: true },
                    PollOptRow { ix: 1, text: "Tomorrow".into(), pct: 33, chosen: false },
                ],
                voters: 3,
                closed: false,
            });
            msgs.push(poll_msg);
        }
        let today = chrono::Local::now().date_naive().num_days_from_ce() as i64;
        for r in &mut msgs {
            r.day = today;
        }
        // First demo message is "yesterday" so the date pill shows.
        if let Some(first) = msgs.first_mut() {
            first.day = today - 1;
        }
        msgs
    }

    /// Kick off the authorization flow once the view is mounted.
    pub fn start(&self) {
        self.restore_accounts();
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::get_authorization_state(client).await;
        })
        .detach();
    }

    /// Register a downloaded-file path and bump the version counter so
    /// `file_signal` derived views re-read the map.
    fn register_file(&self, file: &types::File) {
        let mut changed = {
            let mut files = self.files.borrow_mut();
            let entry = files.entry(file.id).or_default();
            let new_path = if file.local.is_downloading_completed {
                file.local.path.clone()
            } else {
                String::new()
            };
            if *entry != new_path {
                *entry = new_path;
                true
            } else {
                false
            }
        };
        let pct = if file.local.is_downloading_completed {
            100
        } else if file.expected_size > 0 {
            ((file.local.downloaded_size * 100) / file.expected_size)
                .clamp(0, 99) as i32
        } else {
            0
        };
        {
            let mut progress = self.file_progress.borrow_mut();
            let entry = progress.entry(file.id).or_insert(0);
            if *entry != pct {
                *entry = pct;
                changed = true;
            }
        }
        if changed {
            self.files_version.add_assign(1);
        }
    }

    /// Request a download for a TDLib file if it is not local yet.
    pub fn want_file(&self, file: &types::File) {
        self.register_file(file);
        if !file.local.is_downloading_completed
            && !file.local.is_downloading_active
            && file.local.can_be_downloaded
        {
            let (client, id) = (self.client_id.get(), file.id);
            spawn_local(async move {
                let _ = functions::download_file(id, 8, 0, 0, false, client).await;
            })
            .detach();
        }
    }

    /// Same as `want_file` but resolves the `File` object through `getFile`
    /// first — message content only carries raw file ids.
    pub fn want_file_id(&self, file_id: i32) {
        let client = self.client_id.get();
        let files = self.files.clone();
        let version = self.files_version.clone();
        spawn_local(async move {
            if let Ok(enums::File::File(f)) = functions::get_file(file_id, client).await
            {
                let completed = f.local.is_downloading_completed;
                files.borrow_mut().insert(
                    file_id,
                    if completed {
                        f.local.path.clone()
                    } else {
                        String::new()
                    },
                );
                version.add_assign(1);
                if !completed && f.local.can_be_downloaded {
                    let _ =
                        functions::download_file(file_id, 8, 0, 0, false, client).await;
                }
            }
        })
        .detach();
    }

    /// Signal of `file_id -> local path`, re-derived whenever a download
    /// completes. Pass it to `Photo::new`/`when` so avatars and photo
    /// thumbnails update precisely.
    pub fn file_signal(&self, file_id: i32) -> Computed<Str> {
        let files = self.files.clone();
        self.files_version
            .map(move |_| {
                files
                    .borrow()
                    .get(&file_id)
                    .cloned()
                    .unwrap_or_default()
                    .into()
            })
            .computed()
    }

    /// Chat-detail header subtitle: typing overrides everything, groups and
    /// channels show the member count, private chats show presence.
    pub fn chat_subtitle(&self, chat_id: i64) -> Computed<Str> {
        self.chats
            .zip(&self.members_count)
            .map(move |(rows, mc)| {
                rows.iter()
                    .find(|r| r.id == chat_id)
                    .map(|r| {
                        match (
                            r.typing,
                            r.kind_icon.to_string().as_str(),
                            r.online,
                        ) {
                            (true, _, _) => Str::from("typing…"),
                            (false, "group" | "channel", _) => mc.clone(),
                            (false, _, true) => Str::from("online"),
                            _ => Str::from(""),
                        }
                    })
                    .unwrap_or_default()
            })
            .distinct()
            .computed()
    }

    /// Signal of `file_id -> download progress percent` (0-100).
    pub fn file_progress_signal(&self, file_id: i32) -> Computed<i32> {
        let progress = self.file_progress.clone();
        self.files_version
            .map(move |_| progress.borrow().get(&file_id).copied().unwrap_or(0))
            .computed()
    }

    fn user_name(&self, user_id: i64) -> String {
        self.users
            .borrow()
            .get(&user_id)
            .map(|u| format!("{} {}", u.first_name, u.last_name).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "User".to_string())
    }

    fn chat_name(&self, chat_id: i64) -> String {
        self.chat_objs
            .borrow()
            .get(&chat_id)
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "Chat".to_string())
    }

    fn sender_name(&self, sender: &enums::MessageSender) -> Str {
        match sender {
            enums::MessageSender::User(u) => self
                .users
                .borrow()
                .get(&u.user_id)
                .map(|u| format!("{} {}", u.first_name, u.last_name).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "User".to_string())
                .into(),
            enums::MessageSender::Chat(c) => self
                .chat_objs
                .borrow()
                .get(&c.chat_id)
                .map(|c| c.title.clone())
                .unwrap_or_else(|| "Chat".to_string())
                .into(),
        }
    }

#[allow(if_else_view)] // when() requires a signal; conditions here are plain bools
    fn content_preview(content: &enums::MessageContent) -> (Str, i32, Str, i32) {
        // (text/caption, thumbnail/preview file id, media label, playable file id)
        match content {
            enums::MessageContent::MessageText(t) => {
                (t.text.text.clone().into(), 0, Str::from(""), 0)
            }
            enums::MessageContent::MessagePhoto(p) => {
                let file = p
                    .photo
                    .sizes
                    .last()
                    .map(|s| s.photo.id)
                    .unwrap_or_default();
                let caption = p.caption.text.clone();
                (
                    if caption.is_empty() {
                        "Photo".into()
                    } else {
                        caption.into()
                    },
                    file,
                    "Photo".into(),
                    0,
                )
            }
            enums::MessageContent::MessageVideo(v) => (
                format!("Video {}", {
                    let c = v.caption.text.clone();
                    if c.is_empty() {
                        String::new()
                    } else {
                        format!("— {c}")
                    }
                })
                .into(),
                v.video
                    .thumbnail
                    .as_ref()
                    .map(|t| t.file.id)
                    .unwrap_or_default(),
                "Video".into(),
                v.video.video.id,
            ),
            enums::MessageContent::MessageDocument(d) => (
                format!("Document: {}", d.document.file_name).into(),
                d.document
                    .thumbnail
                    .as_ref()
                    .map(|t| t.file.id)
                    .unwrap_or_default(),
                "Document".into(),
                0,
            ),
            enums::MessageContent::MessageAudio(a) => (
                format!("Audio: {}", a.audio.title).into(),
                0,
                "Audio".into(),
                a.audio.audio.id,
            ),
            enums::MessageContent::MessageVoiceNote(v) => {
                (
                    format!("Voice message ({}s)", v.voice_note.duration).into(),
                    0,
                    "Voice".into(),
                    v.voice_note.voice.id,
                )
            }
            enums::MessageContent::MessageVideoNote(v) => {
                (
                    format!("Video note ({}s)", v.video_note.duration).into(),
                    v.video_note
                        .thumbnail
                        .as_ref()
                        .map(|t| t.file.id)
                        .unwrap_or_default(),
                    "Video note".into(),
                    v.video_note.video.id,
                )
            }
            enums::MessageContent::MessageSticker(s) => (
                format!("{} Sticker", s.sticker.emoji).into(),
                0,
                "Sticker".into(),
                0,
            ),
            enums::MessageContent::MessageAnimation(a) => (
                "GIF".into(),
                a.animation
                    .thumbnail
                    .as_ref()
                    .map(|t| t.file.id)
                    .unwrap_or_default(),
                "GIF".into(),
                a.animation.animation.id,
            ),
            enums::MessageContent::MessageLocation(_) => {
                ("Location".into(), 0, "Location".into(), 0)
            }
            enums::MessageContent::MessageContact(c) => (
                format!("Contact: {} {}", c.contact.first_name, c.contact.last_name)
                    .into(),
                0,
                "Contact".into(),
                0,
            ),
            enums::MessageContent::MessagePoll(p) => {
                (format!("Poll: {}", p.poll.question.text).into(), 0, "Poll".into(), 0)
            }
            enums::MessageContent::MessageCall(c) => (
                format!(
                    "Call ({})",
                    if c.is_video { "video" } else { "voice" }
                )
                .into(),
                0,
                "Call".into(),
                0,
            ),
            enums::MessageContent::MessageChatAddMembers(_) => {
                ("New members joined".into(), 0, Str::from(""), 0)
            }
            enums::MessageContent::MessageChatJoinByLink
            | enums::MessageContent::MessageChatJoinByRequest => {
                ("Joined the chat".into(), 0, Str::from(""), 0)
            }
            enums::MessageContent::MessagePinMessage(_) => {
                ("Pinned a message".into(), 0, Str::from(""), 0)
            }
            _ => ("Unsupported message".into(), 0, Str::from(""), 0),
        }
    }

    /// (display string, emoji chosen by the current user)
    fn reactions_info(m: &types::Message) -> (Str, Str) {
        let Some(reactions) = m
            .interaction_info
            .as_ref()
            .and_then(|i| i.reactions.as_ref())
        else {
            return (Str::from(""), Str::from(""));
        };
        let mut mine = String::new();
        let display = reactions
            .reactions
            .iter()
            .map(|mr| {
                let emoji = match &mr.r#type {
                    enums::ReactionType::Emoji(e) => e.emoji.clone(),
                    _ => "★".to_string(),
                };
                if mr.is_chosen {
                    mine = emoji.clone();
                }
                format!("{emoji} {}", mr.total_count)
            })
            .collect::<Vec<_>>()
            .join("  ");
        (display.into(), mine.into())
    }

    /// Build a `MessageRow` from a TDLib message. Runs on the UI thread, so it
    /// may borrow the caches and kick off file downloads.
#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
    pub fn message_row(&self, m: &types::Message) -> MessageRow {
        let (mut text, media_file, media_label, play_file) = Self::content_preview(&m.content);
        if let enums::MessageContent::MessageText(t) = &m.content {
            text = t.text.text.clone().into();
        }
        if media_file != 0 {
            self.want_file_id(media_file);
        }
        if play_file != 0 {
            self.want_file_id(play_file);
        }
        let reply_excerpt = match &m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) => r
                .content
                .as_ref()
                .map(|c| {
                    let (t, _, l, _) = Self::content_preview(c);
                    if t.is_empty() {
                        l
                    } else {
                        t
                    }
                })
                .unwrap_or_else(|| "Reply".into()),
            Some(enums::MessageReplyTo::Story(_)) => "Story".into(),
            None => Str::from(""),
        };
        let (pending, failed) = match &m.sending_state {
            Some(enums::MessageSendingState::Pending(_)) => (true, false),
            Some(enums::MessageSendingState::Failed(_)) => (false, true),
            _ => (false, false),
        };
        let read_out = m.is_outgoing
            && self
                .chat_objs
                .borrow()
                .get(&m.chat_id)
                .map(|c| m.id <= c.last_read_outbox_message_id)
                .unwrap_or(false);
        let (reactions, my_reaction) = Self::reactions_info(m);
        let (styled, webpage) = if let enums::MessageContent::MessageText(t) = &m.content {
            (
                styled_from_formatted(&t.text),
                t.link_preview
                    .as_ref()
                    .map(|p| {
                        let desc = p.description.text.trim();
                        if desc.is_empty() {
                            format!("{} — {}", p.site_name, p.title)
                        } else {
                            format!("{} — {} · {}", p.site_name, p.title, desc)
                        }
                    })
                    .unwrap_or_default()
                    .into(),
            )
        } else {
            (StyledStr::empty(), Str::from(""))
        };
        let forwarded_from = m
            .forward_info
            .as_ref()
            .map(|f| match &f.origin {
                enums::MessageOrigin::User(u) => format!(
                    "Forwarded from {}",
                    self.user_name(u.sender_user_id)
                )
                .into(),
                enums::MessageOrigin::HiddenUser(h) => {
                    format!("Forwarded from {}", h.sender_name).into()
                }
                enums::MessageOrigin::Chat(c) => {
                    format!("Forwarded from {}", self.chat_name(c.sender_chat_id))
                        .into()
                }
                enums::MessageOrigin::Channel(c) => {
                    format!("Forwarded from {}", self.chat_name(c.chat_id))
                        .into()
                }
            })
            .unwrap_or_else(|| Str::from(""));
        let poll = match &m.content {
            enums::MessageContent::MessagePoll(mp) => {
                let p = &mp.poll;
                Some(PollRow {
                    question: p.question.text.clone().into(),
                    options: p
                        .options
                        .iter()
                        .enumerate()
                        .map(|(ix, o)| PollOptRow {
                            ix,
                            text: o.text.text.clone().into(),
                            pct: o.vote_percentage,
                            chosen: o.is_chosen,
                        })
                        .collect(),
                    voters: p.total_voter_count,
                    closed: p.is_closed,
                })
            }
            _ => None,
        };
        MessageRow {
            id: m.id,
            sender: self.sender_name(&m.sender_id),
            text,
            time: fmt_time(m.date),
            outgoing: m.is_outgoing,
            read_out,
            styled,
            webpage,
            forwarded_from,
            can_edit: m.is_outgoing,
            reply_excerpt,
            media_file,
            play_file,
            media_label,
            reactions,
            my_reaction,
            failed,
            pending,
            highlighted: false,
            unread_divider: false,
            day: local_day(m.date),
            day_header: false,
            day_label: Str::from(""),
            edited: m.edit_date != 0,
            poll,
        }
    }

#[allow(if_else_view)] // when() requires a signal; conditions here are plain bools
    fn preview_text(&self, m: &types::Message) -> Str {
        let (t, _, label, _) = Self::content_preview(&m.content);
        if matches!(m.content, enums::MessageContent::MessageText(_)) {
            t
        } else if t.is_empty() {
            format!("[{label}]").into()
        } else {
            format!("[{label}] {t}").into()
        }
    }

    fn upsert_chat(&self, chat: types::Chat) {
        let pos = position_in(&chat, &self.active_list()).cloned();
        self.chat_objs.borrow_mut().insert(chat.id, chat.clone());
        if let Some(photo) = &chat.photo {
            self.want_file(&photo.small);
        }
        let (order, pinned) = pos
            .as_ref()
            .map(|p| (p.order, p.is_pinned))
            .unwrap_or((0, false));
        let muted = !chat.notification_settings.use_default_mute_for
            && chat.notification_settings.mute_for > 0;
        let online = match &chat.r#type {
            enums::ChatType::Private(p) => self
                .users
                .borrow()
                .get(&p.user_id)
                .map(|u| matches!(u.status, enums::UserStatus::Online(_)))
                .unwrap_or(false),
            _ => false,
        };
        let kind_icon = match &chat.r#type {
            enums::ChatType::Private(_) | enums::ChatType::Secret(_) => {
                if chat.id == self.my_id.get() {
                    "saved"
                } else {
                    "person"
                }
            }
            enums::ChatType::BasicGroup(_) => "group",
            enums::ChatType::Supergroup(s) => {
                if s.is_channel {
                    "channel"
                } else {
                    "group"
                }
            }
        };
        let preview = chat
            .last_message
            .as_ref()
            .map(|m| self.preview_text(m))
            .unwrap_or_default();
        let time = chat
            .last_message
            .as_ref()
            .map(|m| fmt_time(m.date))
            .unwrap_or_default();
        let row = ChatRow {
            id: chat.id,
            title: chat.title.clone().into(),
            preview,
            draft: self
                .drafts
                .borrow()
                .get(&chat.id)
                .cloned()
                .unwrap_or_default(),
            order,
            unread: chat.unread_count,
            pinned,
            muted,
            marked_unread: chat.is_marked_as_unread,
            in_archive: position_in(&chat, &enums::ChatList::Archive).is_some(),
            photo_file: chat.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
            time,
            typing: false,
            online,
            kind_icon: kind_icon.into(),
        };
        let mut list = self.chats.get();
        match list.iter().position(|r| r.id == chat.id) {
            Some(i) => list[i] = row,
            None => {
                if order == 0 {
                    // A chat we have no main-list position for does not belong
                    // in the visible list.
                    return;
                }
                list.push(row)
            }
        }
        list.sort();
        self.chats.set(list);
    }

    fn update_chat_row(&self, chat_id: i64, f: impl Fn(&mut ChatRow)) {
        let mut list = self.chats.get();
        if let Some(r) = list.iter_mut().find(|r| r.id == chat_id) {
            f(r);
            list.sort();
            self.chats.set(list);
        }
    }

    fn update_message_row(&self, message_id: i64, f: impl Fn(&mut MessageRow)) {
        let mut list = self.messages.get();
        if let Some(r) = list.iter_mut().find(|r| r.id == message_id) {
            f(r);
            self.set_messages(list);
        }
    }

    fn patch_positions(&self, chat_id: i64, positions: Vec<types::ChatPosition>) {
        if let Some(c) = self.chat_objs.borrow_mut().get_mut(&chat_id) {
            for p in &positions {
                c.positions
                    .retain(|e| e.list != p.list);
                c.positions.push(p.clone());
            }
        }
        let active = self.active_list();
        if let Some(p) = positions.iter().find(|p| p.list == active).cloned() {
            let (order, pinned) = (p.order, p.is_pinned);
            if order == 0 {
                let mut list = self.chats.get();
                list.retain(|r| r.id != chat_id);
                self.chats.set(list);
            } else {
                self.update_chat_row(chat_id, |r| {
                    r.order = order;
                    r.pinned = pinned;
                });
            }
        }
    }

    /// The one big dispatch — runs on the UI executor for every Update.
#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
    pub fn update(&self, u: enums::Update, client: i32) {
        if client != self.client_id.get() {
            // Updates belonging to a background (non-active) account are
            // dropped; switching accounts re-queries that client's state.
            return;
        }
        match u {
            enums::Update::AuthorizationState(s) => {
                self.on_auth_state(s.authorization_state)
            }
            enums::Update::User(u) => {
                self.users.borrow_mut().insert(u.user.id, u.user);
            }
            enums::Update::UserStatus(u) => {
                if let Some(user) = self.users.borrow_mut().get_mut(&u.user_id) {
                    user.status = u.status.clone();
                }
                let online = matches!(u.status, enums::UserStatus::Online(_));
                let target = self
                    .chat_objs
                    .borrow()
                    .iter()
                    .find(|(_, c)| {
                        matches!(&c.r#type, enums::ChatType::Private(p) if p.user_id == u.user_id)
                    })
                    .map(|(id, _)| *id);
                if let Some(id) = target {
                    self.update_chat_row(id, |r| r.online = online);
                }
            }
            enums::Update::NewChat(c) => self.upsert_chat(c.chat),
            enums::Update::ChatFolders(u) => {
                self.folders.set(
                    u.chat_folders
                        .iter()
                        .map(|f| FolderRow {
                            id: f.id,
                            title: f.name.text.text.clone().into(),
                            active: false,
                        })
                        .collect(),
                );
            }
            enums::Update::ChatTitle(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.title = u.title.clone();
                }
                let t = u.title.clone();
                self.update_chat_row(u.chat_id, |r| r.title = t.clone().into());
            }
            enums::Update::ChatPhoto(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.photo = u.photo.clone();
                }
                if let Some(p) = &u.photo {
                    self.want_file(&p.small);
                }
                let fid = u.photo.map(|p| p.small.id).unwrap_or(0);
                self.update_chat_row(u.chat_id, |r| r.photo_file = fid);
            }
            enums::Update::ChatLastMessage(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.last_message = u.last_message.clone();
                }
                self.patch_positions(u.chat_id, u.positions);
                let preview = u
                    .last_message
                    .as_ref()
                    .map(|m| self.preview_text(m))
                    .unwrap_or_default();
                let time = u
                    .last_message
                    .as_ref()
                    .map(|m| fmt_time(m.date))
                    .unwrap_or_default();
                self.update_chat_row(u.chat_id, |r| {
                    r.preview = preview.clone();
                    r.time = time.clone();
                });
            }
            enums::Update::ChatPosition(u) => {
                self.patch_positions(u.chat_id, vec![u.position]);
            }
            enums::Update::ChatReadInbox(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.last_read_inbox_message_id = u.last_read_inbox_message_id;
                }
                self.update_chat_row(u.chat_id, |r| r.unread = u.unread_count);
                if u.chat_id == self.open_chat.get() {
                    self.apply_unread_divider(u.chat_id);
                }
            }
            enums::Update::ChatReadOutbox(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.last_read_outbox_message_id = u.last_read_outbox_message_id;
                }
                if u.chat_id == self.open_chat.get() {
                    let mut list = self.messages.get();
                    for r in list.iter_mut() {
                        if r.outgoing && r.id <= u.last_read_outbox_message_id {
                            r.pending = false;
                            r.read_out = true;
                        }
                    }
                    self.set_messages(list);
                }
            }
            enums::Update::ChatIsMarkedAsUnread(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.is_marked_as_unread = u.is_marked_as_unread;
                }
                self.update_chat_row(u.chat_id, |r| {
                    r.marked_unread = u.is_marked_as_unread
                });
            }
            enums::Update::MessageInteractionInfo(u) => {
                if u.chat_id == self.open_chat.get() {
                    let (display, mine) = u
                        .interaction_info
                        .as_ref()
                        .and_then(|i| i.reactions.as_ref())
                        .map(|reactions| {
                            let mut mine = String::new();
                            let display = reactions
                                .reactions
                                .iter()
                                .map(|mr| {
                                    let emoji = match &mr.r#type {
                                        enums::ReactionType::Emoji(e) => e.emoji.clone(),
                                        _ => "★".to_string(),
                                    };
                                    if mr.is_chosen {
                                        mine = emoji.clone();
                                    }
                                    format!("{emoji} {}", mr.total_count)
                                })
                                .collect::<Vec<_>>()
                                .join("  ");
                            (display, mine)
                        })
                        .unwrap_or_default();
                    self.update_message_row(u.message_id, |r| {
                        r.reactions = display.clone().into();
                        r.my_reaction = mine.clone().into();
                    });
                }
            }
            enums::Update::ChatNotificationSettings(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.notification_settings = u.notification_settings.clone();
                }
                let muted = !u.notification_settings.use_default_mute_for
                    && u.notification_settings.mute_for > 0;
                self.update_chat_row(u.chat_id, |r| r.muted = muted);
            }
            enums::Update::ChatAction(u) => {
                let typing = matches!(
                    u.action,
                    enums::ChatAction::Typing
                        | enums::ChatAction::RecordingVoiceNote
                        | enums::ChatAction::RecordingVideo
                        | enums::ChatAction::UploadingPhoto(_)
                        | enums::ChatAction::UploadingVideo(_)
                        | enums::ChatAction::RecordingVideoNote
                );
                let name = self.sender_name(&u.sender_id);
                let chat_id = u.chat_id;
                self.update_chat_row(chat_id, |r| {
                    r.typing = typing;
                    r.preview = if typing {
                        format!("{name} is typing…").into()
                    } else {
                        self.chat_objs
                            .borrow()
                            .get(&chat_id)
                            .and_then(|c| c.last_message.as_ref())
                            .map(|m| self.preview_text(m))
                            .unwrap_or_default()
                    };
                });
                if typing {
                    let seq = {
                        let mut g = self.typing_gen.borrow_mut();
                        let e = g.entry(chat_id).or_insert(0);
                        *e += 1;
                        *e
                    };
                    let store = self.clone();
                    spawn_local(async move {
                        sleep(std::time::Duration::from_secs(6)).await;
                        if store.typing_gen.borrow().get(&chat_id) == Some(&seq) {
                            store.update_chat_row(chat_id, |r| {
                                r.typing = false;
                                r.preview = store
                                    .chat_objs
                                    .borrow()
                                    .get(&chat_id)
                                    .and_then(|c| c.last_message.as_ref())
                                    .map(|m| store.preview_text(m))
                                    .unwrap_or_default();
                            });
                        }
                    })
                    .detach();
                }
            }
            enums::Update::NewMessage(u) => {
                let m = u.message;
                if m.chat_id == self.open_chat.get() {
                    let row = self.message_row(&m);
                    let mut list = self.messages.get();
                    if !list.iter().any(|r| r.id == row.id) {
                        list.push(row);
                        list.sort();
                        self.set_messages(list);
                        self.scroll_bottom();
                    }
                    if !m.is_outgoing {
                        let (client, chat_id, mid) = (self.client_id.get(), m.chat_id, m.id);
                        spawn_local(async move {
                            let _ = functions::view_messages(
                                chat_id,
                                vec![mid],
                                None,
                                false,
                                client,
                            )
                            .await;
                        })
                        .detach();
                    }
                }
            }
            enums::Update::MessageSendSucceeded(u) => {
                if u.message.chat_id == self.open_chat.get() {
                    let mut list = self.messages.get();
                    list.retain(|r| r.id != u.old_message_id && r.id != u.message.id);
                    list.push(self.message_row(&u.message));
                    list.sort();
                    self.set_messages(list);
                }
            }
            enums::Update::MessageSendFailed(u) => {
                if u.message.chat_id == self.open_chat.get() {
                    self.update_message_row(u.old_message_id, |r| {
                        r.failed = true;
                        r.pending = false;
                    });
                }
            }
            enums::Update::MessageContent(u) => {
                if u.chat_id == self.open_chat.get() {
                    let (text, media, label, play) = Self::content_preview(&u.new_content);
                    self.update_message_row(u.message_id, |r| {
                        r.text = text.clone();
                        r.media_file = media;
                        r.play_file = play;
                        r.media_label = label.clone();
                    });
                }
            }
            enums::Update::MessageEdited(u) => {
                if u.chat_id == self.open_chat.get() {
                    let client = self.client_id.get();
                    let store = self.clone();
                    spawn_local(async move {
                        if let Ok(enums::Message::Message(m)) =
                            functions::get_message(u.chat_id, u.message_id, client)
                                .await
                        {
                            let row = store.message_row(&m);
                            store.update_message_row(u.message_id, |r| *r = row.clone());
                        }
                    })
                    .detach();
                }
            }
            enums::Update::DeleteMessages(u) => {
                if u.chat_id == self.open_chat.get() {
                    let mut list = self.messages.get();
                    list.retain(|r| !u.message_ids.contains(&r.id));
                    self.set_messages(list);
                }
            }
            enums::Update::ChatDraftMessage(u) => {
                self.patch_positions(u.chat_id, u.positions);
                let text = match u.draft_message.as_ref() {
                    Some(d) => match &d.input_message_text {
                        enums::InputMessageContent::InputMessageText(t) => {
                            t.text.text.clone()
                        }
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                if u.chat_id == self.open_chat.get() {
                    self.composer.set_from(text.clone());
                }
                self.set_draft(u.chat_id, Some(text.into()));
            }
            enums::Update::File(f) => {
                self.register_file(&f.file);
            }
            enums::Update::Option(o) => {
                if o.name == "my_id"
                    && let enums::OptionValue::Integer(v) = &o.value
                {
                    self.my_id.set(v.value);
                }
                if o.name == "language_pack_id"
                    && let enums::OptionValue::String(v) = &o.value
                {
                    self.lang_id.set_from(v.value.clone());
                }
            }
            enums::Update::LanguagePackStrings(u) => {
                self.merge_language_strings(&u);
            }
            enums::Update::ConnectionState(s) => {
                let label = match s.state {
                    enums::ConnectionState::Ready => "",
                    enums::ConnectionState::Updating => "Updating…",
                    enums::ConnectionState::Connecting
                    | enums::ConnectionState::ConnectingToProxy => "Connecting…",
                    enums::ConnectionState::WaitingForNetwork => "Waiting for network…",
                };
                self.connection.set_from(label);
            }
            _ => {}
        }
    }

    fn on_auth_state(&self, state: enums::AuthorizationState) {
        use enums::AuthorizationState as A;
        match state {
            A::WaitTdlibParameters => {
                if let Some(cfg) = Config::load() {
                    self.set_tdlib_parameters(cfg);
                } else {
                    if let Ok(id) = std::env::var("WATERGRAM_API_ID") {
                        self.api_id.set_from(id);
                    }
                    if let Ok(h) = std::env::var("WATERGRAM_API_HASH") {
                        self.api_hash.set_from(h);
                    }
                    self.screen.set(Screen::ApiKeys);
                }
            }
            A::WaitPhoneNumber => self.screen.set(Screen::Phone),
            A::WaitCode(_) => {
                self.busy.set(false);
                self.screen.set(Screen::Code);
            }
            A::WaitPassword(p) => {
                self.busy.set(false);
                self.password_hint.set_from(p.password_hint);
                self.screen.set(Screen::Password);
            }
            A::WaitRegistration(_) => {
                self.busy.set(false);
                self.screen.set(Screen::Register);
            }
            A::WaitOtherDeviceConfirmation(c) => {
                self.busy.set(false);
                self.qr_link.set_from(c.link);
                self.screen.set(Screen::Qr);
            }
            A::WaitEmailAddress(_) | A::WaitEmailCode(_) => {
                self.auth_note.set_from("This account requires email verification, which Watergram does not support yet.");
            }
            A::Ready => {
                self.busy.set(false);
                self.screen.set(Screen::Main);
                self.after_login();
                self.refresh_account_label();
            }
            A::LoggingOut | A::Closing => {
                self.busy.set(true);
            }
            A::Closed => {
                self.reset();
                self.screen.set(Screen::Phone);
            }
            _ => {}
        }
    }

    fn set_tdlib_parameters(&self, cfg: Config) {
        let client = self.client_id.get();
        let db_root = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram");
        let db = db_root
            .join(format!("db_{}", self.client_id.get()))
            .to_string_lossy()
            .to_string();
        let files = db_root
            .join(format!("files_{}", self.client_id.get()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::create_dir_all(&db_root);
        self.screen.set(Screen::Loading);
        let store = self.clone();
        spawn_local(async move {
            if let Err(e) = functions::set_tdlib_parameters(
                cfg.test_dc,
                db,
                files,
                String::new(),
                true,
                true,
                true,
                false,
                cfg.api_id,
                cfg.api_hash,
                "en".to_string(),
                "Watergram".to_string(),
                "desktop".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
                client,
            )
            .await
            {
                error!("setTdlibParameters failed: {e:?}");
                store
                    .auth_note
                    .set_from(format!("{}: {}", e.code, e.message));
                store.busy.set(false);
                store.screen.set(Screen::ApiKeys);
            }
        })
        .detach();
    }

    fn after_login(&self) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::User::User(u)) = functions::get_me(client).await {
                let name = format!("{} {}", u.first_name, u.last_name)
                    .trim()
                    .to_string();
                let username = u
                    .usernames
                    .as_ref()
                    .and_then(|us| us.active_usernames.first().cloned())
                    .map(|n| format!("@{n}"))
                    .unwrap_or_default();
                store.users.borrow_mut().insert(u.id, u.clone());
                store.my_id.set(u.id);
                store.me.set(MeInfo {
                    id: u.id,
                    name: name.into(),
                    phone: u.phone_number.into(),
                    username: username.into(),
                });
            }
            let _ = functions::load_chats(Some(enums::ChatList::Main), 200, client).await;
        })
        .detach();
        self.load_language_packs();
    }

    fn reset(&self) {
        self.chats.set(Vec::new());
        self.set_messages(Vec::new());
        self.server_results.set(Vec::new());
        self.chat_objs.borrow_mut().clear();
        self.users.borrow_mut().clear();
        self.files.borrow_mut().clear();
        self.drafts.borrow_mut().clear();
        self.open_chat.set(0);
        self.selected.set(None);
        self.syncing_selection.set(true);
        self.list_selection.set(None);
        self.syncing_selection.set(false);
        self.me.set(MeInfo::default());
        self.busy.set(false);
    }

    // ----- actions called from views -----

    pub fn submit_api_keys(&self) {
        let id: i32 = self.api_id.get().trim().parse().unwrap_or(0);
        let hash = self.api_hash.get().to_string();
        if id <= 0 || hash.is_empty() {
            self.auth_note.set_from("Enter a valid api_id and api_hash (from my.telegram.org).");
            return;
        }
        let cfg = Config {
            api_id: id,
            api_hash: hash,
            test_dc: self.test_dc.get(),
        };
        cfg.save();
        self.set_tdlib_parameters(cfg);
    }

    pub fn submit_phone(&self) {
        let phone = self.phone.get().to_string();
        if phone.trim().is_empty() {
            self.auth_note.set_from("Enter your phone number.");
            return;
        }
        self.busy.set(true);
        self.auth_note.set_from("");
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            match functions::set_authentication_phone_number(phone, None, client)
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    store.busy.set(false);
                    store
                        .auth_note
                        .set_from(format!("{}: {}", e.code, e.message));
                }
            }
        })
        .detach();
    }

    pub fn request_qr(&self) {
        self.busy.set(true);
        self.auth_note.set_from("");
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Err(e) =
                functions::request_qr_code_authentication(Vec::new(), client).await
            {
                store.busy.set(false);
                store
                    .auth_note
                    .set_from(format!("{}: {}", e.code, e.message));
            }
        })
        .detach();
    }

    pub fn submit_code(&self) {
        let code = self.code.get().to_string();
        if code.trim().is_empty() {
            return;
        }
        self.busy.set(true);
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            match functions::check_authentication_code(code, client).await {
                Ok(()) => {}
                Err(e) => {
                    store.busy.set(false);
                    store
                        .auth_note
                        .set_from(format!("{}: {}", e.code, e.message));
                }
            }
        })
        .detach();
    }

    pub fn submit_password(&self) {
        let pw = self.password.get().expose().to_string();
        if pw.is_empty() {
            return;
        }
        self.busy.set(true);
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            match functions::check_authentication_password(pw, client).await {
                Ok(()) => {}
                Err(e) => {
                    store.busy.set(false);
                    store
                        .auth_note
                        .set_from(format!("{}: {}", e.code, e.message));
                }
            }
        })
        .detach();
    }

    pub fn register(&self) {
        let first = self.first_name.get().to_string();
        if first.trim().is_empty() {
            self.auth_note.set_from("First name is required.");
            return;
        }
        let last = self.last_name.get().to_string();
        self.busy.set(true);
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Err(e) = functions::register_user(first, last, false, client).await {
                store.busy.set(false);
                store
                    .auth_note
                    .set_from(format!("{}: {}", e.code, e.message));
            }
        })
        .detach();
    }

    pub fn logout(&self) {
        self.busy.set(true);
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::log_out(client).await;
        })
        .detach();
    }

    pub fn select_chat(&self, chat_id: i64) {
        if self.syncing_selection.get() {
            return;
        }
        // Forwarding mode: a pending forwarded message (or a selected
        // batch) routes the tap to forwardMessages instead of opening the
        // chat. `send_copy` forwards without author attribution.
        let batch = self.forward_ids.get();
        let single = self.forward_message.get();
        if !batch.is_empty() || single.is_some() {
            let (from, msg_ids) = if !batch.is_empty() {
                (self.open_chat.get(), batch)
            } else {
                let (from, msg_id) = single.unwrap_or_default();
                (from, vec![msg_id])
            };
            let send_copy = self.forward_noattr.get();
            let comment = self.forward_comment.get();
            self.forward_message.set(None);
            self.forward_ids.set(Vec::new());
            self.forward_noattr.set(false);
            self.forward_comment.set_from("");
            let client = self.client_id.get();
            spawn_local(async move {
                let _ = functions::forward_messages(
                    chat_id, None, from, msg_ids, None, send_copy, false, client,
                )
                .await;
                if !comment.is_empty() {
                    let _ = functions::send_message(
                        chat_id,
                        None,
                        None,
                        None,
                        enums::InputMessageContent::InputMessageText(
                            types::InputMessageText {
                                text: types::FormattedText {
                                    text: comment.into(),
                                    entities: Vec::new(),
                                },
                                link_preview_options: None,
                                clear_draft: false,
                            },
                        ),
                        client,
                    )
                    .await;
                }
            })
            .detach();
            // The tap already highlighted the row — restore the highlight to
            // the still-open chat (forwarding does not open a chat).
            self.syncing_selection.set(true);
            self.list_selection.set(self.selected.get());
            self.syncing_selection.set(false);
            return;
        }
        if self.open_chat.get() == chat_id {
            self.selected.set(Some(chat_id));
            self.syncing_selection.set(true);
            self.list_selection.set(Some(chat_id));
            self.syncing_selection.set(false);
            return;
        }
        let prev = self.open_chat.replace(chat_id);
        self.selected.set(Some(chat_id));
        self.syncing_selection.set(true);
        self.list_selection.set(Some(chat_id));
        self.syncing_selection.set(false);
        self.set_messages(Vec::new());
        self.reply_to.set(None);
        self.editing.set(None);
        self.no_more_history.set(false);
        self.loading_history.set(false);
        self.oldest_message.set(0);
        self.pinned_label.set_from("");
        self.pinned_id.set(0);
        self.chat_search_open.set(false);
        self.chat_search.set_from("");
        self.chat_search_results.set(Vec::new());
        self.members.set(Vec::new());
        if prev != 0 {
            let cur = self.composer.get();
            self.set_draft(prev, if cur.is_empty() { None } else { Some(cur.clone()) });
            self.sync_draft(prev, cur);
        }
        if let Some(d) = self.drafts.borrow().get(&chat_id) {
            self.composer.set(d.clone());
        } else {
            self.composer.set_from("");
        }
        self.members_open.set(false);
        self.scheduled_open.set(false);
        self.info_open.set(false);
        self.poll_open.set(false);
        self.shared_media.set(Vec::new());
        self.attach_caption.set_from("");
        self.selected_msgs.set(Vec::new());
        self.viewer.set(None);
        if self.client_id.get() == 0 {
            // Unit-test/demo store: no TDLib — reinstall the seeded
            // conversation so clicking chats in the demo isn't an empty pane.
            self.set_messages(Self::demo_conversation());
            self.scroll_bottom();
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if prev != 0 {
                let _ = functions::close_chat(prev, client).await;
            }
            let _ = functions::open_chat(chat_id, client).await;
            store.load_history(chat_id, 0, 0).await;
            store.refresh_pinned(chat_id).await;
            store.apply_unread_divider(chat_id);
        })
        .detach();
    }

    /// Toggle the right-side info panel and (re)load shared media.
    pub fn toggle_info(&self) {
        let open = !self.info_open.get();
        self.info_open.set(open);
        if open {
            self.load_shared_media();
            let kind = self
                .chats
                .get()
                .iter()
                .find(|r| r.id == self.open_chat.get())
                .map(|r| r.kind_icon.to_string())
                .unwrap_or_default();
            if matches!(kind.as_str(), "group" | "channel") {
                self.load_members();
            }
        }
    }

    /// Shared media grid for the info panel: `searchChatMessages` with
    /// the PhotoAndVideo filter; photo small-file ids go through
    /// `want_file_id` so thumbs download and show via `file_signal`.
    pub fn load_shared_media(&self) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if self.client_id.get() == 0 {
            // Demo store: seed a grid so the panel is exercisable.
            self.shared_media.set(
                ["🖼️", "🎬", "🖼️", "📹", "🖼️", "🎞️", "🖼️", "📷", "🖼️"]
                    .iter()
                    .enumerate()
                    .map(|(i, e)| SharedMediaRow {
                        file: -(i as i32) - 1,
                        label: Str::from(*e),
                    })
                    .collect(),
            );
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::FoundChatMessages::FoundChatMessages(found)) =
                functions::search_chat_messages(
                    chat_id,
                    None,
                    String::new(),
                    None,
                    0,
                    0,
                    21,
                    Some(enums::SearchMessagesFilter::PhotoAndVideo),
                    client,
                )
                .await
            {
                let mut rows = Vec::new();
                for m in found.messages.iter() {
                    let (file, label) = match &m.content {
                        enums::MessageContent::MessagePhoto(p) => {
                            let fid = p
                                .photo
                                .sizes
                                .first()
                                .map(|s| s.photo.id)
                                .unwrap_or(0);
                            (fid, "🖼️".to_string())
                        }
                        enums::MessageContent::MessageVideo(v) => {
                            (v.video.thumbnail.as_ref().map(|t| t.file.id).unwrap_or(0),
                             "🎬".to_string())
                        }
                        enums::MessageContent::MessageAnimation(a) => {
                            (a.animation.thumbnail.as_ref().map(|t| t.file.id).unwrap_or(0),
                             "🎞️".to_string())
                        }
                        _ => (0, String::new()),
                    };
                    if file != 0 {
                        store.want_file_id(file);
                        rows.push(SharedMediaRow { file, label: label.into() });
                    }
                }
                store.shared_media.set(rows);
            }
        })
        .detach();
    }

    /// Flag the first incoming message past `last_read_inbox` so the row
    /// renders the "Unread messages" divider (Desktop parity).
    pub fn apply_unread_divider(&self, chat_id: i64) {
        let lri = self
            .chat_objs
            .borrow()
            .get(&chat_id)
            .map(|c| c.last_read_inbox_message_id)
            .unwrap_or(0);
        let unread = self
            .chats
            .get()
            .iter()
            .find(|r| r.id == chat_id)
            .map(|r| r.unread)
            .unwrap_or(0);
        let mut list = self.messages.get();
        let target = if unread > 0 {
            list.iter()
                .filter(|r| !r.outgoing)
                .filter(|r| r.id > lri)
                .map(|r| r.id)
                .min()
        } else {
            None
        };
        let mut changed = false;
        for r in list.iter_mut() {
            let want = Some(r.id) == target;
            if r.unread_divider != want {
                r.unread_divider = want;
                changed = true;
            }
        }
        if changed {
            self.set_messages(list);
        }
    }

    /// Sets `messages` after marking the first row of each distinct local
    /// day with `day_header` + `day_label` (the date pill).
    pub fn set_messages(&self, mut rows: Vec<MessageRow>) {
        let mut prev_day = 0i64;
        for r in &mut rows {
            r.day_header = r.day != prev_day;
            if r.day_header {
                r.day_label = fmt_day_label(r.day);
            }
            prev_day = r.day;
        }
        self.messages.set(rows);
    }

    /// Fetch the chat's pinned message into `pinned_label`/`pinned_id`.
    #[allow(if_else_view)] // string pick, not a view
    async fn refresh_pinned(&self, chat_id: i64) {
        match functions::get_chat_pinned_message(chat_id, self.client_id.get()).await {
            Ok(enums::Message::Message(m)) => {
                let (t, _, label, _) = Self::content_preview(&m.content);
                self.pinned_id.set(m.id);
                self.pinned_label
                    .set(if t.is_empty() { label } else { t });
            }
            _ => {
                self.pinned_id.set(0);
                self.pinned_label.set_from("");
            }
        }
    }

    pub fn pin_message(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if functions::pin_chat_message(chat_id, message_id, false, false, client)
                .await
                .is_ok()
            {
                store.refresh_pinned(chat_id).await;
            }
        })
        .detach();
    }

    pub fn unpin_message(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if functions::unpin_chat_message(chat_id, message_id, client)
                .await
                .is_ok()
            {
                store.refresh_pinned(chat_id).await;
            }
        })
        .detach();
    }

    /// Quick-react to a message: removes the reaction when the same emoji was
    /// already chosen, otherwise adds it.
    pub fn toggle_reaction(&self, row: &MessageRow, emoji: &str) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let (client, mid) = (self.client_id.get(), row.id);
        let emoji = emoji.to_string();
        let chosen = row.my_reaction.as_str() == emoji;
        let rt = enums::ReactionType::Emoji(types::ReactionTypeEmoji { emoji });
        spawn_local(async move {
            if chosen {
                let _ = functions::remove_message_reaction(chat_id, mid, rt, client).await;
            } else {
                let _ = functions::add_message_reaction(
                    chat_id, mid, rt, false, true, client,
                )
                .await;
            }
        })
        .detach();
    }

    /// Mark a chat unread/read manually.
    pub fn toggle_mark_unread(&self, chat_id: i64) {
        let current = self
            .chat_objs
            .borrow()
            .get(&chat_id)
            .map(|c| c.is_marked_as_unread)
            .unwrap_or(false);
        let client = self.client_id.get();
        spawn_local(async move {
            let _ =
                functions::toggle_chat_is_marked_as_unread(chat_id, !current, client)
                    .await;
        })
        .detach();
    }

    /// In-chat message search via `searchChatMessages`.
    pub fn run_chat_search(&self, query: Str) {
        self.chat_search.set(query.clone());
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if query.is_empty() {
            self.chat_search_results.set(Vec::new());
        self.members.set(Vec::new());
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::FoundChatMessages::FoundChatMessages(found)) =
                functions::search_chat_messages(
                    chat_id,
                    None,
                    query.to_string(),
                    None,
                    0,
                    0,
                    20,
                    None,
                    client,
                )
                .await
            {
                let mut rows: Vec<MessageRow> = found
                    .messages
                    .iter()
                    .map(|m| store.message_row(m))
                    .collect();
                rows.sort();
                store.chat_search_results.set(rows);
            }
        })
        .detach();
    }

    /// Scroll/jump to a message: loads a window of history around it, then
    /// drives `List`'s `ScrollController<usize>` to the exact row index and
    /// flash-highlights the bubble.
    pub fn jump_to_message(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Messages::Messages(msgs)) = functions::get_chat_history(
                chat_id,
                message_id,
                -20,
                40,
                false,
                store.client_id.get(),
            )
            .await
            {
                let mut list = store.messages.get();
                for m in msgs.messages.iter().flatten() {
                    let row = store.message_row(m);
                    if !list.iter().any(|r| r.id == row.id) {
                        list.push(row);
                    }
                }
                list.sort();
                let pos = list.iter().position(|r| r.id == message_id);
                for (i, r) in list.iter_mut().enumerate() {
                    r.highlighted = Some(i) == pos;
                }
                store.set_messages(list);
                store.highlight_msg.set(message_id);
                if let Some(idx) = pos {
                    store.scroll.scroll_to(idx);
                }
            }
        })
        .detach();
    }

    /// Sync the composer's draft for `chat_id` to the server so it follows
    /// the account across devices (`setChatDraftMessage`; `None` clears).
    /// Keep `drafts` and the chat-row `draft` label in sync at every site
    /// that creates or clears a draft (send, edit, switching chats, server
    /// `updateDraftMessage`).
    fn set_draft(&self, chat_id: i64, draft: Option<Str>) {
        match &draft {
            Some(d) => {
                self.drafts.borrow_mut().insert(chat_id, d.clone());
            }
            None => {
                self.drafts.borrow_mut().remove(&chat_id);
            }
        }
        let text = draft.unwrap_or_default();
        self.update_chat_row(chat_id, |r| r.draft = text.clone());
    }

    fn sync_draft(&self, chat_id: i64, cur: Str) {
        if self.client_id.get() == 0 {
            return;
        }
        let draft = if cur.is_empty() {
            None
        } else {
            Some(types::DraftMessage {
                reply_to: None,
                date: 0,
                input_message_text: enums::InputMessageContent::InputMessageText(
                    types::InputMessageText {
                        text: types::FormattedText {
                            text: cur.to_string(),
                            entities: Vec::new(),
                        },
                        link_preview_options: None,
                        clear_draft: false,
                    },
                ),
                effect_id: 0,
                suggested_post_info: None,
            })
        };
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_chat_draft_message(chat_id, None, draft, client).await;
        })
        .detach();
    }

    /// Append an emoji (or any text) to the composer from the picker.
    pub fn insert_emoji(&self, emoji: &str) {
        let mut s = self.composer.get().to_string();
        s.push_str(emoji);
        self.composer.set_from(s);
    }

    /// Open the media viewer overlay for a bubble's photo/playable file.
    pub fn open_viewer(&self, row: &MessageRow) {
        let (file, video) = if row.play_file != 0 {
            (
                row.play_file,
                !matches!(row.media_label.as_str(), "Voice" | "Audio"),
            )
        } else {
            (row.media_file, false)
        };
        if file == 0 {
            return;
        }
        self.viewer.set(Some(ViewerRow {
            file,
            video,
            caption: row.text.clone(),
            from: row.sender.clone(),
        }));
    }

    pub fn close_viewer(&self) {
        self.viewer.set(None);
    }

    /// Vote in a poll (`setPollAnswer` takes option indices).
    pub fn vote_poll(&self, message_id: i64, option: usize) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ =
                functions::set_poll_answer(chat_id, message_id, vec![option as i32], client)
                    .await;
        })
        .detach();
    }

    /// Telegram's `poll_answer_count_max` option value (TDLib default 10).
    pub const POLL_MAX_OPTIONS: usize = 10;

    /// Toggle the composer poll creator (attach menu → Poll).
    pub fn toggle_poll_creator(&self) {
        self.poll_open.toggle();
    }

    /// Append one empty option slot, capped at `POLL_MAX_OPTIONS`.
    pub fn add_poll_option(&self) {
        let n = self.poll_option_count.get();
        if n < self.poll_option_fields.len() {
            self.poll_option_count.set(n + 1);
        }
    }

    /// Remove the option at `ix`, shifting later texts up; a poll keeps at
    /// least two options.
    pub fn remove_poll_option(&self, ix: usize) {
        let n = self.poll_option_count.get();
        if n <= 2 || ix >= n {
            return;
        }
        for i in ix..n - 1 {
            let next = self.poll_option_fields[i + 1].get();
            self.poll_option_fields[i].set(next);
        }
        self.poll_option_fields[n - 1].set_from("");
        self.poll_option_count.set(n - 1);
        let correct = self.poll_correct.get();
        if correct >= n - 1 {
            self.poll_correct.set(0);
        }
    }

    /// Send the poll via `sendMessage inputMessagePoll`; no-op until the
    /// question and at least two non-empty options are filled.
    pub fn send_poll(&self) {
        let chat_id = self.open_chat.get();
        let question = self.poll_question.get().to_string().trim().to_string();
        let n = self.poll_option_count.get();
        let options: Vec<String> = (0..n)
            .map(|i| {
                self.poll_option_fields[i]
                    .get()
                    .to_string()
                    .trim()
                    .to_string()
            })
            .filter(|o| !o.is_empty())
            .collect();
        if chat_id == 0 || question.is_empty() || options.len() < 2 {
            return;
        }
        let quiz = self.poll_quiz.get();
        let correct = self.poll_correct.get().min(options.len() - 1) as i32;
        let multiple = self.poll_multiple.get();
        let anonymous = self.poll_anonymous.get();
        self.poll_open.set(false);
        self.poll_question.set_from("");
        for field in &self.poll_option_fields {
            field.set_from("");
        }
        self.poll_option_count.set(2);
        self.poll_quiz.set(false);
        self.poll_multiple.set(false);
        self.poll_anonymous.set(true);
        self.poll_correct.set(0);
        let client = self.client_id.get();
        spawn_local(async move {
            let content = enums::InputMessageContent::InputMessagePoll(
                types::InputMessagePoll {
                    question: types::FormattedText {
                        text: question,
                        entities: Vec::new(),
                    },
                    options: options
                        .into_iter()
                        .map(|text| types::FormattedText {
                            text,
                            entities: Vec::new(),
                        })
                        .collect(),
                    is_anonymous: anonymous,
                    r#type: if quiz {
                        enums::PollType::Quiz(types::PollTypeQuiz {
                            correct_option_id: correct,
                            explanation: types::FormattedText {
                                text: String::new(),
                                entities: Vec::new(),
                            },
                        })
                    } else {
                        enums::PollType::Regular(types::PollTypeRegular {
                            allow_multiple_answers: multiple,
                        })
                    },
                },
            );
            let _ = functions::send_message(chat_id, None, None, None, content, client).await;
        })
        .detach();
    }

    /// Delete the whole history of a chat without leaving it.
    pub fn clear_history(&self, chat_id: i64) {
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::delete_chat_history(chat_id, false, false, client).await;
        })
        .detach();
    }

    /// Load the main block list into `blocked` (settings → privacy).
    #[allow(if_else_view)] // string fallback, not a view
    pub fn load_blocked(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::MessageSenders::MessageSenders(list)) =
                functions::get_blocked_message_senders(
                    enums::BlockList::Main,
                    0,
                    50,
                    client,
                )
                .await
            {
                let mut rows = Vec::new();
                for sender in list.senders {
                    if let enums::MessageSender::User(u) = &sender {
                        // Blocked senders usually aren't in the user cache
                        // yet; resolve the name before building the row.
                        if let Ok(enums::User::User(user)) =
                            functions::get_user(u.user_id, client).await
                        {
                            store.users.borrow_mut().insert(u.user_id, user);
                        }
                    }
                    let name = match &sender {
                        enums::MessageSender::User(_) => {
                            store.sender_name(&sender).to_string()
                        }
                        enums::MessageSender::Chat(c) => store.chat_name(c.chat_id),
                    };
                    let key = match &sender {
                        enums::MessageSender::User(u) => u.user_id,
                        enums::MessageSender::Chat(c) => c.chat_id,
                    };
                    rows.push(MemberRow {
                        key,
                        name: if name.is_empty() { "Blocked".into() } else { name.into() },
                        status: "".into(),
                        sender: sender.clone(),
                        username: "".into(),
                        photo: 0,
                    });
                }
                store.blocked.set(rows);
            }
        })
        .detach();
    }

    /// Unblock a sender (block list `None` = not blocked), then refresh.
    pub fn unblock_sender(&self, member: &MemberRow) {
        let sender = member.sender.clone();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let _ =
                functions::set_message_sender_block_list(sender, None, client).await;
            store.load_blocked();
        })
        .detach();
    }

    /// Start an end-to-end encrypted secret chat with a contact.
    pub fn new_secret_chat(&self, user_id: i64) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Chat::Chat(chat)) =
                functions::create_new_secret_chat(user_id, client).await
            {
                store.nav.pop();
                store.select_chat(chat.id);
            }
        })
        .detach();
    }

    /// Fetch the open chat's scheduled messages and open the panel.
    pub fn toggle_scheduled(&self) {
        let open = !self.scheduled_open.get();
        self.scheduled_open.set(open);
        if open {
            self.load_scheduled();
        }
    }

    pub fn load_scheduled(&self) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        if chat_id == 0 || client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Messages::Messages(list)) =
                functions::get_chat_scheduled_messages(chat_id, client).await
            {
                let mut rows: Vec<MessageRow> = list
                    .messages
                    .iter()
                    .flatten()
                    .map(|m| store.message_row(m))
                    .collect();
                rows.sort();
                store.scheduled.set(rows);
            }
        })
        .detach();
    }

    /// Send a scheduled message immediately (`editMessageSchedulingState`
    /// with no state), then refresh the panel.
    pub fn scheduled_send_now(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::edit_message_scheduling_state(
                chat_id, message_id, None, client,
            )
            .await;
            store.load_scheduled();
        })
        .detach();
    }

    /// Multi-selection: context-menu "Select" starts it, tapping a bubble
    /// toggles membership; an empty selection ends the mode.
    pub fn toggle_select(&self, message_id: i64) {
        let mut sel = self.selected_msgs.get();
        if sel.contains(&message_id) {
            sel.retain(|&x| x != message_id);
        } else {
            sel.push(message_id);
        }
        self.selected_msgs.set(sel);
    }

    pub fn clear_selection(&self) {
        self.selected_msgs.set(Vec::new());
    }

    /// Batch-delete the selected messages (revoke for everyone).
    pub fn delete_selected(&self) {
        let chat_id = self.open_chat.get();
        let ids = self.selected_msgs.get();
        self.selected_msgs.set(Vec::new());
        if chat_id == 0 || ids.is_empty() {
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::delete_messages(chat_id, ids, true, client).await;
        })
        .detach();
    }

    /// Stage the selected batch for forwarding; the next chat tap delivers
    /// it (same pick-a-chat flow as single-message forward).
    pub fn forward_selected(&self) {
        let ids = self.selected_msgs.get();
        if ids.is_empty() {
            return;
        }
        self.selected_msgs.set(Vec::new());
        self.forward_ids.set(ids);
    }

    async fn load_history(&self, chat_id: i64, from: i64, offset: i32) {
        self.loading_history.set(true);
        if let Ok(enums::Messages::Messages(msgs)) = functions::get_chat_history(
            chat_id,
            from,
            offset,
            50,
            false,
            self.client_id.get(),
        )
        .await
        {
            let mut rows: Vec<MessageRow> = msgs
                .messages
                .iter()
                .flatten()
                .map(|m| self.message_row(m))
                .collect();
            if let Some(min) = rows.iter().map(|r| r.id).min() {
                let current = self.oldest_message.get();
                if current == 0 || min < current {
                    self.oldest_message.set(min);
                }
            }
            self.no_more_history
                .set(msgs.messages.len() < 50);
            let mut list = self.messages.get();
            for row in rows.drain(..) {
                if !list.iter().any(|r| r.id == row.id) {
                    list.push(row);
                }
            }
            list.sort();
            self.set_messages(list);
            if offset == 0 && from == 0 {
                self.scroll_bottom();
            }
            if let Some(last) = msgs.messages.iter().flatten().next()
                && !last.is_outgoing
            {
                let client = self.client_id.get();
                let mid = last.id;
                spawn_local(async move {
                    let _ =
                        functions::view_messages(chat_id, vec![mid], None, false, client)
                            .await;
                })
                .detach();
            }
        }
        self.loading_history.set(false);
    }

    /// Fetch the next page of older history for the open chat.
    pub fn load_older(&self) {
        if self.loading_history.get() || self.no_more_history.get() {
            return;
        }
        let chat_id = self.open_chat.get();
        let oldest = self.oldest_message.get();
        if chat_id == 0 || oldest == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            store.load_history(chat_id, oldest, -1).await;
        })
        .detach();
    }

    pub(crate) fn scroll_bottom(&self) {
        let last = self.messages.get().len().saturating_sub(1);
        self.scroll.scroll_to(last);
    }

    pub fn send(&self) {
        self.send_opt(None);
    }

    /// Send without a notification for the recipient.
    pub fn send_silent(&self) {
        self.send_opt(Some(types::MessageSendOptions {
            disable_notification: true,
            ..Default::default()
        }));
    }

    /// Schedule the message `secs` from now.
    pub fn send_later(&self, secs: i32) {
        let at = chrono::Local::now().timestamp() + i64::from(secs);
        self.send_opt(Some(types::MessageSendOptions {
            scheduling_state: Some(enums::MessageSchedulingState::SendAtDate(
                types::MessageSchedulingStateSendAtDate {
                    send_date: at as i32,
                    repeat_period: 0,
                },
            )),
            ..Default::default()
        }));
    }

    /// Shared composer send path. `options` applies to text sends only —
    /// attachments always go immediately for now.
    fn send_opt(&self, options: Option<types::MessageSendOptions>) {
        if !self.attach.get().is_empty() {
            self.send_attachment();
            return;
        }
        let chat_id = self.open_chat.get();
        let text = self.composer.get().to_string();
        if chat_id == 0 || text.trim().is_empty() {
            return;
        }
        if let Some(msg_id) = self.editing.get() {
            self.editing.set(None);
            self.composer.set_from("");
            self.set_draft(chat_id, None);
            let client = self.client_id.get();
            spawn_local(async move {
                let _ = functions::edit_message_text(
                    chat_id,
                    msg_id,
                    enums::InputMessageContent::InputMessageText(
                        types::InputMessageText {
                            text: types::FormattedText {
                                text,
                                entities: Vec::new(),
                            },
                            link_preview_options: None,
                            clear_draft: true,
                        },
                    ),
                    client,
                )
                .await;
            })
            .detach();
            return;
        }
        let reply = self.reply_to.get().map(|id| {
            enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
                message_id: id,
                quote: None,
                checklist_task_id: 0,
            })
        });
        self.reply_to.set(None);
        self.composer.set_from("");
        self.set_draft(chat_id, None);
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                reply,
                options,
                enums::InputMessageContent::InputMessageText(
                    types::InputMessageText {
                        text: types::FormattedText {
                            text,
                            entities: Vec::new(),
                        },
                        link_preview_options: None,
                        clear_draft: true,
                    },
                ),
                client,
            )
            .await;
        })
        .detach();
    }

    /// Build the right `InputMessageContent` for a local file path based on
    /// its extension: photos, videos, and audio get their native message
    /// types; everything else goes as a document.
    pub(crate) fn attachment_content(path: String, caption: String) -> enums::InputMessageContent {
        let ext = path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase();
        let file = enums::InputFile::Local(types::InputFileLocal { path });
        let caption = if caption.is_empty() {
            None
        } else {
            Some(types::FormattedText {
                text: caption,
                entities: Vec::new(),
            })
        };
        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "webp" | "bmp" => {
                enums::InputMessageContent::InputMessagePhoto(types::InputMessagePhoto {
                    photo: file,
                    thumbnail: None,
                    added_sticker_file_ids: Vec::new(),
                    width: 0,
                    height: 0,
                    caption,
                    show_caption_above_media: false,
                    self_destruct_type: None,
                    has_spoiler: false,
                })
            }
            "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => {
                enums::InputMessageContent::InputMessageVideo(types::InputMessageVideo {
                    video: file,
                    thumbnail: None,
                    cover: None,
                    start_timestamp: 0,
                    added_sticker_file_ids: Vec::new(),
                    duration: 0,
                    width: 0,
                    height: 0,
                    supports_streaming: true,
                    caption,
                    show_caption_above_media: false,
                    self_destruct_type: None,
                    has_spoiler: false,
                })
            }
            "ogg" | "opus" => {
                enums::InputMessageContent::InputMessageVoiceNote(
                    types::InputMessageVoiceNote {
                        voice_note: file,
                        duration: 0,
                        waveform: String::new(),
                        caption,
                        self_destruct_type: None,
                    },
                )
            }
            "mp3" | "m4a" | "flac" | "wav" => {
                enums::InputMessageContent::InputMessageAudio(types::InputMessageAudio {
                    audio: file,
                    album_cover_thumbnail: None,
                    duration: 0,
                    title: String::new(),
                    performer: String::new(),
                    caption,
                })
            }
            "gif" => enums::InputMessageContent::InputMessageAnimation(
                types::InputMessageAnimation {
                    animation: file,
                    thumbnail: None,
                    added_sticker_file_ids: Vec::new(),
                    duration: 0,
                    width: 0,
                    height: 0,
                    caption,
                    show_caption_above_media: false,
                    has_spoiler: false,
                },
            ),
            _ => enums::InputMessageContent::InputMessageDocument(
                types::InputMessageDocument {
                    document: file,
                    thumbnail: None,
                    disable_content_type_detection: false,
                    caption,
                },
            ),
        }
    }

    pub fn send_attachment(&self) {
        let chat_id = self.open_chat.get();
        let urls = self.attach.get();
        if chat_id == 0 || urls.is_empty() {
            return;
        }
        let paths: Vec<String> = urls
            .iter()
            .map(|u| u.path().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if paths.is_empty() {
            return;
        }
        let preview_caption = self.attach_caption.get().to_string();
        let caption = if preview_caption.is_empty() {
            self.composer.get().to_string()
        } else {
            preview_caption
        };
        self.attach.set(Vec::new());
        self.attach_caption.set_from("");
        self.composer.set_from("");
        let client = self.client_id.get();
        spawn_local(async move {
            match Self::plan_attachments(paths, caption) {
                AttachmentPlan::Album(items) => {
                    let contents = items
                        .into_iter()
                        .map(|(path, caption)| Self::attachment_content(path, caption))
                        .collect();
                    let _ = functions::send_message_album(
                        chat_id, None, None, None, contents, client,
                    )
                    .await;
                }
                AttachmentPlan::Singles(items) => {
                    for (path, caption) in items {
                        let _ = functions::send_message(
                            chat_id,
                            None,
                            None,
                            None,
                            Self::attachment_content(path, caption),
                            client,
                        )
                        .await;
                    }
                }
            }
        })
        .detach();
    }

    /// Toggle voice-note recording: first tap starts, second stops+encodes.
    pub fn toggle_voice_record(&self) {
        if self.voice_session.borrow().is_some() {
            self.finish_voice_record();
            return;
        }
        match crate::capture::start_voice() {
            Ok(cap) => {
                self.capture_error.set_from("");
                *self.voice_session.borrow_mut() = Some(cap);
                self.recording_voice.set(true);
                let store = self.clone();
                spawn_local(async move {
                    loop {
                        sleep(std::time::Duration::from_millis(500)).await;
                        if store.voice_session.borrow().is_none() {
                            break;
                        }
                        store.refresh_voice_elapsed();
                    }
                })
                .detach();
                self.refresh_voice_elapsed();
            }
            Err(e) => {
                self.capture_error.set_from(e);
                self.recording_voice.set(false);
            }
        }
    }

    /// Update the "0:SS" elapsed label from the live recorder.
    fn refresh_voice_elapsed(&self) {
        let secs = self
            .voice_session
            .borrow()
            .as_ref()
            .map(|c| c.elapsed())
            .unwrap_or(0);
        self.voice_elapsed.set_from(format!("0:{secs:02}"));
    }

    /// Stop + encode + send the recorded voice note.
    fn finish_voice_record(&self) {
        let Some(cap) = self.voice_session.borrow_mut().take() else {
            return;
        };
        self.recording_voice.set(false);
        match cap.finish() {
            Ok(take) => {
                self.voice_elapsed.set_from("");
                if take.duration < 1 {
                    self.capture_error.set_from("recording too short");
                    return;
                }
                let path = self.next_capture_path("ogg");
                if std::fs::write(&path, &take.data).is_err() {
                    self.capture_error.set_from("failed to write recording");
                    return;
                }
                self.send_voice_file(path, take.duration, take.waveform);
            }
            Err(e) => self.capture_error.set_from(e),
        }
    }

    /// Send a finished .ogg voice note to the open chat.
    fn send_voice_file(&self, path: String, duration: i32, waveform: String) {
        if self.open_chat.get() == 0 {
            return;
        }
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                None,
                None,
                enums::InputMessageContent::InputMessageVoiceNote(
                    types::InputMessageVoiceNote {
                        voice_note: enums::InputFile::Local(types::InputFileLocal {
                            path,
                        }),
                        duration,
                        waveform,
                        caption: None,
                        self_destruct_type: None,
                    },
                ),
                client,
            )
            .await;
        })
        .detach();
    }

    /// Cancel recording without sending.
    pub fn cancel_voice_record(&self) {
        if let Some(cap) = self.voice_session.borrow_mut().take() {
            drop(cap); // recorder drops → cpal stream ends
        }
        self.recording_voice.set(false);
        self.voice_elapsed.set_from("");
    }

    /// Open the video-note sheet: spin up camera + preview pump.
    pub fn open_video_note(&self) {
        if self.video_shared.borrow().is_none() {
            // Fresh shared state — the GpuSurface's GpuView opens the camera on
            // the render surface's own device/queue in `setup`.
            *self.video_shared.borrow_mut() = Some(crate::capture::new_video_note_shared());
        }
        self.video_note_open.set(true);
    }

    /// Close the sheet and drop the camera session.
    pub fn close_video_note(&self) {
        if self.video_recording.get() {
            self.finish_video_record();
        }
        self.video_shared.borrow_mut().take();
        self.video_done_rx.borrow_mut().take();
        self.video_note_open.set(false);
        self.video_recording.set(false);
        self.video_elapsed.set_from("");
    }

    /// Start collecting frames for the recording.
    pub fn start_video_record(&self) {
        let shared = self.video_shared.borrow().clone();
        let Some(shared) = shared else {
            return;
        };
        let path = self.next_capture_path("mp4");
        *self.video_path.borrow_mut() = path;
        crate::capture::video_note_start_recording(&shared, self.video_path.borrow().clone().into());
        self.video_recording.set(true);
        self.refresh_video_elapsed();
    }

    /// Update the video-note elapsed label.
    pub fn refresh_video_elapsed(&self) {
        let shared = self.video_shared.borrow().clone();
        let secs = shared
            .as_ref()
            .map(crate::capture::video_note_elapsed)
            .unwrap_or(0);
        self.video_elapsed.set_from(format!("0:{secs:02}"));
    }

    /// Stop recording; the encoder thread finishes the mp4 and replies on a
    /// channel this poll reads back on the UI task (never blocks).
    pub fn finish_video_record(&self) {
        self.video_recording.set(false);
        let shared = self.video_shared.borrow().clone();
        let Some(shared) = shared else {
            return;
        };
        let rx = crate::capture::video_note_stop_recording(&shared);
        shared.borrow().status.set_from("finishing…");
        *self.video_done_rx.borrow_mut() = rx;
        let store = self.clone();
        spawn_local(async move {
            loop {
                sleep(std::time::Duration::from_millis(120)).await;
                let result = {
                    let mut slot = store.video_done_rx.borrow_mut();
                    match slot.as_ref().map(|rx| rx.try_recv()) {
                        Some(Ok(result)) => {
                            *slot = None;
                            Some(result)
                        }
                        Some(Err(mpsc::TryRecvError::Disconnected)) => {
                            *slot = None;
                            None
                        }
                        _ => continue,
                    }
                };
                match result {
                    Some(Ok(done)) => {
                        let thumb_path = store.next_capture_path("jpg");
                        let thumb_path_w = thumb_path.clone();
                        let thumb_bytes = done.thumb;
                        std::thread::spawn(move || {
                            let _ = std::fs::write(thumb_path_w, &thumb_bytes);
                        });
                        let path = store.video_path.borrow().clone();
                        store.send_video_file(path, thumb_path, done.duration);
                        if let Some(sh) = store.video_shared.borrow().as_ref() {
                            sh.borrow().status.set_from("sent");
                        }
                        break;
                    }
                    Some(Err(e)) => {
                        if let Some(sh) = store.video_shared.borrow().as_ref() {
                            sh.borrow().status.set_from(format!("encode: {e}"));
                        }
                        break;
                    }
                    None => break,
                }
            }
        })
        .detach();
    }

    /// Send the recorded mp4 as a video-note message.
    fn send_video_file(&self, path: String, thumb: String, duration: i32) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                None,
                None,
                enums::InputMessageContent::InputMessageVideoNote(
                    types::InputMessageVideoNote {
                        video_note: enums::InputFile::Local(types::InputFileLocal {
                            path,
                        }),
                        thumbnail: Some(types::InputThumbnail {
                            thumbnail: enums::InputFile::Local(types::InputFileLocal {
                                path: thumb,
                            }),
                            width: crate::capture::VIDEO_NOTE_SIZE as i32,
                            height: crate::capture::VIDEO_NOTE_SIZE as i32,
                        }),
                        duration,
                        length: crate::capture::VIDEO_NOTE_SIZE as i32,
                        self_destruct_type: None,
                    },
                ),
                client,
            )
            .await;
        })
        .detach();
    }

    /// Next file path inside the client's files dir for a capture artifact.
    fn next_capture_path(&self, ext: &str) -> String {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram")
            .join(format!("files_{}", self.client_id.get()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!(
            "capture_{}.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            ext
        ))
        .to_string_lossy()
        .into_owned()
    }

    /// Decide how to deliver the composer's picked files: two or more
    /// image/video files go out together as one album (caption on the
    /// first item); anything else sends as individual messages, caption
    /// on the first one.
    pub(crate) fn plan_attachments(paths: Vec<String>, caption: String) -> AttachmentPlan {
        let is_media = |p: &str| {
            matches!(
                p.rsplit('.').next().unwrap_or("").to_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "bmp" | "mp4" | "mov" | "mkv" | "webm" | "avi"
                    | "m4v"
            )
        };
        let media_count = paths.iter().filter(|p| is_media(p)).count();
        if paths.len() == media_count && media_count >= 2 {
            let items = paths
                .into_iter()
                .enumerate()
                .map(|(i, p)| (p, if i == 0 { caption.clone() } else { String::new() }))
                .collect();
            return AttachmentPlan::Album(items);
        }
        let mut caption_left = caption;
        let items = paths
            .into_iter()
            .map(|p| {
                let c = std::mem::take(&mut caption_left);
                (p, c)
            })
            .collect();
        AttachmentPlan::Singles(items)
    }

    pub fn typing_ping(&self) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if self.last_typing_sent.get().elapsed().as_secs() < 3 {
            return;
        }
        self.last_typing_sent.set(Instant::now());
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::send_chat_action(
                chat_id,
                None,
                Some(enums::ChatAction::Typing),
                client,
            )
            .await;
        })
        .detach();
    }

    pub fn delete_message(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        spawn_local(async move {
            let _ =
                functions::delete_messages(chat_id, vec![message_id], true, client).await;
        })
        .detach();
    }

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
    pub fn start_reply(&self, row: &MessageRow) {
        self.reply_to.set(Some(row.id));
        let label = if row.text.is_empty() {
            row.media_label.clone()
        } else {
            row.text.clone()
        };
        self.reply_label.set(label);
    }

    pub fn start_edit(&self, row: &MessageRow) {
        if row.can_edit {
            self.editing.set(Some(row.id));
            self.composer.set(row.text.clone());
        }
    }

    /// Copy a message's text to the internal clipboard binding (and the OS
    /// clipboard when available).
    pub fn copy_message(&self, row: &MessageRow) {
        self.clipboard.set(row.text.clone());
        let text = row.text.to_string();
        if !text.is_empty()
            && let Ok(mut cb) = arboard::Clipboard::new()
        {
            let _ = cb.set_text(text);
        }
    }

    pub fn start_forward(&self, row: &MessageRow) {
        let from = self.open_chat.get();
        if from != 0 {
            self.forward_message.set(Some((from, row.id)));
        }
    }

    pub fn toggle_pin(&self, chat_id: i64) {
        let pinned = self
            .chats
            .get()
            .iter()
            .find(|r| r.id == chat_id)
            .map(|r| r.pinned)
            .unwrap_or(false);
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::toggle_chat_is_pinned(
                enums::ChatList::Main,
                chat_id,
                !pinned,
                client,
            )
            .await;
        })
        .detach();
    }

    pub fn toggle_mute(&self, chat_id: i64) {
        let Some(settings) = self
            .chat_objs
            .borrow()
            .get(&chat_id)
            .map(|c| c.notification_settings.clone())
        else {
            return;
        };
        let muted = !settings.use_default_mute_for && settings.mute_for > 0;
        let mut next = settings;
        next.use_default_mute_for = false;
        next.mute_for = if muted { 0 } else { i32::MAX };
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_chat_notification_settings(chat_id, next, client).await;
        })
        .detach();
    }

    pub fn mark_read(&self, chat_id: i64) {
        let Some(last) = self
            .chat_objs
            .borrow()
            .get(&chat_id)
            .and_then(|c| c.last_message.as_ref().map(|m| m.id))
        else {
            return;
        };
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::view_messages(chat_id, vec![last], None, true, client).await;
        })
        .detach();
    }

    pub fn leave(&self, chat_id: i64) {
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::leave_chat(chat_id, client).await;
        })
        .detach();
    }

    /// Join a public group/channel the user found via search.
    pub fn join(&self, chat_id: i64) {
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::join_chat(chat_id, client).await;
        })
        .detach();
    }

    /// Switch the sidebar between the Main and Archive chat lists.
    pub fn toggle_archive_view(&self) {
        let next = !self.archive_mode.get();
        self.set_list(if next { -1 } else { 0 });
    }

    /// Toggle the accounts panel from the sidebar menu.
    pub fn toggle_accounts(&self) {
        self.accounts_open.toggle();
    }

    /// Switch the sidebar list: 0 = All chats, -1 = Archive, n = folder.
    pub fn set_list(&self, list_id: i32) {
        if self.active_folder.get() == list_id && self.archive_mode.get() == (list_id == -1) {
            return;
        }
        self.active_folder.set(list_id);
        self.archive_mode.set(list_id == -1);
        self.chats.set(Vec::new());
        let chats: Vec<types::Chat> = self.chat_objs.borrow().values().cloned().collect();
        for c in chats {
            self.upsert_chat(c);
        }
        let client = self.client_id.get();
        if client != 0 && list_id > 0 {
            spawn_local(async move {
                let _ = functions::load_chats(
                    Some(enums::ChatList::Folder(types::ChatListFolder {
                        chat_folder_id: list_id,
                    })),
                    200,
                    client,
                )
                .await;
            })
            .detach();
        }
    }

    /// Extract the active @mention token: the run of word chars after the
    /// last '@' when that '@' starts a word (at text start or after
    /// whitespace). Bare "@" yields an empty token (show all members).
    pub fn mention_token(s: &str) -> Option<String> {
        let b = s.as_bytes();
        let mut i = b.len();
        while i > 0 {
            let c = b[i - 1];
            if c == b'@' {
                return if i == 1 || b[i - 2].is_ascii_whitespace() {
                    Some(s[i..].to_string())
                } else {
                    None
                };
            }
            if !(c.is_ascii_alphanumeric() || c == b'_') {
                return None;
            }
            i -= 1;
        }
        None
    }

    /// Replace the trailing @token in the composer with `@username `.
    pub fn apply_mention(&self, username: &str) {
        let cur = self.composer.get().to_string();
        if let Some(tok) = Self::mention_token(&cur) {
            let at = cur.len() - tok.len() - 1;
            let mut s = String::with_capacity(at + username.len() + 2);
            s.push_str(&cur[..at]);
            s.push('@');
            s.push_str(username.trim_start_matches('@'));
            s.push(' ');
            self.composer.set_from(s);
        }
    }

    /// Lazily populate `members` when the composer enters @mention context.
    pub fn maybe_load_members(&self) {
        if Self::mention_token(&self.composer.get()).is_some()
            && self.members.get().is_empty()
        {
            self.load_members();
        }
    }

    /// Move a chat between Main and Archive.
    pub fn toggle_archive(&self, chat_id: i64) {
        let archived = self
            .chat_objs
            .borrow()
            .get(&chat_id)
            .map(|c| {
                c.positions
                    .iter()
                    .any(|p| p.list == enums::ChatList::Archive)
            })
            .unwrap_or(false);
        let list = if archived {
            enums::ChatList::Main
        } else {
            enums::ChatList::Archive
        };
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::add_chat_to_list(chat_id, list, client).await;
        })
        .detach();
    }

    /// Load the open group/channel's member list into `members`.
    pub fn load_members(&self) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::ChatMembers::ChatMembers(m)) =
                functions::search_chat_members(chat_id, String::new(), 50, None, client)
                    .await
            {
                store
                    .members_count
                    .set_from(format!("{} members", m.total_count));
                let mut rows = Vec::new();
                for member in m.members {
                    let (key, name, sender, username, photo) = match &member.member_id {
                        enums::MessageSender::User(u) => {
                            let uid = u.user_id;
                            let cached = store.users.borrow().get(&uid).cloned();
                            let user = match cached {
                                Some(u) => Some(u),
                                None => match functions::get_user(uid, client).await {
                                    Ok(enums::User::User(u)) => {
                                        store.users.borrow_mut().insert(uid, u.clone());
                                        Some(u)
                                    }
                                    _ => None,
                                },
                            };
                            let name = user
                                .as_ref()
                                .map(|u| {
                                    format!("{} {}", u.first_name, u.last_name)
                                        .trim()
                                        .to_string()
                                })
                                .filter(|n| !n.is_empty())
                                .unwrap_or_else(|| format!("User {uid}"));
                            let uname = user
                                .as_ref()
                                .and_then(|u| u.usernames.as_ref())
                                .and_then(|us| us.active_usernames.first().cloned())
                                .unwrap_or_default();
                            let photo = user
                                .as_ref()
                                .and_then(|u| u.profile_photo.as_ref())
                                .map(|p| p.small.id)
                                .unwrap_or(0);
                            if photo != 0 {
                                store.want_file_id(photo);
                            }
                            (uid, name, member.member_id.clone(), uname, photo)
                        }
                        enums::MessageSender::Chat(c) => {
                            let (title, photo) = store
                                .chat_objs
                                .borrow()
                                .get(&c.chat_id)
                                .map(|ch| {
                                    (
                                        ch.title.clone(),
                                        ch.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
                                    )
                                })
                                .unwrap_or_else(|| (format!("Chat {}", c.chat_id), 0));
                            (c.chat_id, title, member.member_id.clone(), String::new(), photo)
                        }
                    };
                    let status = match &member.status {
                        enums::ChatMemberStatus::Creator(_) => "owner",
                        enums::ChatMemberStatus::Administrator(_) => "admin",
                        enums::ChatMemberStatus::Member(_) => "member",
                        enums::ChatMemberStatus::Restricted(_) => "restricted",
                        enums::ChatMemberStatus::Left => "left",
                        enums::ChatMemberStatus::Banned(_) => "banned",
                    };
                    rows.push(MemberRow {
                        key,
                        name: name.into(),
                        status: status.into(),
                        sender,
                        username: username.into(),
                        photo,
                    });
                }
                store.members.set(rows);
            }
        })
        .detach();
    }

    /// Populate `contacts` for the New Chat screen.
    #[allow(if_else_view)] // string picks, not views
    pub fn load_contacts(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let mut rows = Vec::new();
            if let Ok(enums::Users::Users(u)) = functions::get_contacts(client).await {
                for uid in u.user_ids {
                    if let Ok(enums::User::User(user)) = functions::get_user(uid, client).await {
                        store.users.borrow_mut().insert(uid, user.clone());
                        let uname = user
                            .usernames
                            .as_ref()
                            .and_then(|u| u.active_usernames.first())
                            .map(|s| format!("@{s}"))
                            .unwrap_or_default();
                        let name = format!("{} {}", user.first_name, user.last_name)
                            .trim()
                            .to_string();
                        rows.push(MemberRow {
                            key: uid,
                            name: if name.is_empty() { uname.clone().into() } else { name.into() },
                            status: uname.clone().into(),
                            sender: enums::MessageSender::User(types::MessageSenderUser {
                                user_id: uid,
                            }),
                            username: uname.trim_start_matches('@').to_string().into(),
                            photo: user.profile_photo.map(|p| p.small.id).unwrap_or(0),
                        });
                    }
                }
            }
            store.contacts.set(rows);
        })
        .detach();
    }

    /// Open a private chat with a user (from contacts) and select it.
    pub fn start_chat_with(&self, user_id: i64) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Chat::Chat(chat)) =
                functions::create_private_chat(user_id, false, client).await
            {
                store.nav.pop();
                store.select_chat(chat.id);
            }
        })
        .detach();
    }

    /// Picker tab switch: Emoji (local grid), Stickers, GIFs (inline bot).
    pub fn pick_panel_tab(&self, tab: i32) {
        self.panel_tab.set(tab);
        match tab {
            1 => self.load_stickers(),
            2 => {
                let q = self.sticker_query.get().to_string();
                self.search_gifs_by(q);
            }
            _ => {}
        }
    }

    /// Toggle the sticker/GIF picker; loads recent stickers + saved GIFs
    /// on first open.
    pub fn toggle_stickers(&self) {
        let open = !self.stickers_open.get();
        self.stickers_open.set(open);
        if open {
            self.load_stickers();
        }
    }

    /// Recent stickers plus saved GIFs, thumbnails kicked into the
    /// shared file-download map.
    pub fn load_stickers(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        self.load_sticker_packs();
        let store = self.clone();
        spawn_local(async move {
            let mut items: Vec<StickerItem> = Vec::new();
            if let Ok(enums::Stickers::Stickers(r)) =
                functions::get_recent_stickers(false, client).await
            {
                for st in r.stickers {
                    let thumb = st.thumbnail.as_ref().map(|t| t.file.id).unwrap_or(0);
                    if thumb != 0 {
                        let _ =
                            functions::download_file(thumb, 8, 0, 0, false, client).await;
                    }
                    items.push(StickerItem {
                        file_id: st.sticker.id,
                        thumb,
                        emoji: st.emoji.into(),
                        gif: false,
                    });
                }
            }
            if let Ok(enums::Animations::Animations(a)) =
                functions::get_saved_animations(client).await
            {
                for an in a.animations {
                    let thumb = an.thumbnail.as_ref().map(|t| t.file.id).unwrap_or(0);
                    if thumb != 0 {
                        let _ =
                            functions::download_file(thumb, 8, 0, 0, false, client).await;
                    }
                    items.push(StickerItem {
                        file_id: an.animation.id,
                        thumb,
                        emoji: "".into(),
                        gif: true,
                    });
                }
            }
            store.sticker_items.set(items);
        })
        .detach();
    }

    /// Load installed sticker packs into the picker's pack strip.
    pub fn load_sticker_packs(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::StickerSets::StickerSets(ss)) =
                functions::get_installed_sticker_sets(
                    enums::StickerType::Regular,
                    client,
                )
                .await
            {
                store.sticker_packs.set(
                    ss.sets
                        .iter()
                        .map(|i| PackRow {
                            id: i.id,
                            title: i.title.clone().into(),
                        })
                        .collect(),
                );
            }
        })
        .detach();
    }

    /// Load a sticker pack's stickers into the picker cells.
    pub fn open_sticker_pack(&self, set_id: i64) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::StickerSet::StickerSet(set)) =
                functions::get_sticker_set(set_id, client).await
            {
                store.replace_stickers_with(set.stickers);
            }
        })
        .detach();
    }

    /// Register once-per-session watchers pushing toggle state to TDLib.
    pub fn start_notification_watchers(&self) {
        if !self.notif_watchers.borrow().is_empty() {
            return;
        }
        for (binding, scope) in [
            (self.notif_private.clone(), "private"),
            (self.notif_groups.clone(), "groups"),
            (self.notif_channels.clone(), "channels"),
        ] {
            let store = self.clone();
            let guard = Signal::watch(
                &binding,
                move |ctx| store.set_scope_notifications(scope, ctx.into_value()),
            );
            self.notif_watchers
                .borrow_mut()
                .push(Box::new(guard) as Box<dyn std::any::Any>);
        }
    }

    /// Apply a global notification mute toggle for one scope.
    pub fn set_scope_notifications(&self, scope: &str, on: bool) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let ns_scope = match scope {
            "groups" => enums::NotificationSettingsScope::GroupChats,
            "channels" => enums::NotificationSettingsScope::ChannelChats,
            _ => enums::NotificationSettingsScope::PrivateChats,
        };
        let settings = types::ScopeNotificationSettings {
            mute_for: if on { 0 } else { i32::MAX },
            sound_id: 0,
            show_preview: true,
            use_default_mute_stories: true,
            mute_stories: false,
            story_sound_id: 0,
            show_story_poster: false,
            disable_pinned_message_notifications: false,
            disable_mention_notifications: false,
        };
        spawn_local(async move {
            let _ = functions::set_scope_notification_settings(
                ns_scope,
                settings,
                client,
            )
            .await;
        })
        .detach();
    }

    /// Replace picker contents with a TDLib sticker result set.
    fn replace_stickers_with(&self, stickers: Vec<types::Sticker>) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let mut items = Vec::new();
            for st in stickers {
                let thumb = st.thumbnail.as_ref().map(|t| t.file.id).unwrap_or(0);
                if thumb != 0 {
                    let _ =
                        functions::download_file(thumb, 8, 0, 0, false, client).await;
                }
                items.push(StickerItem {
                    file_id: st.sticker.id,
                    thumb,
                    emoji: st.emoji.into(),
                    gif: false,
                });
            }
            store.sticker_items.set(items);
        })
        .detach();
    }

    /// Send a sticker or saved GIF into the open chat.
    pub fn send_sticker(&self, item: StickerItem) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        if chat_id == 0 || client == 0 {
            return;
        }
        self.stickers_open.set(false);
        let content = if item.gif {
            enums::InputMessageContent::InputMessageAnimation(
                types::InputMessageAnimation {
                    animation: enums::InputFile::Id(types::InputFileId { id: item.file_id }),
                    thumbnail: None,
                    added_sticker_file_ids: Vec::new(),
                    duration: 0,
                    width: 0,
                    height: 0,
                    caption: None,
                    show_caption_above_media: false,
                    has_spoiler: false,
                },
            )
        } else {
            enums::InputMessageContent::InputMessageSticker(types::InputMessageSticker {
                sticker: enums::InputFile::Id(types::InputFileId { id: item.file_id }),
                thumbnail: None,
                width: 0,
                height: 0,
                emoji: item.emoji.to_string(),
            })
        };
        spawn_local(async move {
            let _ = functions::send_message(chat_id, None, None, None, content, client).await;
        })
        .detach();
    }

    /// Import the new-contact form fields as a contact, then refresh the
    /// contacts list.
    pub fn add_contact(&self) {
        let phone = self.nc_phone.get().to_string();
        let first = self.nc_first.get().to_string();
        let last = self.nc_last.get().to_string();
        let client = self.client_id.get();
        if client == 0 || phone.trim().is_empty() || first.trim().is_empty() {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let contact = types::ImportedContact {
                phone_number: phone,
                first_name: first,
                last_name: last,
                note: types::FormattedText {
                    text: String::new(),
                    entities: Vec::new(),
                },
            };
            if functions::import_contacts(vec![contact], client)
                .await
                .is_ok()
            {
                store.add_contact_open.set(false);
                store.load_contacts();
            }
        })
        .detach();
    }

    /// Remove a contact by user id, then refresh the list.
    pub fn remove_contact(&self, user_id: i64) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if functions::remove_contacts(vec![user_id], client).await.is_ok() {
                store.load_contacts();
            }
        })
        .detach();
    }

    /// Create a fresh invite link for the open group/channel.
    pub fn create_invite(&self) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        if chat_id == 0 || client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::ChatInviteLink::ChatInviteLink(l)) =
                functions::create_chat_invite_link(chat_id, String::new(), 0, 0, false, client)
                    .await
            {
                store.invite_link.set_from(l.invite_link);
            }
        })
        .detach();
    }

    /// Storage statistics summary for Settings (size + file count).
    pub fn load_storage(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::StorageStatistics::StorageStatistics(st)) =
                functions::get_storage_statistics(0, client).await
            {
                let mb = st.size as f64 / (1024.0 * 1024.0);
                store
                    .storage_summary
                    .set_from(format!("{mb:.1} MB in {} files", st.count));
            }
        })
        .detach();
    }

    /// Clear all cached media files, then refresh the summary.
    pub fn clear_storage(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if functions::optimize_storage(
                0, -1, -1, 0, Vec::new(), Vec::new(), Vec::new(), true, 0, client,
            )
            .await
            .is_ok()
            {
                store.load_storage();
            }
        })
        .detach();
    }

    /// Path of the persisted account list.
    fn accounts_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram")
            .join("accounts.json")
    }

    /// Persist the account labels so future sessions can re-create the
    /// TDLib clients.
    fn persist_accounts(&self) {
        let labels: Vec<String> = self
            .accounts
            .get()
            .iter()
            .map(|a| a.label.to_string())
            .collect();
        if let Ok(json) = serde_json::to_string(&labels)
            && let Some(parent) = Self::accounts_path().parent()
        {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(Self::accounts_path(), json);
        }
    }

    /// Create TDLib clients for the persisted accounts. The active client
    /// is reused for the first row; the rest get fresh clients + params.
    pub fn restore_accounts(&self) {
        let Ok(raw) = std::fs::read_to_string(Self::accounts_path()) else {
            self.accounts.set(vec![AccountRow {
                id: self.client_id.get(),
                label: "Account".into(),
            }]);
            return;
        };
        let labels: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        if labels.is_empty() {
            return;
        }
        let mut rows = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            let id = if i == 0 {
                self.client_id.get()
            } else {
                let id = tdlib_rs::create_client();
                if let Some(cfg) = Config::load() {
                    self.set_tdlib_parameters_for(id, cfg);
                }
                id
            };
            rows.push(AccountRow {
                id,
                label: label.clone().into(),
            });
        }
        self.accounts.set(rows);
        self.refresh_account_label();
    }

    /// Refresh the active account's switcher label via `getMe`.
    pub fn refresh_account_label(&self) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::User::User(me)) = functions::get_me(client).await {
                let name = format!("{} {}", me.first_name, me.last_name)
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    let mut rows = store.accounts.get();
                    if let Some(row) = rows.iter_mut().find(|r| r.id == client) {
                        row.label = name.into();
                        store.accounts.set(rows);
                        store.persist_accounts();
                    }
                }
            }
        })
        .detach();
    }

    /// Add a fresh TDLib client and switch to it (lands on the auth flow).
    pub fn add_account(&self) {
        let id = tdlib_rs::create_client();
        self.accounts_open.set(false);
        let mut rows = self.accounts.get();
        rows.push(AccountRow {
            id,
            label: "New account".into(),
        });
        self.accounts.set(rows);
        self.switch_account(id);
    }

    /// Switch the active TDLib client; caches are per-account so they are
    /// cleared and re-loaded through `get_authorization_state`.
    pub fn switch_account(&self, id: i32) {
        if id == self.client_id.get() {
            self.accounts_open.set(false);
            return;
        }
        self.client_id.set(id);
        self.accounts_open.set(false);
        // Clear account-scoped caches.
        self.chats.set(Vec::new());
        self.set_messages(Vec::new());
        self.folders.set(Vec::new());
        self.members.set(Vec::new());
        self.server_results.set(Vec::new());
        self.chat_objs.borrow_mut().clear();
        self.users.borrow_mut().clear();
        self.files.borrow_mut().clear();
        self.file_progress.borrow_mut().clear();
        self.open_chat.set(0);
        self.active_folder.set(0);
        self.screen.set(Screen::Loading);
        let store = self.clone();
        spawn_local(async move {
            if let Ok(state) = functions::get_authorization_state(id).await {
                store.on_auth_state(state);
            }
        })
        .detach();
    }

    /// `set_tdlib_parameters` for an arbitrary client (saved accounts get
    /// their own `db_<id>` / `files_<id>` dirs).
    fn set_tdlib_parameters_for(&self, client: i32, cfg: Config) {
        let db_root = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram");
        let _ = std::fs::create_dir_all(&db_root);
        let db = db_root
            .join(format!("db_{client}"))
            .to_string_lossy()
            .to_string();
        let files = db_root
            .join(format!("files_{client}"))
            .to_string_lossy()
            .to_string();
        spawn_local(async move {
            let _ = functions::set_tdlib_parameters(
                cfg.test_dc,
                db,
                files,
                String::new(),
                true,
                true,
                true,
                false,
                cfg.api_id,
                cfg.api_hash,
                "en".to_string(),
                "Watergram".to_string(),
                "desktop".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
                client,
            )
            .await;
        })
        .detach();
    }

    /// Apply an audience preset to a privacy setting and reload.
    pub fn set_privacy_audience(&self, key: enums::UserPrivacySetting, audience: &str) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let rule = match audience {
            "Everyone" => enums::UserPrivacySettingRule::AllowAll,
            "Nobody" => enums::UserPrivacySettingRule::RestrictAll,
            _ => enums::UserPrivacySettingRule::AllowContacts,
        };
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::set_user_privacy_setting_rules(
                key,
                types::UserPrivacySettingRules { rules: vec![rule] },
                client,
            )
            .await;
            store.load_privacy();
        })
        .detach();
    }

    /// Open the folder editor for `id` (0 = new folder).
    pub fn open_folder_editor(&self, id: i32) {
        self.editing_folder.set(id);
        if id == 0 {
            self.folder_name.set_from("");
        } else {
            let name = self
                .folders
                .get()
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.title.clone())
                .unwrap_or_default();
            self.folder_name.set(name);
        }
        self.folder_open.set(true);
    }

    /// Create or rename a chat folder from the editor form.
    pub fn save_folder(&self) {
        let client = self.client_id.get();
        let name = self.folder_name.get().to_string();
        if client == 0 || name.trim().is_empty() {
            return;
        }
        let folder = types::ChatFolder {
            name: types::ChatFolderName {
                text: types::FormattedText {
                    text: name,
                    entities: Vec::new(),
                },
                animate_custom_emoji: false,
            },
            icon: None,
            color_id: -1,
            is_shareable: false,
            pinned_chat_ids: Vec::new(),
            included_chat_ids: Vec::new(),
            excluded_chat_ids: Vec::new(),
            exclude_muted: false,
            exclude_read: false,
            exclude_archived: false,
            include_contacts: self.folder_contacts.get(),
            include_non_contacts: false,
            include_bots: false,
            include_groups: self.folder_groups.get(),
            include_channels: self.folder_channels.get(),
        };
        let id = self.editing_folder.get();
        let store = self.clone();
        spawn_local(async move {
            let ok = if id == 0 {
                functions::create_chat_folder(folder, client).await.is_ok()
            } else {
                functions::edit_chat_folder(id, folder, client)
                    .await
                    .is_ok()
            };
            store.folder_open.set(false);
            let _ = ok;
            // The folder list refreshes through Update::ChatFolders.
        })
        .detach();
    }

    /// Delete a chat folder by id (chats stay in the main list).
    pub fn delete_folder(&self, id: i32) {
        let client = self.client_id.get();
        if client == 0 || id <= 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::delete_chat_folder(id, Vec::new(), client).await;
            if store.active_folder.get() == id {
                store.set_list(0);
            }
        })
        .detach();
    }

    /// Search stickers by emoji text and show results in the picker.
    pub fn search_stickers_by(&self) {
        let client = self.client_id.get();
        let emoji = self.sticker_query.get().to_string();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Stickers::Stickers(set)) = functions::search_stickers(
                enums::StickerType::Regular,
                emoji.clone(),
                String::new(),
                Vec::new(),
                0,
                40,
                client,
            )
            .await
            {
                store.replace_stickers_with(set.stickers);
            }
        })
        .detach();
    }

    /// Search trending GIFs through the @gif inline bot and replace the
    /// picker strip with the results (sends go out as `inputMessageAnimation`).
    pub fn search_gifs_by(&self, query: String) {
        let client = self.client_id.get();
        let chat_id = self.open_chat.get();
        if client == 0 || chat_id == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let mut bot_id = store.gif_bot.get();
            if bot_id == 0 {
                let Ok(enums::Chat::Chat(chat)) =
                    functions::search_public_chat("gif".into(), client).await
                else {
                    return;
                };
                if let enums::ChatType::Private(p) = chat.r#type {
                    bot_id = p.user_id;
                    store.gif_bot.set(bot_id);
                }
            }
            if bot_id == 0 {
                return;
            }
            let Ok(enums::InlineQueryResults::InlineQueryResults(res)) =
                functions::get_inline_query_results(
                    bot_id,
                    chat_id,
                    None,
                    query,
                    String::new(),
                    client,
                )
                .await
            else {
                return;
            };
            let mut items = Vec::new();
            for r in res.results {
                if let enums::InlineQueryResult::Animation(a) = r {
                    let thumb = a
                        .animation
                        .thumbnail
                        .as_ref()
                        .map(|t| t.file.id)
                        .unwrap_or(0);
                    if thumb != 0 {
                        let _ =
                            functions::download_file(thumb, 8, 0, 0, false, client)
                                .await;
                    }
                    items.push(StickerItem {
                        file_id: a.animation.animation.id,
                        thumb,
                        emoji: "".into(),
                        gif: true,
                    });
                }
            }
            if !items.is_empty() {
                store.sticker_items.set(items);
            }
        })
        .detach();
    }

    /// Upload the picked file as this chat's photo.
    pub fn set_chat_avatar(&self) {
        let client = self.client_id.get();
        let chat_id = self.open_chat.get();
        let urls = self.chat_avatar_pick.get();
        let Some(url) = urls.first() else { return };
        let path = url.path().to_string();
        if client == 0 || chat_id == 0 || path.is_empty() {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::set_chat_photo(
                chat_id,
                Some(enums::InputChatPhoto::Static(
                    types::InputChatPhotoStatic {
                        photo: enums::InputFile::Local(types::InputFileLocal { path }),
                    },
                )),
                client,
            )
            .await;
            store.chat_avatar_pick.set(Vec::new());
        })
        .detach();
    }

    /// Load a user's profile into the Profile route card and push it.
    pub fn open_profile(&self, user_id: i64) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let mut card = ProfileCard {
                user_id,
                ..Default::default()
            };
            if let Ok(enums::User::User(u)) = functions::get_user(user_id, client).await {
                card.name = format!("{} {}", u.first_name, u.last_name)
                    .trim()
                    .to_string()
                    .into();
                card.username = u
                    .usernames
                    .as_ref()
                    .and_then(|x| x.active_usernames.first())
                    .map(|s| format!("@{s}"))
                    .unwrap_or_default()
                    .into();
                card.phone = u.phone_number.clone().into();
                card.online = matches!(u.status, enums::UserStatus::Online(_));
            }
            if let Ok(enums::UserFullInfo::UserFullInfo(f)) =
                functions::get_user_full_info(user_id, client).await
                && let Some(bio) = f.bio
            {
                card.bio = bio.text.into();
            }
            store.profile.set(Some(card));
            store.nav.push(Route::Profile);
        })
        .detach();
    }

    /// Upload the picked Settings file as the account's profile photo.
    pub fn set_avatar(&self) {
        let client = self.client_id.get();
        let urls = self.avatar_pick.get();
        let Some(url) = urls.first() else { return };
        if client == 0 {
            return;
        }
        let path = url.path().to_string();
        if path.is_empty() {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let ok = functions::set_profile_photo(
                enums::InputChatPhoto::Static(types::InputChatPhotoStatic {
                    photo: enums::InputFile::Local(types::InputFileLocal { path }),
                }),
                false,
                client,
            )
            .await
            .is_ok();
            store.avatar_pick.set(Vec::new());
            store
                .profile_note
                .set_from(if ok { "Avatar updated" } else { "Avatar upload failed" });
        })
        .detach();
    }

    /// Resend a message whose delivery failed (tap the ✗ indicator).
    pub fn resend_failed(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let mut list = self.messages.get();
        if let Some(r) = list.iter_mut().find(|r| r.id == message_id) {
            r.failed = false;
            r.pending = true;
            self.set_messages(list);
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ =
                functions::resend_messages(chat_id, vec![message_id], None, 0, client)
                    .await;
        })
        .detach();
    }

    /// Open the two-step-verification management sheet.
    pub fn open_twofa(&self) {
        self.twofa_note.set_from("");
        self.twofa_open.set(true);
    }

    /// Apply 2FA changes: set/change/disable password + recovery email.
    /// TDLib computes the SRP exchange internally — the client only sends
    /// the plaintext over the encrypted MTProto channel.
    pub fn save_twofa(&self) {
        let old = self.twofa_old.get().expose().to_string();
        let new = self.twofa_new.get().expose().to_string();
        if old.is_empty() && new.is_empty() {
            self.twofa_note
                .set_from("Enter your current password to disable 2FA");
            return;
        }
        let hint = self.twofa_hint_in.get().to_string();
        let email = self.twofa_email.get().to_string();
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            match functions::set_password(
                old,
                new,
                hint,
                !email.is_empty(),
                email,
                client,
            )
            .await
            {
                Ok(enums::PasswordState::PasswordState(p)) => {
                    store
                        .twofa
                        .set_from(if p.has_password { "On" } else { "Off" });
                    store.twofa_note.set_from("Saved");
                    store.twofa_open.set(false);
                }
                Err(e) => store.twofa_note.set_from(e.message),
            }
        })
        .detach();
    }

    /// Open the contact picker to add a per-user exception to a privacy
    /// setting (`allow`=Always allow, `false`=Never allow).
    pub fn open_privacy_exception(&self, key: enums::UserPrivacySetting, allow: bool) {
        *self.privacy_target.borrow_mut() = Some((key, allow));
        self.load_contacts();
        self.privacy_picker_open.set(true);
    }

    /// Merge the picked user into Allow/RestrictUsers rules and push.
    pub fn pick_privacy_exception(&self, user_id: i64) {
        let Some((key, allow)) = self.privacy_target.borrow_mut().take() else {
            return;
        };
        self.privacy_picker_open.set(false);
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let Ok(enums::UserPrivacySettingRules::UserPrivacySettingRules(rs)) =
                functions::get_user_privacy_setting_rules(key.clone(), client)
                    .await
            else {
                return;
            };
            let rules = Self::merge_privacy_exception(rs.rules, user_id, allow);
            let _ = functions::set_user_privacy_setting_rules(
                key,
                types::UserPrivacySettingRules { rules },
                client,
            )
            .await;
            store.load_privacy();
        })
        .detach();
    }

    /// Pure merge: fold AllowUsers/RestrictUsers lists, drop the user from
    /// both, re-add on the requested side, keep user rules first (first
    /// matched rule wins in TDLib semantics).
    pub(crate) fn merge_privacy_exception(
        rules: Vec<enums::UserPrivacySettingRule>,
        user_id: i64,
        allow: bool,
    ) -> Vec<enums::UserPrivacySettingRule> {
        let mut allow_ids: Vec<i64> = Vec::new();
        let mut deny_ids: Vec<i64> = Vec::new();
        let mut out = Vec::new();
        for r in rules {
            match r {
                enums::UserPrivacySettingRule::AllowUsers(u) => {
                    allow_ids.extend(u.user_ids.into_iter().filter(|id| *id != user_id))
                }
                enums::UserPrivacySettingRule::RestrictUsers(u) => {
                    deny_ids.extend(u.user_ids.into_iter().filter(|id| *id != user_id))
                }
                other => out.push(other),
            }
        }
        if allow {
            allow_ids.push(user_id);
        } else {
            deny_ids.push(user_id);
        }
        if !deny_ids.is_empty() {
            out.insert(
                0,
                enums::UserPrivacySettingRule::RestrictUsers(
                    types::UserPrivacySettingRuleRestrictUsers {
                        user_ids: deny_ids,
                    },
                ),
            );
        }
        if !allow_ids.is_empty() {
            out.insert(
                0,
                enums::UserPrivacySettingRule::AllowUsers(
                    types::UserPrivacySettingRuleAllowUsers {
                        user_ids: allow_ids,
                    },
                ),
            );
        }
        out
    }

    /// Read-only privacy summary: who can see phone/photo/status, who can
    /// invite to chats. Rule evaluation is a simplified display mapping —
    /// AllowAll/RestrictAll dominate; AllowContacts (without an opposing
    /// Restrict rule) reads as "My contacts"; anything mixed reads
    /// "Custom" so the row never overstates access.
    pub fn load_privacy(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let keys: [(enums::UserPrivacySetting, &str); 4] = [
                (enums::UserPrivacySetting::ShowPhoneNumber, "Phone number"),
                (enums::UserPrivacySetting::ShowProfilePhoto, "Profile photo"),
                (enums::UserPrivacySetting::ShowStatus, "Last seen & online"),
                (enums::UserPrivacySetting::AllowChatInvites, "Group invites"),
            ];
            let mut rows = Vec::new();
            for (setting, label) in keys {
                if let Ok(enums::UserPrivacySettingRules::UserPrivacySettingRules(rs)) =
                    functions::get_user_privacy_setting_rules(
                        setting.clone(),
                        client,
                    )
                    .await
                {
                    rows.push(PrivacyRow {
                        setting: label.into(),
                        audience: privacy_audience(&rs.rules).into(),
                        key: setting,
                    });
                }
            }
            store.privacy_rows.set(rows);
        })
        .detach();
    }

    /// Load the account's active sessions for Settings.
    pub fn load_sessions(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Sessions::Sessions(s)) =
                functions::get_active_sessions(client).await
            {
                let rows: Vec<SessionRow> = s
                    .sessions
                    .iter()
                    .map(|sess| SessionRow {
                        id: sess.id,
                        title: format!(
                            "{} {} · {}",
                            sess.application_name, sess.application_version, sess.device_model
                        )
                        .into(),
                        subtitle: format!("{} · {} {}", sess.location, sess.ip_address, sess.platform)
                            .into(),
                        current: sess.is_current,
                    })
                    .collect();
                store.sessions.set(rows);
            }
        })
        .detach();
    }

    /// Terminate another device's session, then refresh the list.
    pub fn terminate_session_by_id(&self, id: i64) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::terminate_session(id, client).await;
            store.load_sessions();
        })
        .detach();
    }

    /// Read whether two-step verification is enabled.
    pub fn load_twofa(&self) {
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::PasswordState::PasswordState(p)) =
                functions::get_password_state(client).await
            {
                if p.has_password {
                    store.twofa.set_from("On");
                } else {
                    store.twofa.set_from("Off");
                }
            }
        })
        .detach();
    }

    /// Look up a localization key in the applied language pack; `n` selects
    /// the plural slot (zero/one/other — few/many collapse into `other`,
    /// which needs the pack's `plural_code` to disambiguate). Missing or
    /// deleted keys fall back to the app's English literal.
    pub fn tr(&self, key: &str, n: i64, fallback: &str) -> Str {
        use enums::LanguagePackStringValue as V;
        let pick = |p: &types::LanguagePackStringValuePluralized| {
            let out = match n {
                0 if !p.zero_value.is_empty() => &p.zero_value,
                1 if !p.one_value.is_empty() => &p.one_value,
                _ => &p.other_value,
            };
            out.clone()
        };
        match self.lang_strings.get().get(key) {
            Some(V::Ordinary(o)) => Str::from(o.value.clone()),
            Some(V::Pluralized(p)) => Str::from(pick(p)),
            _ => Str::from(fallback.to_string()),
        }
    }

    /// Fetch pack strings for `id` into `lang_strings` and mark it applied.
    async fn refresh_language(&self, id: &str, client: i32) {
        let id = if id.is_empty() { "en" } else { id };
        if client != 0 {
            let _ = functions::synchronize_language_pack(id.into(), client).await;
            if let Ok(enums::LanguagePackStrings::LanguagePackStrings(pack)) =
                functions::get_language_pack_strings(id.into(), vec![], client)
                    .await
            {
                let mut map = HashMap::new();
                for s in pack.strings {
                    if let Some(v) = s.value {
                        map.insert(s.key, v);
                    }
                }
                self.lang_strings.set(map);
            }
        }
        self.lang_id.set_from(id.to_string());
    }

    /// List available language packs + load the applied pack's strings.
    /// Called after login; `Update::LanguagePackStrings` keeps the map fresh.
    pub fn load_language_packs(&self) {
        let client = self.client_id.get();
        if client == 0 {
            // Demo mode: representative packs so Settings → Language renders.
            self.lang_packs.set(vec![
                LangRow {
                    id: "en".into(),
                    name: "English".into(),
                    beta: false,
                    active: true,
                },
                LangRow {
                    id: "zh-hans-raw".into(),
                    name: "简体中文 — Chinese (Simplified)".into(),
                    beta: false,
                    active: false,
                },
                LangRow {
                    id: "de".into(),
                    name: "Deutsch — German".into(),
                    beta: true,
                    active: false,
                },
            ]);
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            let mut current = store.lang_id.get().to_string();
            if let Ok(enums::OptionValue::String(s)) =
                functions::get_option("language_pack_id".into(), client).await
            {
                current = s.value.clone();
            }
            store.refresh_language(&current, client).await;
            if let Ok(enums::LocalizationTargetInfo::LocalizationTargetInfo(
                info,
            )) = functions::get_localization_target_info(false, client).await
            {
                store.lang_packs.set(
                    info.language_packs
                        .iter()
                        .map(|p| LangRow {
                            id: p.id.clone().into(),
                            name: format!("{} — {}", p.native_name, p.name)
                                .into(),
                            beta: p.is_beta,
                            active: p.id.as_str() == current,
                        })
                        .collect(),
                );
            }
        })
        .detach();
    }

    /// Switch the UI language: `setOption language_pack_id`, then fetch the
    /// pack's strings. Telegram applies on restart; views rebuild lazily.
    pub fn apply_language(&self, id: &str) {
        let id = id.to_string();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if client != 0 {
                let _ = functions::set_option(
                    "language_pack_id".into(),
                    Some(enums::OptionValue::String(types::OptionValueString {
                        value: id.clone(),
                    })),
                    client,
                )
                .await;
            }
            store.refresh_language(&id, client).await;
            let mut packs = store.lang_packs.get();
            for p in packs.iter_mut() {
                p.active = p.id.as_str() == id;
            }
            store.lang_packs.set(packs);
        })
        .detach();
    }

    /// Merge pushed `Update::LanguagePackStrings` for the applied pack.
    fn merge_language_strings(&self, u: &types::UpdateLanguagePackStrings) {
        if u.language_pack_id != self.lang_id.get().as_str() {
            return;
        }
        let mut map = self.lang_strings.get();
        for s in &u.strings {
            match &s.value {
                Some(v) => {
                    map.insert(s.key.clone(), v.clone());
                }
                None => {
                    map.remove(&s.key);
                }
            }
        }
        self.lang_strings.set(map);
    }

    /// Save profile fields to the account.
    pub fn save_profile(&self) {
        let client = self.client_id.get();
        let (first, last, bio, uname) = (
            self.edit_first.get().to_string(),
            self.edit_last.get().to_string(),
            self.edit_bio.get().to_string(),
            self.edit_username.get().to_string(),
        );
        let store = self.clone();
        spawn_local(async move {
            let mut note = "Saved";
            if functions::set_name(first.clone(), last.clone(), client)
                .await
                .is_err()
            {
                note = "Failed to update name";
            } else if functions::set_bio(bio, client).await.is_err() {
                note = "Failed to update bio";
            } else if functions::set_username(uname.clone(), client)
                .await
                .is_err()
            {
                note = "Failed to update username";
            } else {
                let mut me = store.me.get();
                me.name = format!("{first} {last}").trim().to_string().into();
                me.username = uname.into();
                store.me.set(me);
            }
            store.profile_note.set_from(note);
        })
        .detach();
    }

    /// Rename the open group/channel (admin).
    pub fn rename_chat(&self) {
        let chat_id = self.open_chat.get();
        let title = self.admin_title.get().to_string();
        if chat_id == 0 || title.is_empty() {
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_chat_title(chat_id, title, client).await;
        })
        .detach();
    }

    /// Set the open group/channel's description (admin).
    pub fn set_chat_desc(&self) {
        let chat_id = self.open_chat.get();
        let desc = self.admin_desc.get().to_string();
        if chat_id == 0 {
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_chat_description(chat_id, desc, client).await;
        })
        .detach();
    }

    /// Block/unblock a member (main block list).
    pub fn toggle_block(&self, member: &MemberRow, block: bool) {
        let sender = member.sender.clone();
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_message_sender_block_list(
                sender,
                if block {
                    Some(enums::BlockList::Main)
                } else {
                    None
                },
                client,
            )
            .await;
        })
        .detach();
    }

    /// Log out every session except the current one.
    pub fn terminate_all_sessions(&self) {
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::terminate_all_other_sessions(client).await;
            store.load_sessions();
        })
        .detach();
    }

    /// Ban a member (basic admin op) then refresh the list.
    pub fn kick_member(&self, member: &MemberRow) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        let sender = member.sender.clone();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let _ = functions::set_chat_member_status(
                chat_id,
                sender,
                enums::ChatMemberStatus::Banned(types::ChatMemberStatusBanned {
                    banned_until_date: 0,
                }),
                client,
            )
            .await;
            store.load_members();
        })
        .detach();
    }

    pub fn run_search(&self, query: Str) {
        if query.is_empty() {
            self.server_results.set(Vec::new());
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Chats::Chats(c)) =
                functions::search_chats_on_server(query.to_string(), 20, client).await
            {
                let mut found = Vec::new();
                for id in c.chat_ids {
                    if let Ok(enums::Chat::Chat(chat)) =
                        functions::get_chat(id, client).await
                    {
                        found.push(chat);
                    }
                }
                for chat in found {
                    store.upsert_result_row(chat);
                }
            }
        })
        .detach();
    }

    fn upsert_result_row(&self, chat: types::Chat) {
        if self.chats.get().iter().any(|r| r.id == chat.id) {
            return;
        }
        self.chat_objs.borrow_mut().insert(chat.id, chat.clone());
        if let Some(photo) = &chat.photo {
            self.want_file(&photo.small);
        }
        let row = ChatRow {
            id: chat.id,
            title: chat.title.clone().into(),
            preview: "".into(),
            draft: "".into(),
            order: 0,
            unread: 0,
            pinned: false,
            muted: false,
            marked_unread: false,
            in_archive: position_in(&chat, &enums::ChatList::Archive).is_some(),
            photo_file: chat.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
            time: "".into(),
            typing: false,
            online: false,
            kind_icon: "person".into(),
        };
        let mut list = self.server_results.get();
        if !list.iter().any(|r| r.id == row.id) {
            list.push(row);
            self.server_results.set(list);
        }
    }

    /// Create (or open) a chat from the New Chat page. `kind`: 0 private
    /// (input = @username or user id), 1 group (title), 2 channel (title).
    pub fn create_chat(&self) {
        let input = self.new_chat_input.get().to_string();
        let kind = self.new_chat_kind.get();
        if input.trim().is_empty() {
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            match kind {
                0 => {
                    let mut uname = input.trim().to_string();
                    if uname.starts_with('@') {
                        uname.remove(0);
                    }
                    let mut user_id: i64 = uname.parse().unwrap_or(0);
                    if user_id == 0
                        && let Ok(enums::Chats::Chats(c)) =
                            functions::search_chats_on_server(format!("@{uname}"), 5, client)
                                .await
                    {
                        for cid in c.chat_ids {
                            if let Ok(enums::Chat::Chat(chat)) =
                                functions::get_chat(cid, client).await
                                && let enums::ChatType::Private(p) = chat.r#type
                            {
                                user_id = p.user_id;
                                break;
                            }
                        }
                    }
                    if user_id != 0
                        && let Ok(enums::Chat::Chat(chat)) =
                            functions::create_private_chat(user_id, false, client)
                                .await
                    {
                        store.select_chat(chat.id);
                    }
                }
                1 => {
                    if let Ok(enums::Chat::Chat(chat)) =
                        functions::create_new_supergroup_chat(
                            input,
                            false,
                            false,
                            String::new(),
                            None,
                            0,
                            false,
                            client,
                        )
                        .await
                    {
                        store.select_chat(chat.id);
                    }
                }
                _ => {
                    if let Ok(enums::Chat::Chat(chat)) =
                        functions::create_new_supergroup_chat(
                            input,
                            false,
                            true,
                            String::new(),
                            None,
                            0,
                            false,
                            client,
                        )
                        .await
                    {
                        store.select_chat(chat.id);
                    }
                }
            }
        })
        .detach();
    }
}
