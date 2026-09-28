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
use waterui::theme::color::{
    AccentForeground, Foreground, SelectionForeground, TertiaryContainer,
};
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

/// One row in the pinned-messages popup.
#[derive(Clone, Identifiable)]
pub struct PinnedRow {
    #[id]
    pub id: i64,
    pub label: Str,
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
    /// TDLib `unread_mention_count` — renders the `@` badge on the row.
    pub unread_mentions: i32,
    /// TDLib `unread_reaction_count` — renders the `❤` badge on the row
    /// and drives the floating ❤ jump button in the open chat.
    pub unread_reactions: i32,
    /// `message_auto_delete_time` in seconds (0 = off) — a clock icon on
    /// the row like Telegram Desktop's auto-delete indicator.
    pub auto_delete: i32,
    pub pinned: bool,
    pub muted: bool,
    pub marked_unread: bool,
    /// Has a position in the Archive list.
    pub in_archive: bool,
    /// TDLib `ChatList::Folder` membership (0 = no user folder).
    pub folder_id: i32,
    pub photo_file: i32,
    /// TDLib `accent_color_id` for the peer (−1 = none) — seeds the
    /// userpic color slot.
    pub accent: i32,
    pub time: Str,
    pub typing: bool,
    pub online: bool,
    pub kind_icon: Str,
    /// r35: sidebar-search highlight variants (empty = render the plain
    /// `title`/`preview`). Built in the filtered map, not on the wire path.
    pub title_styled: StyledStr,
    pub preview_styled: StyledStr,
    /// `chatActionBar*` kind for the panel above the composer
    /// (""/report_spam/report_add_block/add_contact/share_phone/
    /// invite_members/join_request); empty = normal composer.
    pub action_bar: Str,
    /// Aux label for the bar (join-request chat title).
    pub action_title: Str,
    /// Private-chat peer user id — the action bar's add-contact and
    /// share-phone calls need it.
    pub peer_user: i64,
}

/// Style metadata isn't `PartialEq`, so styled fields compare by plain text
/// and chunk count — enough for the empty↔highlighted transitions we produce.
fn styled_row_eq(a: &StyledStr, b: &StyledStr) -> bool {
    a.to_plain() == b.to_plain() && a.chunks().len() == b.chunks().len()
}

// Structural equality: the list diff skips rows that compare equal, so an
// id-only `eq` would freeze every field update (unread badges, typing, …).
impl PartialEq for ChatRow {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
            && self.title == o.title
            && self.preview == o.preview
            && self.draft == o.draft
            && self.order == o.order
            && self.unread == o.unread
            && self.unread_mentions == o.unread_mentions
            && self.unread_reactions == o.unread_reactions
            && self.auto_delete == o.auto_delete
            && self.pinned == o.pinned
            && self.muted == o.muted
            && self.marked_unread == o.marked_unread
            && self.in_archive == o.in_archive
            && self.folder_id == o.folder_id
            && self.photo_file == o.photo_file
            && self.accent == o.accent
            && self.time == o.time
            && self.typing == o.typing
            && self.online == o.online
            && self.kind_icon == o.kind_icon
            && styled_row_eq(&self.title_styled, &o.title_styled)
            && styled_row_eq(&self.preview_styled, &o.preview_styled)
            && self.action_bar == o.action_bar
            && self.action_title == o.action_title
            && self.peer_user == o.peer_user
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

/// One emoji on a message's reaction strip — emoji, total count, and
/// whether the current user chose it (Desktop highlights the chosen pill).
#[derive(Clone, PartialEq)]
pub struct ReactionChip {
    pub emoji: Str,
    pub count: i32,
    pub chosen: bool,
}

#[derive(Clone, Identifiable)]
pub struct MessageRow {
    #[id]
    pub id: i64,
    pub sender: Str,
    /// Sender's TDLib `accent_color_id` (−1 = none) — seeds the per-peer
    /// color slot on the group sender name and avatar circle.
    pub sender_accent: i32,
    pub text: Str,
    pub time: Str,
    pub outgoing: bool,
    /// Outgoing message the peer has read (double check).
    pub read_out: bool,
    pub can_edit: bool,
    pub reply_excerpt: Str,
    /// Id of the message this row replies to (0 = none); the quote taps jump to it.
    pub reply_to_id: i64,
    pub media_file: i32,
    /// Playable media file id (video/voice/audio/animation payload, as
    /// opposed to `media_file` which may hold a thumbnail).
    pub play_file: i32,
    pub media_label: Str,
    /// Media duration in seconds for playable payloads (video/animation);
    /// renders as the m:ss corner badge on the thumbnail, as in Telegram
    /// Desktop.
    pub media_secs: i32,
    pub reaction_chips: Vec<ReactionChip>,
    /// Emoji the current user has chosen on this message, if any.
    pub my_reaction: Str,
    /// Rich-text body; empty when the message has no formatting entities.
    /// For spoiler rows this is the MASKED variant (spoiler spans colored
    /// like the bubble fill); `styled_open` holds the revealed text.
    pub styled: StyledStr,
    /// Unmasked styled body for spoiler rows after the user taps to
    /// reveal; empty otherwise.
    pub styled_open: StyledStr,
    /// The message carries spoiler entities (masked spans).
    pub has_spoiler: bool,
    /// Link-preview card: site name, title and description (empty = none).
    pub link_site: Str,
    pub link_title: Str,
    pub link_desc: Str,
    /// "Forwarded from X" attribution, empty when not forwarded.
    pub forwarded_from: Str,
    /// Sender peer for the avatar/name → profile tap: `sender_user` is a
    /// TDLib user id, `sender_chat` a chat id for channel posts (0 = none).
    pub sender_user: i64,
    pub sender_chat: i64,
    /// Forwarded-origin peer for the badge → source profile tap
    /// (0 = none / hidden sender).
    pub fwd_user: i64,
    pub fwd_chat: i64,
    /// URL the link-preview card opens when tapped (`link_preview.url`,
    /// else the first URL/TextUrl entity); empty = card not tappable.
    pub link_url: Str,
    pub failed: bool,
    pub pending: bool,
    /// Flash-highlighted after a jump-to-message.
    pub highlighted: bool,
    /// In-chat search: this row matched the query; `search_styled` is the
    /// body with every occurrence highlighted (span `background`).
    pub search_hit: bool,
    pub search_styled: StyledStr,
    /// The message mentions the current user — drives the floating `@`
    /// jump button over the chat (Telegram Desktop parity).
    pub mentions_me: bool,
    /// Render the "Unread messages" divider above this row (first incoming
    /// message after `last_read_inbox_message_id`).
    pub unread_divider: bool,
    /// First message carrying unread reactions — the floating ❤ button
    /// jumps here (set from `updateMessageUnreadReactions` / demo seed).
    pub unread_reaction: bool,
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
    /// First row of a same-sender run (Desktop message grouping): the
    /// sender name shows only on it.
    pub group_first: bool,
    /// Last row of a same-sender run — in groups it carries the avatar.
    pub group_last: bool,
    /// Incoming row in a group/channel: reserve the avatar column so the
    /// run's bubbles stay aligned.
    pub avatar_col: bool,
    /// Paint the sender avatar circle (`avatar_col` on the run's last row).
    pub show_avatar: bool,
    /// Sender profile-photo small file id (0 → initials).
    pub sender_photo: i32,
    /// Service row (pin/join/etc.) — renders as a centered label, not a
    /// bubble; excluded from sender runs and multi-select. Kept as its own
    /// list row (the r30 fold was reverted — DOGFOOD r31-1).
    pub is_service: bool,
    /// Channel post footer: interaction_info view count (0 = none; user
    /// posts have no view count — `getMessageViewCount` is channel-only).
    pub view_count: i32,
    /// Post author signature (`author_signature`) on channel posts.
    pub author_sig: Str,
    /// Album this message belongs to (`media_album_id`, 0 = none).
    /// Consecutive rows with the same id merge into one album bubble at
    /// `set_messages`; the merged row keeps the first member's id and
    /// carries every member's media file in `album_files`.
    pub album_id: i64,
    /// Media file ids of a merged album (non-empty only on album rows).
    pub album_files: Vec<i32>,
    /// Inline keyboard rows (`replyMarkupInlineKeyboard`) rendered as
    /// button pills under the bubble content.
    pub kb_rows: Vec<Vec<KbBtn>>,
}

impl MessageRow {
    /// Post footer text: "👁 1.2K  ·  Alice Liddell" — Desktop draws the
    /// view count before the author signature in the bubble's meta line.
    pub fn post_footer(&self) -> Str {
        let mut s = String::new();
        if self.view_count > 0 {
            s.push_str("👁 ");
            s.push_str(&fmt_count(self.view_count));
        }
        if !self.author_sig.is_empty() {
            if !s.is_empty() {
                s.push_str("  ·  ");
            }
            s.push_str(&self.author_sig);
        }
        Str::from(s)
    }
}

/// A single poll answer option as shown inside a poll bubble.
#[derive(Clone, PartialEq)]
pub struct PollOptRow {
    /// Option index (the `option_ids` value `setPollAnswer` expects).
    pub ix: usize,
    pub text: Str,
    pub pct: i32,
    pub chosen: bool,
}

/// Renderable form of `messagePoll`.
#[derive(Clone, PartialEq)]
pub struct PollRow {
    pub question: Str,
    pub options: Vec<PollOptRow>,
    pub voters: i32,
    pub closed: bool,
}

/// What an inline-keyboard tap does (`inlineKeyboardButtonType*`).
/// Telegram Desktop draws every kind; Watergram dispatches the ones with
/// a usable effect and toasts the rest as unsupported.
#[derive(Clone, PartialEq)]
pub enum KbKind {
    /// `inlineKeyboardButtonTypeUrl` — open in the browser.
    Url(Str),
    /// `inlineKeyboardButtonTypeCallback` — answer the bot via
    /// `getCallbackQueryAnswer`; its `text`/`url` drive the toast/open.
    Callback(Str),
    /// `inlineKeyboardButtonTypeCopyText` — clipboard + "Copied" toast.
    Copy(Str),
    /// `inlineKeyboardButtonTypeSwitchInline` — seed the composer.
    SwitchInline(Str),
    /// `inlineKeyboardButtonTypeUser` — open the user's profile.
    User(i64),
    /// Button kinds without a client-side action yet (LoginUrl, WebApp,
    /// CallbackWithPassword, CallbackGame, Buy, …).
    Unsupported,
}

/// One cell of a bot's inline keyboard (`replyMarkupInlineKeyboard` row).
#[derive(Clone, PartialEq)]
pub struct KbBtn {
    pub text: Str,
    pub kind: KbKind,
}

/// The "Show message info" card contents (TDLib `getMessageReadDate` /
/// `getMessageViewers` on real accounts, synthesized in demo mode).
#[derive(Clone, PartialEq)]
pub struct MsgInfo {
    /// Sender display name — the card title.
    pub from: Str,
    /// "Sent <day> <time>".
    pub sent: Str,
    /// "Read <time>" / "Unread" / privacy-restricted label; empty while
    /// loading or when the row is incoming.
    pub read: Str,
    /// "N views" for channel posts; empty otherwise.
    pub views: Str,
    /// "Seen by" names for group posts (Desktop's viewer list).
    pub seen: Vec<Str>,
}

/// A day in the jump-to-date popup (loaded days locally,
/// `getChatMessageCalendar` days on a real session).
#[derive(Clone, Identifiable)]
pub struct DayRow {
    #[id]
    pub day: i64,
    pub label: Str,
    /// Message count on that day (0 = unknown/local listing).
    pub count: i32,
}

/// One row of the info panel's shared Files / Links tabs.
#[derive(Clone, Identifiable)]
pub struct SharedLinkRow {
    #[id]
    pub id: i64,
    pub title: Str,
    pub detail: Str,
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

/// One `:shortcode` emoji-autocomplete suggestion row.
#[derive(Clone, Identifiable)]
pub struct EmojiSug {
    /// Shortcode without colons (row key).
    #[id]
    pub name: Str,
    pub emoji: Str,
}

/// Static `name → emoji` table for `:token` autocomplete (Desktop queries
/// the server emoji index; a local set covers the common shortcodes).
pub(crate) const EMOJI_SHORTCODES: &[(&str, &str)] = &[
    ("smile", "😄"), ("smiley", "😃"), ("grin", "😁"), ("joy", "😂"),
    ("rofl", "🤣"), ("wink", "😉"), ("blush", "😊"), ("yum", "😋"),
    ("sunglasses", "😎"), ("heart_eyes", "😍"), ("kissing_heart", "😘"),
    ("thinking", "🤔"), ("neutral_face", "😐"), ("expressionless", "😑"),
    ("roll_eyes", "🙄"), ("smirk", "😏"), ("persevere", "😣"),
    ("open_mouth", "😮"), ("zipper_mouth", "🤐"), ("hushed", "😯"),
    ("sleepy", "😪"), ("tired_face", "😫"), ("sleeping", "😴"),
    ("relieved", "😌"), ("stuck_out_tongue", "😛"), ("cry", "😢"),
    ("sob", "😭"), ("scream", "😱"), ("triumph", "😤"), ("pensive", "😔"),
    ("confused", "😕"), ("upside_down", "🙃"), ("money_mouth", "🤑"),
    ("angry", "😠"), ("rage", "😡"), ("mask", "😷"), ("sneezing", "🤧"),
    ("innocent", "😇"), ("clown", "🤡"), ("skull", "💀"), ("ghost", "👻"),
    ("alien", "👽"), ("robot", "🤖"), ("poop", "💩"),
    ("thumbsup", "👍"), ("thumbsdown", "👎"), ("clap", "👏"),
    ("raised_hands", "🙌"), ("open_hands", "👐"), ("handshake", "🤝"),
    ("pray", "🙏"), ("v", "✌️"), ("ok_hand", "👌"), ("wave", "👋"),
    ("muscle", "💪"), ("writing_hand", "✍️"),
    ("heart", "❤️"), ("orange_heart", "🧡"), ("yellow_heart", "💛"),
    ("green_heart", "💚"), ("blue_heart", "💙"), ("purple_heart", "💜"),
    ("black_heart", "🖤"), ("broken_heart", "💔"), ("two_hearts", "💕"),
    ("fire", "🔥"), ("sparkles", "✨"), ("star", "⭐"), ("boom", "💥"),
    ("100", "💯"), ("white_check_mark", "✅"), ("x", "❌"),
    ("warning", "⚠️"), ("tada", "🎉"), ("confetti_ball", "🎊"),
    ("gift", "🎁"), ("balloon", "🎈"), ("birthday", "🎂"),
    ("trophy", "🏆"), ("soccer", "⚽"), ("basketball", "🏀"),
    ("dart", "🎯"), ("video_game", "🎮"), ("game_die", "🎲"),
    ("car", "🚗"), ("airplane", "✈️"), ("rocket", "🚀"), ("house", "🏠"),
    ("iphone", "📱"), ("computer", "💻"), ("watch", "⌚"),
    ("camera", "📷"), ("lock", "🔒"), ("key", "🔑"), ("bulb", "💡"),
    ("pushpin", "📌"), ("pencil", "✏️"), ("memo", "📝"), ("book", "📖"),
    ("mag", "🔍"), ("moneybag", "💰"), ("coffee", "☕"), ("pizza", "🍕"),
    ("hamburger", "🍔"), ("apple", "🍎"), ("beer", "🍺"),
    ("champagne", "🥂"), ("alarm_clock", "⏰"), ("calendar", "📅"),
    ("earth_africa", "🌍"), ("moon", "🌙"), ("sunny", "☀️"),
    ("rainbow", "🌈"), ("umbrella", "☔"), ("snowflake", "❄️"),
    ("zap", "⚡"), ("cat", "🐱"), ("dog", "🐶"), ("mouse", "🐭"),
    ("rabbit", "🐰"), ("fox", "🦊"), ("bear", "🐻"), ("panda", "🐼"),
    ("koala", "🐨"), ("tiger", "🐯"), ("lion", "🦁"), ("cow", "🐮"),
    ("pig", "🐷"), ("frog", "🐸"), ("monkey_face", "🐵"),
    ("chicken", "🐔"), ("penguin", "🐧"), ("bird", "🐦"),
    ("unicorn", "🦄"), ("bee", "🐝"), ("bug", "🐛"), ("butterfly", "🦋"),
    ("snail", "🐌"), ("turtle", "🐢"), ("snake", "🐍"), ("octopus", "🐙"),
    ("squid", "🦑"), ("shrimp", "🦐"), ("crab", "🦀"), ("fish", "🐟"),
    ("dolphin", "🐬"), ("whale", "🐳"), ("shark", "🦈"),
    ("crocodile", "🐊"), ("zebra", "🦓"), ("gorilla", "🦍"),
    ("elephant", "🐘"), ("camel", "🐪"), ("giraffe", "🦒"),
    ("horse", "🐎"), ("sheep", "🐑"), ("deer", "🦌"),
];

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
    /// Source message id — the ‹ › controls step through the chat's other
    /// media messages from here.
    pub msg_id: i64,
    pub caption: Str,
    pub from: Str,
}

// Structural equality — see `ChatRow`'s `PartialEq` for why id-only would
// freeze live row updates (reaction chips, read marks, spoiler reveal, …).
impl PartialEq for MessageRow {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
            && self.sender == o.sender
            && self.sender_accent == o.sender_accent
            && self.text == o.text
            && self.time == o.time
            && self.outgoing == o.outgoing
            && self.read_out == o.read_out
            && self.can_edit == o.can_edit
            && self.reply_excerpt == o.reply_excerpt
            && self.reply_to_id == o.reply_to_id
            && self.media_file == o.media_file
            && self.play_file == o.play_file
            && self.media_label == o.media_label
            && self.media_secs == o.media_secs
            && self.reaction_chips == o.reaction_chips
            && self.my_reaction == o.my_reaction
            && styled_row_eq(&self.styled, &o.styled)
            && styled_row_eq(&self.styled_open, &o.styled_open)
            && self.has_spoiler == o.has_spoiler
            && self.link_site == o.link_site
            && self.link_title == o.link_title
            && self.link_desc == o.link_desc
            && self.forwarded_from == o.forwarded_from
            && self.sender_user == o.sender_user
            && self.sender_chat == o.sender_chat
            && self.fwd_user == o.fwd_user
            && self.fwd_chat == o.fwd_chat
            && self.link_url == o.link_url
            && self.failed == o.failed
            && self.pending == o.pending
            && self.highlighted == o.highlighted
            && self.search_hit == o.search_hit
            && styled_row_eq(&self.search_styled, &o.search_styled)
            && self.mentions_me == o.mentions_me
            && self.unread_divider == o.unread_divider
            && self.unread_reaction == o.unread_reaction
            && self.day == o.day
            && self.day_header == o.day_header
            && self.day_label == o.day_label
            && self.edited == o.edited
            && self.poll == o.poll
            && self.group_first == o.group_first
            && self.group_last == o.group_last
            && self.avatar_col == o.avatar_col
            && self.show_avatar == o.show_avatar
            && self.sender_photo == o.sender_photo
            && self.is_service == o.is_service
            && self.view_count == o.view_count
            && self.author_sig == o.author_sig
            && self.album_id == o.album_id
            && self.album_files == o.album_files
            && self.kb_rows == o.kb_rows
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
    /// Sidebar search "Messages" section (`searchMessages` results, or the
    /// demo corpus on a clientless store).
    pub msg_results: Binding<Vec<MsgHit>>,
    /// Destructive-delete confirmation card: the ids pending deletion and
    /// the revoke checkbox's current value.
    pub confirm_delete: Binding<Option<DeleteAsk>>,
    pub delete_revoke: Binding<bool>,
    /// Unread-chat count per sidebar list (TDLib `updateUnreadChatCount`):
    /// key 0 = Main (All), -1 = Archive, n = folder id.
    pub folder_unreads: Binding<HashMap<i32, i32>>,
    /// Demo store: full chat roster `set_list` filters (chat_rows live in
    /// `chats`; `chat_objs` stays empty without TDLib).
    pub demo_roster: Rc<RefCell<Vec<ChatRow>>>,
    /// Demo global-search corpus: (chat_id, message_id, sender, text).
    /// Message ids are real `demo_conversation` ids so a hit's jump lands
    /// the highlight on the seeded row.
    pub demo_corpus: Rc<RefCell<Vec<DemoCorpusEntry>>>,
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
    /// Composer Escape dismissed the completion popup; re-arms on the next
    /// edit (Telegram Desktop's dismiss-until-next-trigger behaviour).
    pub completion_off: Binding<bool>,
    /// (from_chat_id, message_id) of the message being forwarded.
    pub forward_message: Binding<Option<(i64, i64)>>,
    /// Message ids whose spoiler spans have been revealed by tap.
    pub revealed_spoilers: Binding<Vec<i64>>,
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
    /// All pinned messages (most recent first) driving the banner counter
    /// and the pinned-messages popup; `pinned_id`/`pinned_label` track the
    /// banner's current entry (tap cycles through them, Desktop-style).
    pub pinned_msgs: Binding<Vec<PinnedRow>>,
    /// Index into `pinned_msgs` the banner is showing.
    pub pinned_idx: Rc<Cell<usize>>,
    /// The pinned-messages list popup under the banner.
    pub pinned_popup: Binding<bool>,
    /// Jump-to-date popup open; `jump_days` is the popup's day list.
    pub jump_date_open: Binding<bool>,
    pub jump_days: Binding<Vec<DayRow>>,
    /// Info panel "Shared" tab: 0=Media, 1=Files, 2=Links.
    pub shared_tab: Binding<usize>,
    /// Rows for the Files and Links shared-content tabs
    /// (msg id + display strings; tap opens the message).
    pub shared_files: Binding<Vec<SharedLinkRow>>,
    pub shared_links: Binding<Vec<SharedLinkRow>>,
    /// In-chat message search state.
    pub chat_search_open: Binding<bool>,
    pub chat_search: Binding<Str>,
    pub chat_search_results: Binding<Vec<MessageRow>>,
    /// In-chat search: matched message ids in row order plus the index the
    /// "n/N" counter sits on — both drive the prev/next match buttons.
    pub chat_match_ids: Binding<Vec<i64>>,
    pub chat_search_pos: Binding<usize>,
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
    /// Last URL `open_link` handed to the system browser (observability
    /// for tests — headless runs have no browser to prove the open).
    pub link_opened: Binding<Str>,
    /// Picked file for the own-avatar upload on Settings.
    pub avatar_pick: Binding<Vec<Url>>,
    /// Chat avatar picker (group admin section).
    pub chat_avatar_pick: Binding<Vec<Url>>,
    /// Per-message translation results (message id → translated text);
    /// the bubble swaps the body for the translation while present.
    pub translated: Binding<std::collections::BTreeMap<i64, Str>>,
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
    /// Transient toast text the root view routes into `SnackbarManager`
    /// (seq + message; the seq makes identical messages re-fire).
    pub notice: Binding<(u64, Str)>,
    /// Message multi-selection (batch forward/delete).
    pub selected_msgs: Binding<Vec<i64>>,
    /// Folder editor sheet state.
    pub folder_open: Binding<bool>,
    pub folder_name: Binding<Str>,
    pub folder_contacts: Binding<bool>,
    pub folder_groups: Binding<bool>,
    pub folder_channels: Binding<bool>,
    /// Folder editor: the chats the folder explicitly includes
    /// (`ChatFolder.included_chat_ids`) — the picker's checked set.
    pub folder_chats: Binding<Vec<i64>>,
    /// Folder being edited; 0 = creating a new one.
    pub editing_folder: Binding<i32>,
    /// "Show message info" card payload; None = hidden.
    pub msg_info: Binding<Option<MsgInfo>>,
    /// Sidebar global-search filter tab (0 All / 1 Chats / 2 Media /
    /// 3 Files / 4 Links) — mirrors Desktop's tab row under the field.
    pub search_filter: Binding<i32>,
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
    /// Unread chats in this list (TDLib `updateUnreadChatCount`), shown
    /// as a badge on the chip like Telegram Desktop's folder bar.
    pub unread: i32,
    /// Chat ids the folder explicitly includes (`included_chat_ids`) —
    /// seeded into the folder editor's chat picker.
    pub include: Vec<i64>,
}

/// One hit in the sidebar's global "Messages" search section
/// (`searchMessages`); tapping opens the chat and jumps to the message.
#[derive(Clone, Identifiable)]
pub struct MsgHit {
    /// Unique key: chat_id shifted, message ids repeat across chats.
    #[id]
    pub key: i64,
    pub chat_id: i64,
    pub message_id: i64,
    /// Chat title the hit belongs to.
    pub title: Str,
    /// Sender display name (empty for channel posts / outgoing).
    pub sender: Str,
    pub snippet: Str,
    pub time: Str,
    /// Hit kind for the sidebar's search filter tabs (0 text, 1 media,
    /// 2 file, 3 link). The TDLib path fills it from the message content.
    pub kind: i32,
}

/// One row in the sidebar search results pane. Local chat matches, the
/// "Global search results" section, message hits and the empty state all
/// flow through a single `SignalCollection` so a filter-tab switch is a
/// plain data update — a nested `when` inside the results pane never
/// re-evaluates on winit (DOGFOOD r41-2 / hydrolysis#251).
#[derive(Clone)]
pub enum SearchRow {
    /// Section label row ("Global search results" / "Messages").
    Header(Str),
    Chat(ChatRow),
    Hit(MsgHit),
    /// "No results" marker.
    Empty,
}

impl Identifiable for SearchRow {
    type Id = i64;
    fn id(&self) -> i64 {
        match self {
            SearchRow::Header(t) => {
                i64::MIN + 2 + i64::from(t.as_str().contains("Global"))
            }
            SearchRow::Empty => i64::MIN,
            // Hit keys are chat_id * 1_000_000 + msg_id — they never
            // collide with chat ids or the sentinels above.
            SearchRow::Chat(r) => r.id,
            SearchRow::Hit(h) => h.key,
        }
    }
}

/// A destructive message delete awaiting user confirmation — the card
/// shows the count and, for private chats, the "Also delete for <peer>"
/// checkbox that maps to `deleteMessages`' `revoke` flag.
#[derive(Clone, PartialEq)]
pub struct DeleteAsk {
    pub ids: Vec<i64>,
    /// Peer display name; empty = no revoke option offered.
    pub peer: Str,
}

/// (chat_id, msg_id, sender, text, kind) — the searchable demo message
/// set. kind drives the global-search filter tabs: 0 text, 1 media,
/// 2 file, 3 link (mirrors `SearchMessagesFilter`).
pub type DemoCorpusEntry = (i64, i64, Str, Str, i32);

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

/// Compact count for bubble footers: 999 → "999", 1234 → "1.2K",
/// 4_500_000 → "4.5M" (Telegram Desktop's one-decimal style).
fn fmt_count(n: i32) -> String {
    let n = n.max(0) as f64;
    if n < 1_000.0 {
        format!("{}", n as i64)
    } else if n < 1_000_000.0 {
        let v = n / 1_000.0;
        if v < 10.0 { format!("{v:.1}K") } else { format!("{}K", v.round() as i64) }
    } else {
        format!("{:.1}M", n / 1_000_000.0)
    }
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
/// `StyledStr`: bold/italic/underline/strike/mono, links in accent blue,
/// quotes on a light background. `mask`: spoiler chunks get `background =
/// mask` — a span background in the text's own color hides the glyphs
/// inside a solid block until revealed (Telegram Desktop). hydrolysis
/// drops per-span `background` until #207 lands, so the text shows
/// unmasked for now — see DOGFOOD r32-2. `None` leaves spoiler text
/// visible.
pub(crate) fn styled_from_formatted_mask(ft: &types::FormattedText, mask: Option<Color>) -> StyledStr {
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
        if spoiler
            && let Some(mask) = &mask
        {
            st = st.background((*mask).clone());
        }
        if quote {
            st = st.background(Srgb::try_from_hex("#E8E8E8").unwrap());
        }
        styled.push(seg.to_string(), st);
    }
    styled
}

/// Demo sender name → stable fake user id, so the avatar/name → profile
/// tap resolves in the seeded conversation (0 = outgoing/system).
fn demo_user_id(sender: &str) -> i64 {
    match sender {
        "Alice" => 101,
        "Bob" => 102,
        "Carol" => 103,
        _ => 0,
    }
}

/// Label for an auto-delete timeout in seconds (Desktop's "1 day" /
/// "1 week" / "1 month" ladder).
pub(crate) fn auto_delete_label(secs: i32) -> &'static str {
    match secs {
        86_400 => "1 day",
        604_800 => "1 week",
        2_592_000 => "1 month",
        _ => "custom",
    }
}

/// First link target a `FormattedText` points at: a `TextUrl`'s url, or a
/// `Url` entity sliced out of the text (entity offsets are UTF-16 units).
fn first_url_entity(ft: &types::FormattedText) -> Option<Str> {
    for e in &ft.entities {
        match &e.r#type {
            enums::TextEntityType::TextUrl(u) => {
                return Some(Str::from(u.url.clone()));
            }
            enums::TextEntityType::Url => {
                let units: Vec<u16> = ft.text.encode_utf16().collect();
                let s = e.offset.max(0) as usize;
                let t = (e.offset + e.length).max(0) as usize;
                if s < units.len() && t <= units.len() {
                    return Some(Str::from(String::from_utf16_lossy(&units[s..t])));
                }
            }
            _ => {}
        }
    }
    None
}

/// Convenience: entities rendered with spoiler spans masked by `mask`.
pub(crate) fn styled_from_formatted(ft: &types::FormattedText) -> StyledStr {
    styled_from_formatted_mask(ft, Some(Color::from(Srgb::BLACK)))
}

/// Return `base` with every case-insensitive occurrence of `q` marked with
/// the `hl_bg`/`hl_fg` span pair (r35: in-chat + sidebar search highlighting;
/// r36: TertiaryContainer+SelectionForeground — SelectionContainer resolves to
/// secondary_container, which is nearly identical to the incoming bubble's
/// fill and left the mark invisible; the tertiary hue contrasts both fills).
/// Byte-offset matching
/// runs on the lowercased copy, which is only safe while lowercasing
/// preserves byte length — otherwise fall back to a case-sensitive match so
/// offsets never mis-slice the original.
pub(crate) fn highlight_styled(
    base: &StyledStr,
    q: &str,
    hl_bg: Color,
    hl_fg: Color,
) -> StyledStr {
    if q.is_empty() {
        return base.clone();
    }
    let ql = q.to_lowercase();
    let mut out = StyledStr::empty();
    for (chunk, style) in base.chunks() {
        let text = chunk.as_str();
        let lower = text.to_lowercase();
        let (hay, needle) = if lower.len() == text.len() {
            (lower.as_str(), ql.as_str())
        } else {
            (text, q)
        };
        let mut at = 0usize;
        while let Some(p) = hay[at..].find(needle) {
            let (s, e) = (at + p, at + p + needle.len());
            if s > at {
                out.push(text[at..s].to_string(), style.clone());
            }
            let mut hit = style.clone();
            hit.background = Some(hl_bg.clone());
            hit.foreground = Some(hl_fg.clone());
            out.push(text[s..e].to_string(), hit);
            at = e;
        }
        if at < text.len() {
            out.push(text[at..].to_string(), style.clone());
        }
    }
    out
}

/// True when `ft` carries at least one spoiler entity.
pub(crate) fn has_spoiler_entity(ft: &types::FormattedText) -> bool {
    ft.entities
        .iter()
        .any(|e| matches!(e.r#type, enums::TextEntityType::Spoiler))
}

/// Telegram Desktop's composer markdown — `*b*` `_i_` `` `m` `` `~~s~~`
/// `||spoiler||` — parsed into `FormattedText` entities on send. Entity
/// offsets are UTF-16 code units (TDLib's contract). Non-nesting: the
/// earliest opening delimiter wins, unclosed/empty pairs stay literal.
pub(crate) fn parse_markdown(input: &str) -> types::FormattedText {
    const DELIMS: &[&str] = &["**", "~~", "||", "__", "*", "_", "`"];
    fn kind(d: &str) -> enums::TextEntityType {
        match d {
            "**" | "*" => enums::TextEntityType::Bold,
            "__" | "_" => enums::TextEntityType::Italic,
            "~~" => enums::TextEntityType::Strikethrough,
            "||" => enums::TextEntityType::Spoiler,
            _ => enums::TextEntityType::Code,
        }
    }
    let mut clean = String::with_capacity(input.len());
    let mut entities = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let Some(start) = rest.find(|c| "*_`~|".contains(c)) else {
            clean.push_str(rest);
            break;
        };
        clean.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(delim) = DELIMS.iter().find(|d| tail.starts_with(**d)) else {
            unreachable!("find() matched a delimiter char");
        };
        let after = &tail[delim.len()..];
        match after.find(*delim) {
            Some(close) if close > 0 => {
                let inner = &after[..close];
                entities.push(types::TextEntity {
                    offset: clean.encode_utf16().count() as i32,
                    length: inner.encode_utf16().count() as i32,
                    r#type: kind(delim),
                });
                clean.push_str(inner);
                rest = &after[close + delim.len()..];
            }
            _ => {
                clean.push_str(delim);
                rest = after;
            }
        }
    }
    types::FormattedText {
        text: clean,
        entities,
    }
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
    /// Peer `accent_color_id` (−1 = none) — seeds the userpic color.
    pub accent: i32,
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
        let folder = self.active_folder.snapshot();
        if self.archive_mode.snapshot() {
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
            msg_results: Binding::<Vec<MsgHit>>::default(),
            msg_info: Binding::default(),
            search_filter: Binding::default(),
            confirm_delete: Binding::<Option<DeleteAsk>>::default(),
            delete_revoke: Binding::bool(false),
            folder_unreads: Binding::<HashMap<i32, i32>>::default(),
            demo_roster: Rc::new(RefCell::new(Vec::new())),
            demo_corpus: Rc::new(RefCell::new(Vec::new())),
            search: Binding::container(Str::from("")),
            selected: Binding::default(),
            list_selection: Binding::default(),
            syncing_selection: Cell::new(false),
            messages: Binding::<Vec<MessageRow>>::default(),
            scroll: ScrollController::<usize>::new(0),
            composer: Binding::container(Str::from("")),
            reply_to: Binding::default(),
            editing: Binding::default(),
            completion_off: Binding::bool(false),
            forward_message: Binding::default(),
            revealed_spoilers: Binding::<Vec<i64>>::default(),
            translated: Binding::<std::collections::BTreeMap<i64, Str>>::default(),
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
            pinned_msgs: Binding::default(),
            pinned_idx: Rc::new(Cell::new(0)),
            pinned_popup: Binding::bool(false),
            jump_date_open: Binding::bool(false),
            jump_days: Binding::<Vec<DayRow>>::default(),
            shared_tab: Binding::<usize>::default(),
            shared_files: Binding::<Vec<SharedLinkRow>>::default(),
            shared_links: Binding::<Vec<SharedLinkRow>>::default(),
            chat_search_open: Binding::bool(false),
            chat_search: Binding::container(Str::from("")),
            chat_search_results: Binding::<Vec<MessageRow>>::default(),
            chat_match_ids: Binding::<Vec<i64>>::default(),
            chat_search_pos: Binding::usize(0),
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
            link_opened: Binding::container(Str::from("")),
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
            notice: Binding::container((0u64, Str::from(""))),
            // Message multi-selection (Select → batch forward/delete).
            selected_msgs: Binding::<Vec<i64>>::default(),
            folder_open: Binding::bool(false),
            folder_name: Binding::<Str>::default(),
            folder_contacts: Binding::bool(true),
            folder_groups: Binding::bool(true),
            folder_channels: Binding::bool(true),
            folder_chats: Binding::<Vec<i64>>::default(),
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

    /// Demo pinned state for chat 1: two entries — the banner shows the
    /// latest and cycles on tap (Desktop), and the banner's tap opens the
    /// pinned list popup when several messages are pinned.
    fn demo_seed_pinned(&self, chat_id: i64) {
        if chat_id != 1 {
            return;
        }
        self.pinned_label.set_from("Alice: shipping it 🚀");
        self.pinned_id.set(14);
        self.pinned_msgs.set(vec![
            PinnedRow { id: 14, label: Str::from("Alice: shipping it 🚀") },
            PinnedRow { id: 12, label: Str::from("Alice: nice. and the NV12 conversion?") },
        ]);
        self.pinned_idx.set(0);
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
                unread_mentions: 0,
                unread_reactions: 0,
                auto_delete: 0,
                pinned,
                muted,
                marked_unread: false,
                in_archive: false,
                folder_id: 0,
                photo_file: 0,
                accent: -1,
                time: Str::from("14:32"),
                typing,
                online,
                kind_icon: Str::from(icon.to_string()),
                title_styled: StyledStr::empty(),
                preview_styled: StyledStr::empty(),
                action_bar: "".into(),
                action_title: "".into(),
                peer_user: 0,
            }
        };
        // kind_icon mirrors the real path's values (state.rs `kind_icon`
        // mapping): saved/person/group/channel — never an emoji.
        let mut roster = vec![
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
        ];
        // Folder membership (Work = 2 groups/channels, Personal = 3
        // contacts) and one archived row so every chip has content.
        for r in &mut roster {
            match r.id {
                1 | 9 => r.folder_id = 2,
                2 | 6 | 8 => r.folder_id = 3,
                10 => r.in_archive = true,
                _ => {}
            }
            match r.id {
                // Demo unread reactions on the WaterUI devs chat — the ❤
                // badge + floating jump button mirror `unread_mentions`.
                1 => r.unread_reactions = 2,
                // Alice runs a 24 h auto-delete timer (row clock icon).
                2 => r.auto_delete = 86_400,
                _ => {}
            }
            // Private-chat peers carry their user id for the action bar's
            // add-contact / share-phone calls; nokhwa is an unknown
            // channel→user … (seeded rows mirror `chatActionBarReportSpam`
            // and `chatActionBarAddContact`).
            match r.id {
                2 => r.peer_user = 11,
                6 => {
                    r.peer_user = 12;
                    r.action_bar = "add_contact".into();
                }
                8 => r.peer_user = 14,
                9 => r.action_bar = "report_spam".into(),
                _ => {}
            }
        }
        *self.demo_roster.borrow_mut() = roster.clone();
        // Chip badges come from the same recount the real path gets from
        // `updateUnreadChatCount` (All = 3, Work = 1, Personal = 1).
        self.demo_recount_folders();
        self.chats.set(
            roster
                .iter()
                .filter(|r| !r.in_archive)
                .cloned()
                .collect(),
        );
        // Demo draft on a visible row (Telegram Desktop shows "Draft: …").
        self.set_draft(3, Some("release notes proofread".into()));
        self.update_chat_row(5, |r| r.unread_mentions = 2);
        // r35: chat 1 carries one unread mention so the floating `@` jump
        // button is exercisable in the demo.
        self.update_chat_row(1, |r| r.unread_mentions = 1);
        // Mirrors TDLib `chatFolders`: only user-created folders — the
        // built-in All/Archive lists are synthesized by the sidebar itself.
        self.folders.set(vec![
            FolderRow { id: 2, title: "Work".into(), active: false, unread: 0, include: vec![1, 9] },
            FolderRow { id: 3, title: "Personal".into(), active: false, unread: 0, include: vec![2, 6, 8] },
        ]);
        self.set_messages(Self::demo_conversation());
        // Seed the photo message's local file so its thumbnail renders —
        // the demo path has no TDLib download pipeline, so `file_signal(1)`
        // would otherwise stay empty and fall back to the file card.
        if let Some(path) = demo_photo_png() {
            self.files
                .borrow_mut()
                .insert(1, path.to_string_lossy().to_string());
            self.files_version.add_assign(1);
        }
        self.demo_seed_pinned(1);
        // Global-search demo corpus (chat_id, message_id, sender, text,
        // kind: 0 text / 1 media / 2 file / 3 link). Ids are
        // `demo_conversation` ids so `jump_to_message` highlights the
        // seeded row after the chat opens.
        *self.demo_corpus.borrow_mut() = vec![
            (1, 10, "Alice".into(), "morning! did the camera filters example work?".into(), 0),
            (1, 11, "Lexo".into(), "device.clone() into Arc, preview straight on the GpuSurface".into(), 0),
            (1, 12, "Alice".into(), "nice. and the NV12 conversion?".into(), 0),
            (1, 17, "Alice".into(), "photo.jpg".into(), 1),
            (1, 43, "Alice".into(), "clip.mp4".into(), 1),
            (1, 13, "".into(), "check https://waterui.dev for the docs".into(), 3),
            (2, 14, "Alice".into(), "shipping it 🚀 — GPU filters all pass".into(), 0),
            (4, 21, "".into(), "Telegram Desktop adds GPU-accelerated previews".into(), 0),
            (5, 12, "Fan".into(), "nice. and the NV12 conversion?".into(), 0),
            (5, 16, "Wei".into(), "hydrolysis on wayland works now".into(), 0),
            (6, 19, "Bob".into(), "see you at the rust meetup".into(), 0),
            (6, 20, "Bob".into(), "meetup-notes.pdf".into(), 2),
            (8, 10, "Mom".into(), "call me when free".into(), 0),
            (10, 11, "TDLib".into(), "updateAuthorizationState received".into(), 0),
        ];
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
            MemberRow { key: 11, name: "Alice".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 11 }), username: "alice".into(), photo: 0, accent: -1 },
            MemberRow { key: 12, name: "Bob".into(), status: "last seen recently".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 12 }), username: "bob".into(), photo: 0, accent: -1 },
        ]);
        // Chat 1 (WaterUI devs) is a group — the info panel's member list.
        self.members.set(vec![
            MemberRow { key: 11, name: "Alice".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 11 }), username: "alice".into(), photo: 0, accent: -1 },
            MemberRow { key: 12, name: "Bob".into(), status: "last seen recently".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 12 }), username: "bob".into(), photo: 0, accent: -1 },
            MemberRow { key: 13, name: "Lexo".into(), status: "online".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 13 }), username: "lexoliu".into(), photo: 0, accent: -1 },
            MemberRow { key: 14, name: "Carol".into(), status: "last seen 1 hour ago".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 14 }), username: "".into(), photo: 0, accent: -1 },
            MemberRow { key: 15, name: "Dan".into(), status: "last seen yesterday".into(), sender: enums::MessageSender::User(types::MessageSenderUser { user_id: 15 }), username: "".into(), photo: 0, accent: -1 },
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
            // Demo senders carry no TDLib accent id — the peer color
            // falls back to the name hash (still stable per member).
            sender_accent: -1,
            text: Str::from(text.to_string()),
            time: Str::from(time.to_string()),
            outgoing,
            read_out,
            can_edit: outgoing,
            reply_excerpt: Str::from(reply.to_string()),
            reply_to_id: 0,
            media_file: 0,
            play_file: 0,
            media_label: Str::from(media.to_string()),
            media_secs: 0,
            reaction_chips: Self::demo_chips(reactions),
            failed: false,
            pending: false,
            highlighted: false,
            unread_divider: false,
            unread_reaction: false,
            group_first: true,
            group_last: true,
            avatar_col: false,
            show_avatar: false,
            sender_photo: 0,
            day: 0,
            day_header: false,
            day_label: Str::from(""),
            edited: false,
            my_reaction: Str::from(""),
            styled: StyledStr::empty(),
            styled_open: StyledStr::empty(),
            has_spoiler: false,
            search_hit: false,
            search_styled: StyledStr::empty(),
            mentions_me: false,
            link_site: Str::from(""),
            link_title: Str::from(""),
            link_desc: Str::from(""),
            forwarded_from: Str::from(fwd.to_string()),
            sender_user: demo_user_id(sender),
            sender_chat: 0,
            fwd_user: 0,
            fwd_chat: 0,
            link_url: Str::from(""),
            poll: None,
            is_service: false,
            view_count: 0,
            author_sig: Str::from(""),
            album_id: 0,
            album_files: Vec::new(),
            kb_rows: Vec::new(),
        };
        let svc = |id: i64, text: &str| {
            let mut r = m(id, "", text, "", false, false, "", "", "", "");
            r.is_service = true;
            r
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
        // A link message carries a page preview card (r32-1).
        msgs[3].link_site = Str::from("waterui.dev");
        msgs[3].link_title = Str::from("WaterUI — native apps in Rust");
        msgs[3].link_desc = Str::from("One Rust codebase for iOS, Android, web and desktop — declarative views, fine-grained reactive state, native widgets.");
        // The card opens the URL on tap (Desktop: the whole card is a link).
        msgs[3].link_url = Str::from("https://waterui.dev");
        msgs[1].reply_to_id = 10;
        // r35: the first message mentions us — the floating `@` button
        // jumps to it.
        msgs[0].mentions_me = true;
        msgs.push(m(14, "Alice", "shipping it 🚀", "09:44", false, false, "", "👍3 ❤️1", "", ""));
        // Service rows (Desktop's centered grey lines): the pin that put
        // m14 on the banner, and a member join before Bob's first message.
        msgs.push(svc(15, "Alice pinned a message"));
        msgs.push(m(16, "", "deploying the bundle round 6", "09:45", true, false, "", "", "", ""));
        msgs.push(m(17, "Alice", "📷 photo.jpg", "09:46", false, false, "", "", "", "photo · 182 KB"));
        // A real file id so tapping the media slot opens the viewer (demo
        // has no downloaded bytes, so the viewer shows "Downloading…").
        // A real file id resolves via the seeded demo PNG (`demo_photo_png`)
        // — the media slot draws the image and tapping opens the viewer.
        msgs.last_mut().unwrap().media_file = 1;
        msgs[2].unread_divider = true;
        // Unread-reaction jump target (mirrors updateMessageUnreadReactions).
        msgs[2].unread_reaction = true;
        msgs.push(svc(18, "Bob joined the group"));
        // A second named sender exercises per-peer colors on the sender name
        // and the run avatar in group chats.
        msgs.push(m(19, "Bob", "last one from the forwarded channel", "09:47", false, false, "", "", "Telegram News", ""));
        // The forward badge opens the source channel's profile (demo chat 4).
        msgs.last_mut().unwrap().fwd_chat = 4;
        {
            let mut poll_msg = m(20, "Alice", "", "09:48", false, false, "", "", "", "");
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
        // Emoji-only messages render large on no bubble (Telegram Desktop).
        msgs.push(m(21, "", "🎉🎉🎉", "09:49", true, true, "", "", "", ""));
        msgs.push(m(22, "Alice", "🔥", "09:50", false, false, "", "❤️1", "", ""));
        // 1/3/6-chip reaction bands on text and emoji-only rows — the r24
        // no-overlap layout matrix in demo form.
        msgs.push(m(23, "Alice", "single reaction on text", "09:51", false, false, "", "👍1", "", ""));
        msgs.push(m(24, "Alice", "three reactions on this one", "09:52", false, false, "", "👍1 ❤️1 🔥1", "", ""));
        msgs.push(m(25, "Alice", "six distinct reactions", "09:53", false, false, "", "👍4 🔥2 🎉1 👀1 🚀1 ❤️1", "", ""));
        msgs.push(m(26, "", "🚀🚀", "09:54", true, true, "", "👍1 ❤️1 🔥1", "", ""));
        msgs.push(m(27, "Alice", "👀", "09:55", false, false, "", "👍2 🔥2 🎉1 👀1 🚀1 ❤️1", "", ""));
        // A trailing service line — the bottom of the chat carries a
        // service row after the last message row too.
        msgs.push(svc(28, "Lexo created the group"));
        // A spoiler message: the masked span currently renders black on
        // black forever; r32-2 makes it reveal on tap like Desktop.
        {
            let spoiler_text = "no spoilers please — it's a trap";
            let mut spoiler_msg =
                m(29, "Carol", spoiler_text, "09:56", false, false, "", "", "", "");
            let ft = types::FormattedText {
                text: spoiler_text.into(),
                entities: vec![types::TextEntity {
                    offset: 21,
                    length: 11,
                    r#type: enums::TextEntityType::Spoiler,
                }],
            };
            spoiler_msg.has_spoiler = true;
            spoiler_msg.styled = styled_from_formatted_mask(
                &ft,
                Some(Color::from(Foreground)),
            );
            spoiler_msg.styled_open = styled_from_formatted_mask(&ft, None);
            msgs.push(spoiler_msg);
        }
        // A reply quote inside the visible window for live tap-to-jump
        // verification: m24 quotes m23.
        msgs[14].reply_excerpt = Str::from("single reaction on text");
        msgs[14].reply_to_id = 23;
        // A quote whose original is scrolled well out of view (r25-2):
        // m25 quotes the first demo message — tapping it must scroll the
        // list up to m10, not just highlight an on-screen row.
        msgs[15].reply_excerpt = Str::from("morning! did the camera filters example work?");
        msgs[15].reply_to_id = 10;
        // A quote whose original is NOT in the loaded window — tapping it
        // exercises the getChatHistory fetch path (r25-2).
        msgs[17].reply_excerpt = Str::from("earlier history, not loaded");
        msgs[17].reply_to_id = 9;
        // Chosen states in both chip placements (r25-1 verification):
        // inside an incoming bubble (m23) and on an emoji-only row (m26).
        if let Some(chip) = msgs[13].reaction_chips.first_mut() {
            chip.chosen = true;
        }
        // Chosen inside an outgoing bubble (m14) — the container-tint
        // variant that stays readable on the accent fill.
        if let Some(chip) = msgs[4].reaction_chips.first_mut() {
            chip.chosen = true;
        }
        if let Some(chip) = msgs[16].reaction_chips.first_mut() {
            chip.chosen = true;
        }
        // A three-photo album (TDLib media_album_id): the rows merge into
        // one bubble grid at set_messages.
        for (mid, caption) in [(40, "first of the set"), (41, ""), (42, "")] {
            let mut a = m(mid, "Alice", caption, "09:57", false, false, "", "", "", "photo · 240 KB");
            a.media_file = 1;
            a.album_id = 777;
            msgs.push(a);
        }
        // A video bubble: thumbnail already local (file 1, the seeded PNG),
        // payload still downloading — Desktop draws the m:ss duration badge
        // on the thumbnail corner in this state.
        {
            let mut v = m(43, "Alice", "clip.mp4", "09:58", false, false, "", "", "", "video");
            v.media_file = 1;
            v.play_file = 2;
            v.media_secs = 65;
            msgs.push(v);
        }
        // A bot message with an inline keyboard (`replyMarkupInlineKeyboard`):
        // URL opens the browser, Callback answers the bot's query, CopyText
        // puts the token on the clipboard — the three dispatchable kinds.
        {
            let mut kb = m(44, "CI bot", "Build #4127 passed on dev — 3m 42s", "09:59", false, false, "", "", "", "");
            kb.kb_rows = vec![
                vec![
                    KbBtn { text: "Open build".into(), kind: KbKind::Url("https://waterui.dev".into()) },
                    KbBtn { text: "Notify me".into(), kind: KbKind::Callback("sub:4127".into()) },
                ],
                vec![
                    KbBtn { text: "Copy token".into(), kind: KbKind::Copy("tok-4127".into()) },
                ],
            ];
            msgs.push(kb);
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
    fn content_preview(content: &enums::MessageContent) -> (Str, i32, Str, i32, i32) {
        // (text/caption, thumbnail file id, media label, playable file id, secs)
        match content {
            enums::MessageContent::MessageText(t) => {
                (t.text.text.clone().into(), 0, Str::from(""), 0, 0)
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
                v.video.duration,
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
                0,
            ),
            enums::MessageContent::MessageAudio(a) => (
                format!("Audio: {}", a.audio.title).into(),
                0,
                "Audio".into(),
                a.audio.audio.id,
                a.audio.duration,
            ),
            enums::MessageContent::MessageVoiceNote(v) => {
                (
                    format!("Voice message ({}s)", v.voice_note.duration).into(),
                    0,
                    "Voice".into(),
                    v.voice_note.voice.id,
                    v.voice_note.duration,
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
                    v.video_note.duration,
                )
            }
            enums::MessageContent::MessageSticker(s) => (
                format!("{} Sticker", s.sticker.emoji).into(),
                0,
                "Sticker".into(),
                0,
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
                a.animation.duration,
            ),
            enums::MessageContent::MessageLocation(_) => {
                ("Location".into(), 0, "Location".into(), 0, 0)
            }
            enums::MessageContent::MessageContact(c) => (
                format!("Contact: {} {}", c.contact.first_name, c.contact.last_name)
                    .into(),
                0,
                "Contact".into(),
                0,
                0,
            ),
            enums::MessageContent::MessagePoll(p) => {
                (format!("Poll: {}", p.poll.question.text).into(), 0, "Poll".into(), 0, 0)
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
                0,
            ),
            enums::MessageContent::MessageChatAddMembers(_) => {
                ("New members joined".into(), 0, Str::from(""), 0, 0)
            }
            enums::MessageContent::MessageChatJoinByLink
            | enums::MessageContent::MessageChatJoinByRequest => {
                ("Joined the chat".into(), 0, Str::from(""), 0, 0)
            }
            enums::MessageContent::MessagePinMessage(_) => {
                ("Pinned a message".into(), 0, Str::from(""), 0, 0)
            }
            _ => ("Unsupported message".into(), 0, Str::from(""), 0, 0),
        }
    }

    /// Demo seed helper: parse "👍2 ❤️1" into structured chips
    /// (each token is `<emoji><count>`).
    fn demo_chips(display: &str) -> Vec<ReactionChip> {
        display
            .split(' ')
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

    /// Build reaction chips + the chosen-emoji string from TDLib's
    /// `messageInteractionInfo.reactions`.
    fn chips_from_td(
        reactions: &types::MessageReactions,
    ) -> (Vec<ReactionChip>, Str) {
        let mut mine = String::new();
        let chips = reactions
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
                ReactionChip {
                    emoji: Str::from(emoji),
                    count: mr.total_count,
                    chosen: mr.is_chosen,
                }
            })
            .collect();
        (chips, mine.into())
    }

    fn reactions_info(m: &types::Message) -> (Vec<ReactionChip>, Str) {
        match m
            .interaction_info
            .as_ref()
            .and_then(|i| i.reactions.as_ref())
        {
            Some(reactions) => Self::chips_from_td(reactions),
            None => (Vec::new(), Str::from("")),
        }
    }

    /// Small profile-photo file id for a `MessageSender` (0 → initials).
    fn sender_photo(&self, sender: &enums::MessageSender) -> i32 {
        match sender {
            enums::MessageSender::User(u) => self
                .users
                .borrow()
                .get(&u.user_id)
                .and_then(|u| u.profile_photo.as_ref())
                .map(|p| p.small.id)
                .unwrap_or(0),
            enums::MessageSender::Chat(c) => self
                .chat_objs
                .borrow()
                .get(&c.chat_id)
                .and_then(|c| c.photo.as_ref())
                .map(|p| p.small.id)
                .unwrap_or(0),
        }
    }

    /// Build a `MessageRow` from a TDLib message. Runs on the UI thread, so it
    /// may borrow the caches and kick off file downloads.
    #[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
    pub fn message_row(&self, m: &types::Message) -> MessageRow {
        let (mut text, media_file, media_label, play_file, media_secs) = Self::content_preview(&m.content);
        if let enums::MessageContent::MessageText(t) = &m.content {
            text = t.text.text.clone().into();
        }
        // TDLib service messages render as Desktop's centered grey lines,
        // not bubbles — prefix the actor ("You"/sender name) like Desktop.
        let is_service = matches!(
            m.content,
            enums::MessageContent::MessagePinMessage(_)
                | enums::MessageContent::MessageChatAddMembers(_)
                | enums::MessageContent::MessageChatJoinByLink
                | enums::MessageContent::MessageChatJoinByRequest
        );
        if is_service {
            let actor = if m.is_outgoing {
                Str::from("You")
            } else {
                self.sender_name(&m.sender_id)
            };
            let action = match &m.content {
                enums::MessageContent::MessagePinMessage(_) => "pinned a message".to_string(),
                enums::MessageContent::MessageChatAddMembers(a) => {
                    let names = a
                        .member_user_ids
                        .iter()
                        .filter_map(|uid| {
                            self.users
                                .borrow()
                                .get(uid)
                                .map(|u| u.first_name.clone())
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    if names.is_empty() {
                        "added new members".to_string()
                    } else {
                        format!("added {names}")
                    }
                }
                _ => "joined the group".to_string(),
            };
            text = format!("{actor} {action}").into();
        }
        if media_file != 0 {
            self.want_file_id(media_file);
        }
        if play_file != 0 {
            self.want_file_id(play_file);
        }
        let (reply_excerpt, reply_to_id) = match &m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) => (
                r.content
                    .as_ref()
                    .map(|c| {
                        let (t, _, l, _, _) = Self::content_preview(c);
                        if t.is_empty() {
                            l
                        } else {
                            t
                        }
                    })
                    .unwrap_or_else(|| "Reply".into()),
                r.message_id,
            ),
            Some(enums::MessageReplyTo::Story(_)) => (Str::from("Story"), 0),
            None => (Str::from(""), 0),
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
        let (styled, styled_open, has_spoiler, link_site, link_title, link_desc, link_url) =
            if let enums::MessageContent::MessageText(t) = &m.content {
                let mask = if m.is_outgoing {
                    Color::from(AccentForeground)
                } else {
                    Color::from(Foreground)
                };
                let spoiler = has_spoiler_entity(&t.text);
                (
                    styled_from_formatted_mask(&t.text, spoiler.then_some(mask)),
                    if spoiler {
                        styled_from_formatted_mask(&t.text, None)
                    } else {
                        StyledStr::empty()
                    },
                    spoiler,
                    t.link_preview.as_ref().map(|p| p.site_name.clone().into()).unwrap_or_default(),
                    t.link_preview.as_ref().map(|p| p.title.clone().into()).unwrap_or_default(),
                    t.link_preview.as_ref().map(|p| {
                        p.description.text.clone().into()
                    }).unwrap_or_default(),
                    t.link_preview.as_ref().map(|p| Str::from(p.url.clone()))
                        .filter(|u: &Str| !u.is_empty())
                        .or_else(|| first_url_entity(&t.text))
                        .unwrap_or_default(),
                )
            } else {
                (
                    StyledStr::empty(),
                    StyledStr::empty(),
                    false,
                    Str::from(""),
                    Str::from(""),
                    Str::from(""),
                    Str::from(""),
                )
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
        // Peer ids behind the sender name/avatar and the forward badge —
        // Desktop opens the profile / source channel on tap.
        let (sender_user, sender_chat) = match &m.sender_id {
            enums::MessageSender::User(u) => (u.user_id, 0),
            enums::MessageSender::Chat(c) => (0, c.chat_id),
        };
        let (fwd_user, fwd_chat) = m
            .forward_info
            .as_ref()
            .map(|f| match &f.origin {
                enums::MessageOrigin::User(u) => (u.sender_user_id, 0),
                enums::MessageOrigin::HiddenUser(_) => (0, 0),
                enums::MessageOrigin::Chat(c) => (0, c.sender_chat_id),
                enums::MessageOrigin::Channel(c) => (0, c.chat_id),
            })
            .unwrap_or((0, 0));
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
        let sender_photo = self.sender_photo(&m.sender_id);
        if sender_photo != 0 {
            self.want_file_id(sender_photo);
        }
        let kb_rows = match &m.reply_markup {
            Some(enums::ReplyMarkup::InlineKeyboard(kb)) => kb
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|b| {
                            let kind = match &b.r#type {
                                enums::InlineKeyboardButtonType::Url(u) => {
                                    KbKind::Url(u.url.clone().into())
                                }
                                enums::InlineKeyboardButtonType::Callback(c) => {
                                    KbKind::Callback(c.data.clone().into())
                                }
                                enums::InlineKeyboardButtonType::CopyText(c) => {
                                    KbKind::Copy(c.text.clone().into())
                                }
                                enums::InlineKeyboardButtonType::SwitchInline(s) => {
                                    KbKind::SwitchInline(s.query.clone().into())
                                }
                                enums::InlineKeyboardButtonType::User(u) => {
                                    KbKind::User(u.user_id)
                                }
                                _ => KbKind::Unsupported,
                            };
                            KbBtn {
                                text: b.text.clone().into(),
                                kind,
                            }
                        })
                        .collect()
                })
                .collect(),
            _ => Vec::new(),
        };
        MessageRow {
            id: m.id,
            sender: self.sender_name(&m.sender_id),
            sender_accent: match &m.sender_id {
                enums::MessageSender::User(u) => self
                    .users
                    .borrow()
                    .get(&u.user_id)
                    .map(|u| u.accent_color_id)
                    .unwrap_or(-1),
                enums::MessageSender::Chat(c) => self
                    .chat_objs
                    .borrow()
                    .get(&c.chat_id)
                    .map(|ch| ch.accent_color_id)
                    .unwrap_or(-1),
            },
            sender_photo,
            text,
            time: fmt_time(m.date),
            outgoing: m.is_outgoing,
            read_out,
            styled,
            styled_open,
            has_spoiler,
            link_site,
            link_title,
            link_desc,
            link_url,
            forwarded_from,
            sender_user,
            sender_chat,
            fwd_user,
            fwd_chat,
            can_edit: m.is_outgoing,
            reply_excerpt,
            reply_to_id,
            media_file,
            play_file,
            media_label,
            media_secs,
            reaction_chips: reactions,
            my_reaction,
            failed,
            pending,
            highlighted: false,
            search_hit: false,
            search_styled: StyledStr::empty(),
            mentions_me: false,
            unread_divider: false,
            unread_reaction: false,
            group_first: true,
            group_last: true,
            avatar_col: false,
            show_avatar: false,
            day: local_day(m.date),
            day_header: false,
            day_label: Str::from(""),
            edited: m.edit_date != 0,
            poll,
            is_service,
            view_count: m
                .interaction_info
                .as_ref()
                .map(|i| i.view_count)
                .unwrap_or(0),
            author_sig: Str::from(m.author_signature.clone()),
            album_id: m.media_album_id,
            album_files: Vec::new(),
            kb_rows,
        }
    }

#[allow(if_else_view)] // when() requires a signal; conditions here are plain bools
    fn preview_text(&self, m: &types::Message) -> Str {
        let (t, _, label, _, _) = Self::content_preview(&m.content);
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
        let folder_id = chat
            .positions
            .iter()
            .find_map(|p| match &p.list {
                enums::ChatList::Folder(f) => Some(f.chat_folder_id),
                _ => None,
            })
            .unwrap_or(0);
        let peer_user = match &chat.r#type {
            enums::ChatType::Private(p) => p.user_id,
            enums::ChatType::Secret(s) => s.user_id,
            _ => 0,
        };
        let (action_bar, action_title) = Self::action_bar_parts(chat.action_bar.as_ref());
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
            unread_mentions: chat.unread_mention_count,
            unread_reactions: chat.unread_reaction_count,
            auto_delete: chat.message_auto_delete_time,
            pinned,
            muted,
            marked_unread: chat.is_marked_as_unread,
            in_archive: position_in(&chat, &enums::ChatList::Archive).is_some(),
            folder_id,
            photo_file: chat.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
            accent: chat.accent_color_id,
            time,
            typing: false,
            online,
            kind_icon: kind_icon.into(),
            title_styled: StyledStr::empty(),
            preview_styled: StyledStr::empty(),
            action_bar,
            action_title,
            peer_user,
        };
        let mut list = self.chats.snapshot();
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
        // Keep the demo roster authoritative too so `set_list` rebuilds
        // keep the same field values.
        if let Some(r) = self
            .demo_roster
            .borrow_mut()
            .iter_mut()
            .find(|r| r.id == chat_id)
        {
            f(r);
        }
        let mut list = self.chats.snapshot();
        if let Some(r) = list.iter_mut().find(|r| r.id == chat_id) {
            f(r);
            list.sort();
            self.chats.set(list);
        }
    }

    /// Recount the demo folder chips from the roster — the real path gets
    /// the same numbers from `updateUnreadChatCount`.
    fn demo_recount_folders(&self) {
        if self.client_id.get() != 0 {
            return;
        }
        let roster = self.demo_roster.borrow();
        let mut map = HashMap::new();
        for r in roster.iter() {
            let key = if r.in_archive { -1 } else { r.folder_id };
            *map.entry(key).or_insert(0) += i32::from(r.unread > 0 || r.marked_unread);
        }
        map.insert(0, roster.iter().filter(|r| !r.in_archive && (r.unread > 0 || r.marked_unread)).count() as i32);
        self.folder_unreads.set(map);
    }

    fn update_message_row(&self, message_id: i64, f: impl Fn(&mut MessageRow)) {
        let mut list = self.messages.snapshot();
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
                let mut list = self.chats.snapshot();
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
                            unread: 0,
                            // ChatFolderInfo carries no membership — the
                            // editor refetches the full ChatFolder on open.
                            include: Vec::new(),
                        })
                        .collect(),
                );
            }
            enums::Update::ChatActionBar(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.action_bar = u.action_bar.clone();
                }
                let (kind, title) = Self::action_bar_parts(u.action_bar.as_ref());
                self.update_chat_row(u.chat_id, |r| {
                    r.action_bar = kind.clone();
                    r.action_title = title.clone();
                });
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
            enums::Update::UnreadChatCount(u) => {
                let key = match &u.chat_list {
                    enums::ChatList::Main => 0,
                    enums::ChatList::Archive => -1,
                    enums::ChatList::Folder(f) => f.chat_folder_id,
                };
                let mut map = self.folder_unreads.snapshot();
                map.insert(key, u.unread_count);
                self.folder_unreads.set(map);
            }
            enums::Update::ChatReadOutbox(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.last_read_outbox_message_id = u.last_read_outbox_message_id;
                }
                if u.chat_id == self.open_chat.get() {
                    let mut list = self.messages.snapshot();
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
            enums::Update::ChatUnreadMentionCount(u) => {
                if let Some(c) = self.chat_objs.borrow_mut().get_mut(&u.chat_id) {
                    c.unread_mention_count = u.unread_mention_count;
                }
                self.update_chat_row(u.chat_id, |r| {
                    r.unread_mentions = u.unread_mention_count
                });
            }
            enums::Update::ChatUnreadReactionCount(u) => {
                self.update_chat_row(u.chat_id, |r| {
                    r.unread_reactions = u.unread_reaction_count
                });
            }
            // Per-message unread reactions (supergroups/channels): flag the
            // earliest one so the floating ❤ button can jump to it.
            enums::Update::MessageUnreadReactions(u) => {
                if u.chat_id == self.open_chat.get() {
                    self.update_message_row(u.message_id, |r| {
                        r.unread_reaction = u.unread_reaction_count > 0
                    });
                }
            }
            enums::Update::MessageInteractionInfo(u) => {
                if u.chat_id == self.open_chat.get() {
                    let (chips, mine) = u
                        .interaction_info
                        .as_ref()
                        .and_then(|i| i.reactions.as_ref())
                        .map(Self::chips_from_td)
                        .unwrap_or_default();
                    let views = u
                        .interaction_info
                        .as_ref()
                        .map(|i| i.view_count)
                        .unwrap_or(0);
                    self.update_message_row(u.message_id, |r| {
                        r.reaction_chips = chips.clone();
                        r.my_reaction = mine.clone();
                        r.view_count = views;
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
                    let mut list = self.messages.snapshot();
                    if !list.iter().any(|r| r.id == row.id) {
                        let outgoing = row.outgoing;
                        list.push(row);
                        list.sort();
                        self.set_messages(list);
                        if self.follows_tail(outgoing) {
                            self.scroll_bottom();
                        }
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
                    let mut list = self.messages.snapshot();
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
                    let (text, media, label, play, _) = Self::content_preview(&u.new_content);
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
                    let mut list = self.messages.snapshot();
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
        let id: i32 = self.api_id.snapshot().trim().parse().unwrap_or(0);
        let hash = self.api_hash.snapshot().to_string();
        if id <= 0 || hash.is_empty() {
            self.auth_note.set_from("Enter a valid api_id and api_hash (from my.telegram.org).");
            return;
        }
        let cfg = Config {
            api_id: id,
            api_hash: hash,
            test_dc: self.test_dc.snapshot(),
        };
        cfg.save();
        self.set_tdlib_parameters(cfg);
    }

    pub fn submit_phone(&self) {
        let phone = self.phone.snapshot().to_string();
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
        let code = self.code.snapshot().to_string();
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
        let pw = self.password.snapshot().expose().to_string();
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
        let first = self.first_name.snapshot().to_string();
        if first.trim().is_empty() {
            self.auth_note.set_from("First name is required.");
            return;
        }
        let last = self.last_name.snapshot().to_string();
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
        let batch = self.forward_ids.snapshot();
        let single = self.forward_message.snapshot();
        if !batch.is_empty() || single.is_some() {
            let (from, msg_ids) = if !batch.is_empty() {
                (self.open_chat.get(), batch)
            } else {
                let (from, msg_id) = single.unwrap_or_default();
                (from, vec![msg_id])
            };
            let send_copy = self.forward_noattr.snapshot();
            let comment = self.forward_comment.snapshot();
            let n = msg_ids.len();
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
            // the still-open chat (forwarding does not open a chat). The
            // snap-back write is deferred one task turn: this select_chat ran
            // inside `list_selection`'s on_change dispatch, and writing the
            // same signal re-entrantly panics on the handler RefCell
            // (on_change.rs:84) — water-rs/waterui#1297, DOGFOOD r37-1; revert
            // to a plain `set` once the fix lands.
            self.syncing_selection.set(true);
            let sel = self.selected.snapshot();
            let st = self.clone();
            spawn_local(async move {
                st.list_selection.set(sel);
                st.syncing_selection.set(false);
            })
            .detach();
            self.notify(if n == 1 {
                Str::from("Message forwarded")
            } else {
                Str::from(format!("{n} messages forwarded"))
            });
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
        self.pinned_msgs.set(Vec::new());
        self.pinned_idx.set(0);
        self.pinned_popup.set(false);
        self.chat_search_open.set(false);
        self.chat_search.set_from("");
        self.chat_search_results.set(Vec::new());
        self.members.set(Vec::new());
        if prev != 0 {
            let cur = self.composer.snapshot();
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
            let mut msgs = Self::demo_conversation();
            // Same rule as `apply_unread_divider`: only a chat with unread
            // gets the "Unread messages" divider row.
            let chat = self
                .chats
                .snapshot()
                .iter()
                .find(|r| r.id == chat_id)
                .cloned();
            if chat.as_ref().map(|r| r.unread).unwrap_or(0) == 0 {
                for r in msgs.iter_mut() {
                    r.unread_divider = false;
                }
            }
            // Channel posts carry the 👁 view count, and some carry an
            // author signature (TDLib author_signature).
            if chat.map(|r| r.kind_icon.as_str() == "channel").unwrap_or(false) {
                for r in msgs.iter_mut() {
                    if !r.is_service {
                        r.view_count = 1200 + (r.id as i32) * 137;
                        if r.id % 2 == 0 {
                            r.author_sig = Str::from("T. G. Team");
                        }
                    }
                }
            }
            self.set_messages(msgs);
            self.demo_seed_pinned(chat_id);
            self.scroll_to_open(chat_id);
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
            store.scroll_to_open(chat_id);
        })
        .detach();
    }

    /// Toggle the right-side info panel and (re)load shared media.
    pub fn toggle_info(&self) {
        let open = !self.info_open.snapshot();
        self.info_open.set(open);
        if open {
            self.load_shared_tab(self.shared_tab.snapshot());
            let kind = self
                .chats
                .snapshot()
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
            .snapshot()
            .iter()
            .find(|r| r.id == chat_id)
            .map(|r| r.unread)
            .unwrap_or(0);
        let mut list = self.messages.snapshot();
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
        // Media albums (TDLib `media_album_id`): consecutive rows sharing a
        // non-zero album id merge into one bubble — the row keeps the first
        // member's id and collects every member's media file.
        let mut merged: Vec<MessageRow> = Vec::with_capacity(rows.len());
        for mut r in rows.drain(..) {
            let joinable = r.album_id != 0
                && merged
                    .last()
                    .map(|p| p.album_id == r.album_id)
                    .unwrap_or(false);
            if joinable {
                let p = merged.last_mut().expect("joinable implies nonempty");
                if r.media_file != 0 {
                    p.album_files.push(r.media_file);
                }
                if !r.text.is_empty() {
                    if !p.text.is_empty() {
                        p.text = Str::from(format!("{}\n{}", p.text, r.text));
                    } else {
                        p.text = r.text.clone();
                    }
                }
            } else {
                if r.album_id != 0 && r.media_file != 0 {
                    r.album_files = vec![r.media_file];
                }
                merged.push(r);
            }
        }
        let mut rows = merged;
        let mut prev_day = 0i64;
        for r in &mut rows {
            r.day_header = r.day != prev_day;
            if r.day_header {
                r.day_label = fmt_day_label(r.day);
            }
            prev_day = r.day;
        }
        // Desktop message grouping: consecutive rows sharing direction and
        // sender form one visual run — the sender name shows on the run's
        // first row and, in groups/channels, the sender avatar on its last.
        // Unread dividers and day headers break a run.
        let grouped = self
            .chats
            .snapshot()
            .iter()
            .find(|r| r.id == self.open_chat.get())
            .map(|r| matches!(r.kind_icon.to_string().as_str(), "group" | "channel"))
            .unwrap_or(false);
        let n = rows.len();
        for i in 0..n {
            let same_run = |a: &MessageRow, b: &MessageRow| {
                a.outgoing == b.outgoing
                    && a.sender == b.sender
                    && !b.unread_divider
                    && !b.day_header
                    && !a.is_service
                    && !b.is_service
            };
            let first = i == 0 || !same_run(&rows[i - 1], &rows[i]);
            let last = i + 1 == n || !same_run(&rows[i], &rows[i + 1]);
            let r = &mut rows[i];
            r.group_first = first;
            r.group_last = last;
            r.avatar_col = grouped && !r.outgoing && !r.is_service;
            r.show_avatar = r.avatar_col && last;
        }
        self.messages.set(rows);
    }

    /// Re-run the day-header/grouping pass on the current rows — demo
    /// seeds set `open_chat` after the initial `set_messages`, so the
    /// grouping flags need a re-pass once the open chat is known.
    pub fn regroup_messages(&self) {
        self.set_messages(self.messages.snapshot());
    }

    /// Fetch the chat's pinned messages into `pinned_msgs` plus the
    /// banner's `pinned_label`/`pinned_id` pair. `getChatPinnedMessage`
    /// returns only the latest pin; `searchChatMessages(Pinned)` fills
    /// the list popup's rows.
    #[allow(if_else_view)] // string pick, not a view
    async fn refresh_pinned(&self, chat_id: i64) {
        match functions::get_chat_pinned_message(chat_id, self.client_id.get()).await {
            Ok(enums::Message::Message(m)) => {
                let (t, _, label, _, _) = Self::content_preview(&m.content);
                self.pinned_id.set(m.id);
                self.pinned_label
                    .set(if t.is_empty() { label } else { t });
            }
            _ => {
                self.pinned_id.set(0);
                self.pinned_label.set_from("");
            }
        }
        // Full pinned list for the popup (searchChatMessages with the
        // Pinned filter — Desktop's "View all pinned" path). Keep the
        // single-pin state working if the search fails.
        if let Ok(enums::FoundChatMessages::FoundChatMessages(found)) = functions::search_chat_messages(
            chat_id,
            None,
            String::new(),
            None,
            0,
            0,
            100,
            Some(enums::SearchMessagesFilter::Pinned),
            self.client_id.get(),
        )
        .await
        {
            let mut entries: Vec<PinnedRow> = found
                .messages
                .iter()
                .map(|m| {
                    let (t, _, label, _, _) = Self::content_preview(&m.content);
                    PinnedRow {
                        id: m.id,
                        label: if t.is_empty() { label } else { t },
                    }
                })
                .collect();
            entries.sort_by_key(|e| std::cmp::Reverse(e.id));
            if !entries.is_empty() {
                self.pinned_msgs.set(entries);
                self.pinned_idx.set(0);
            } else {
                self.pinned_msgs.set(Vec::new());
            }
        }
    }

    /// Banner tap: cycle to the next pinned message and jump to it
    /// (Desktop advances the bar through every pin). Single pin jumps
    /// straight to it, same as before.
    pub fn pinned_tap(&self) {
        let list = self.pinned_msgs.snapshot();
        if list.len() > 1 {
            let open = !self.pinned_popup.snapshot();
            self.pinned_popup.set(open);
            return;
        }
        self.jump_to_message(self.pinned_id.get());
    }

    /// Jump to a pinned message from the popup, then close it.
    pub fn pinned_jump(&self, message_id: i64) {
        self.pinned_popup.set(false);
        self.jump_to_message(message_id);
    }

    /// Chat-header calendar button: toggle the jump-to-date popup and
    /// (re)load its day list — locally from the loaded window on a demo
    /// store, `getChatMessageCalendar` on a real session.
    pub fn toggle_jump_date(&self) {
        let open = !self.jump_date_open.snapshot();
        self.jump_date_open.set(open);
        if !open {
            return;
        }
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if self.client_id.get() == 0 {
            let mut seen: Vec<i64> = Vec::new();
            for r in self.messages.snapshot() {
                if r.day != 0 && !seen.contains(&r.day) {
                    seen.push(r.day);
                }
            }
            seen.sort_unstable();
            self.jump_days.set(
                seen.iter()
                    .map(|&d| DayRow {
                        day: d,
                        label: fmt_day_label(d),
                        count: 0,
                    })
                    .collect(),
            );
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::MessageCalendar::MessageCalendar(cal)) =
                functions::get_chat_message_calendar(
                    chat_id,
                    None,
                    enums::SearchMessagesFilter::Empty,
                    0,
                    client,
                )
                .await
            {
                store.jump_days.set(
                    cal.days
                        .iter()
                        .map(|d| {
                            let day = local_day(d.message.date);
                            DayRow {
                                day,
                                label: fmt_day_label(day),
                                count: d.total_count,
                            }
                        })
                        .collect(),
                );
            }
        })
        .detach();
    }

    /// Jump-to-date pick: scroll to the first loaded message of the day,
    /// or fetch it via `getChatMessageByDate` when the day predates the
    /// loaded window.
    pub fn jump_to_day(&self, day: i64) {
        self.jump_date_open.set(false);
        let list = self.messages.snapshot();
        if let Some(row) = list.iter().find(|r| r.day == day) {
            let id = row.id;
            self.jump_to_message(id);
            return;
        }
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        if client == 0 {
            return;
        }
        let ts = chrono::NaiveDate::from_num_days_from_ce_opt(day as i32)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
            .map(|t| t.timestamp() as i32)
            .unwrap_or(0);
        if ts == 0 {
            return;
        }
        let store = self.clone();
        spawn_local(async move {
            if let Ok(enums::Message::Message(m)) =
                functions::get_chat_message_by_date(chat_id, ts, client).await
            {
                store.jump_to_message(m.id);
            }
        })
        .detach();
    }

    /// Info panel shared-content tabs: 0 media (PhotoAndVideo), 1 files
    /// (Document), 2 links (Url). Tab switches reload through
    /// `searchChatMessages` with the matching filter.
    pub fn load_shared_tab(&self, tab: usize) {
        self.shared_tab.set(tab);
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if self.client_id.get() == 0 {
            // Demo seeds so all three tabs render content.
            match tab {
                1 => self.shared_files.set(
                    [("spec-draft.md", "182 KB"), ("hydrolysis-trace.log", "4.1 MB"), ("r36-repo.bundle", "41 MB")]
                        .iter()
                        .enumerate()
                        .map(|(i, (t, d))| SharedLinkRow {
                            id: -(i as i64) - 100,
                            title: Str::from(*t),
                            detail: Str::from(*d),
                        })
                        .collect(),
                ),
                2 => self.shared_links.set(
                    [("WaterUI — native apps in Rust", "waterui.dev"), ("hydrolysis#251", "github.com/water-rs/hydrolysis/issues/251")]
                        .iter()
                        .enumerate()
                        .map(|(i, (t, d))| SharedLinkRow {
                            id: -(i as i64) - 200,
                            title: Str::from(*t),
                            detail: Str::from(*d),
                        })
                        .collect(),
                ),
                _ => self.load_shared_media(),
            }
            return;
        }
        if tab == 0 {
            self.load_shared_media();
            return;
        }
        let filter = if tab == 1 {
            enums::SearchMessagesFilter::Document
        } else {
            enums::SearchMessagesFilter::Url
        };
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
                    50,
                    Some(filter),
                    client,
                )
                .await
            {
                let rows: Vec<SharedLinkRow> = found
                    .messages
                    .iter()
                    .map(|m| {
                        let (t, _, label, _, _) = Store::content_preview(&m.content);
                        SharedLinkRow {
                            id: m.id,
                            title: if t.is_empty() { label } else { t },
                            detail: fmt_time(m.date),
                        }
                    })
                    .collect();
                if tab == 1 {
                    store.shared_files.set(rows);
                } else {
                    store.shared_links.set(rows);
                }
            }
        })
        .detach();
    }

    pub fn pin_message(&self, message_id: i64) {
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        if client == 0 {
            // Demo: pin locally, matching what refresh_pinned would produce
            // from a real pinChatMessage + getChatPinnedMessage round-trip.
            let label = self
                .messages
                .snapshot()
                .iter()
                .find(|r| r.id == message_id)
                .map(|r| {
                    if r.text.is_empty() {
                        r.media_label.clone()
                    } else {
                        r.text.clone()
                    }
                })
                .unwrap_or_default();
            self.pinned_id.set(message_id);
            self.pinned_label.set(label.clone());
            let mut pins = self.pinned_msgs.snapshot();
            pins.retain(|r| r.id != message_id);
            pins.insert(0, PinnedRow { id: message_id, label });
            self.pinned_msgs.set(pins);
            self.pinned_idx.set(0);
            // A real pinChatMessage(notify) produces a service row in
            // history; mirror it so the demo chat shows the same line.
            let mut rows = self.messages.snapshot();
            let day = rows.last().map(|r| r.day).unwrap_or(0);
            let next_id = rows.iter().map(|r| r.id).max().unwrap_or(0) + 1;
            rows.push(MessageRow {
                id: next_id,
                sender: Str::from(""),
                sender_accent: -1,
                text: Str::from("You pinned a message"),
                time: Str::from(""),
                outgoing: false,
                read_out: false,
                can_edit: false,
                reply_excerpt: Str::from(""),
                reply_to_id: 0,
                media_file: 0,
                play_file: 0,
                media_label: Str::from(""),
                media_secs: 0,
                reaction_chips: Vec::new(),
                my_reaction: Str::from(""),
                styled: StyledStr::empty(),
                styled_open: StyledStr::empty(),
                has_spoiler: false,
                link_site: Str::from(""),
                link_title: Str::from(""),
                link_desc: Str::from(""),
                link_url: Str::from(""),
                forwarded_from: Str::from(""),
                sender_user: 0,
                sender_chat: 0,
                fwd_user: 0,
                fwd_chat: 0,
                failed: false,
                pending: false,
                highlighted: false,
                search_hit: false,
                search_styled: StyledStr::empty(),
                mentions_me: false,
                unread_divider: false,
                unread_reaction: false,
                day,
                day_header: false,
                day_label: Str::from(""),
                edited: false,
                poll: None,
                group_first: false,
                group_last: false,
                avatar_col: false,
                show_avatar: false,
                sender_photo: 0,
                is_service: true,
                view_count: 0,
                author_sig: Str::from(""),
                album_id: 0,
                album_files: Vec::new(),
                kb_rows: Vec::new(),
            });
            self.set_messages(rows);
            return;
        }
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
        if client == 0 {
            let mut pins = self.pinned_msgs.snapshot();
            pins.retain(|r| r.id != message_id);
            self.pinned_msgs.set(pins);
            self.pinned_idx.set(0);
            self.pinned_popup.set(false);
            match self.pinned_msgs.snapshot().first() {
                Some(row) => {
                    self.pinned_id.set(row.id);
                    self.pinned_label.set(row.label.clone());
                }
                None => {
                    self.pinned_id.set(0);
                    self.pinned_label.set_from("");
                }
            }
            return;
        }
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
        if chat_id == 0 || self.client_id.get() == 0 {
            // Demo mode: apply the toggle locally so the strip updates —
            // chosen chips highlight and count, unchosen at 0 drop out.
            let emoji_s = emoji.to_string();
            self.update_message_row(row.id, move |r| {
                if let Some(pos) = r
                    .reaction_chips
                    .iter()
                    .position(|c| c.emoji.as_str() == emoji_s)
                {
                    let chip = &mut r.reaction_chips[pos];
                    if chip.chosen {
                        chip.chosen = false;
                        chip.count -= 1;
                        r.my_reaction = Str::from("");
                    } else {
                        chip.chosen = true;
                        chip.count += 1;
                        r.my_reaction = Str::from(emoji_s.clone());
                    }
                    if r.reaction_chips[pos].count <= 0 {
                        r.reaction_chips.remove(pos);
                    }
                } else {
                    r.reaction_chips.push(ReactionChip {
                        emoji: Str::from(emoji_s.clone()),
                        count: 1,
                        chosen: true,
                    });
                    r.my_reaction = Str::from(emoji_s.clone());
                }
            });
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
        if self.client_id.get() == 0 {
            // Demo: flip the flag locally the way
            // `updateChatIsMarkedAsUnread` would, then recount badges.
            self.update_chat_row(chat_id, |r| r.marked_unread = !r.marked_unread);
            self.demo_recount_folders();
            return;
        }
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
    /// Mark every loaded row whose text contains `q` (case-insensitive)
    /// `search_hit` and build its `search_styled` highlight variant.
    /// Returns the matched ids in row order — the prev/next buttons and
    /// the "n/N" counter index into it.
    pub(crate) fn mark_search_hits(&self, q: &str) -> Vec<i64> {
        let lower_q = q.to_lowercase();
        let mut list = self.messages.snapshot();
        let mut ids = Vec::new();
        for r in list.iter_mut() {
            let hit = !lower_q.is_empty()
                && r.text.to_lowercase().contains(&lower_q);
            r.search_hit = hit;
            r.search_styled = if hit {
                let base = if r.styled.is_empty() {
                    StyledStr::plain(r.text.clone())
                } else {
                    r.styled.clone()
                };
                highlight_styled(
                    &base,
                    q,
                    Color::from(TertiaryContainer),
                    Color::from(SelectionForeground),
                )
            } else {
                StyledStr::empty()
            };
            if hit {
                ids.push(r.id);
            }
        }
        self.set_messages(list);
        ids
    }

    /// r35: jump the scroll to the previous/next search match and flash it
    /// (Telegram Desktop's ^ up/down match navigation).
    pub fn chat_search_next(&self) {
        let ids = self.chat_match_ids.snapshot();
        if ids.is_empty() {
            return;
        }
        let pos = (self.chat_search_pos.snapshot() + 1) % ids.len();
        self.chat_search_pos.set(pos);
        self.jump_to_message(ids[pos]);
    }

    pub fn chat_search_prev(&self) {
        let ids = self.chat_match_ids.snapshot();
        if ids.is_empty() {
            return;
        }
        let pos = (self.chat_search_pos.snapshot() + ids.len() - 1) % ids.len();
        self.chat_search_pos.set(pos);
        self.jump_to_message(ids[pos]);
    }

    /// r35: the floating `@` button jumps to the first unread mention in
    /// the open chat and clears its mention count (Desktop parity).
    pub fn mention_jump(&self) {
        let target = self
            .messages
            .snapshot()
            .iter()
            .find(|r| r.mentions_me)
            .map(|r| r.id);
        if let Some(id) = target {
            self.jump_to_message(id);
        }
        let open = self.open_chat.get();
        self.update_chat_row(open, |r| r.unread_mentions = 0);
    }

    /// r39: the floating `❤` button jumps to the first message carrying
    /// unread reactions and clears the chat's unread-reaction count
    /// (Desktop parity with the `@` mention jump).
    pub fn reaction_jump(&self) {
        let target = self
            .messages
            .snapshot()
            .iter()
            .find(|r| r.unread_reaction)
            .map(|r| r.id);
        if let Some(id) = target {
            self.update_message_row(id, |r| r.unread_reaction = false);
            self.jump_to_message(id);
        }
        let open = self.open_chat.get();
        self.update_chat_row(open, |r| r.unread_reactions = 0);
    }

    /// r35: Desktop's double-tap quick-react — toggles the default ❤️.
    /// Inert while multi-select is active: there a double-tap is just two
    /// selection clicks, not a reaction.
    pub fn quick_react(&self, row: &MessageRow) {
        if !self.selected_msgs.snapshot().is_empty() {
            return;
        }
        self.toggle_reaction(row, "❤️");
    }

    pub fn run_chat_search(&self, query: Str) {
        self.chat_search.set(query.clone());
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        if query.is_empty() {
            self.chat_search_results.set(Vec::new());
            self.members.set(Vec::new());
            self.chat_match_ids.set(Vec::new());
            self.chat_search_pos.set(0);
            self.mark_search_hits("");
            return;
        }
        // Highlight every match inside the loaded window (Desktop keeps all
        // matches lit and navigates between them); match ids feed the n/N
        // counter + prev/next buttons.
        let ids = self.mark_search_hits(&query);
        self.chat_match_ids.set(ids.clone());
        self.chat_search_pos.set(0);
        if let Some(&first) = ids.first() {
            self.jump_to_message(first);
        }
        if self.client_id.get() == 0 {
            // Demo store: the local pass already found every match in the
            // loaded window; mirror them into the dropdown result rows.
            let list = self.messages.snapshot();
            self.chat_search_results.set(
                list.into_iter()
                    .filter(|r| ids.contains(&r.id))
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
        // Fast path — the target is already in the loaded window (the common
        // case for a reply quote): highlight + scroll without a fetch.
        let mut list = self.messages.snapshot();
        if let Some(pos) = list.iter().position(|r| r.id == message_id) {
            for (i, r) in list.iter_mut().enumerate() {
                r.highlighted = Some(i) == Some(pos);
            }
            self.set_messages(list);
            self.highlight_msg.set(message_id);
            self.scroll.scroll_to(pos);
            return;
        }
        if self.client_id.get() == 0 {
            // Demo: the id isn't in the seeded window — a real client would
            // fetch a window around it; there is nothing to fetch.
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
                let mut list = store.messages.snapshot();
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
        let mut s = self.composer.snapshot().to_string();
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
            msg_id: row.id,
            caption: row.text.clone(),
            from: row.sender.clone(),
        }));
    }

    /// Step the open viewer to the chat's previous/next media message
    /// (Desktop's ‹ › chevrons and ←/→ keys). `delta` -1 = earlier, +1 =
    /// later. Returns false at either end.
    pub fn viewer_step(&self, delta: i64) -> bool {
        let Some(cur) = self.viewer.snapshot() else {
            return false;
        };
        let msgs = self.messages.snapshot();
        let ids: Vec<i64> = msgs
            .iter()
            .filter(|r| r.media_file != 0 || r.play_file != 0)
            .map(|r| r.id)
            .collect();
        let Some(i) = ids.iter().position(|&id| id == cur.msg_id) else {
            return false;
        };
        let j = i as i64 + delta;
        if j < 0 || j >= ids.len() as i64 {
            return false;
        }
        let Some(row) = msgs.iter().find(|r| r.id == ids[j as usize]).cloned() else {
            return false;
        };
        self.open_viewer(&row);
        true
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
        let n = self.poll_option_count.snapshot();
        if n < self.poll_option_fields.len() {
            self.poll_option_count.set(n + 1);
        }
    }

    /// Remove the option at `ix`, shifting later texts up; a poll keeps at
    /// least two options.
    pub fn remove_poll_option(&self, ix: usize) {
        let n = self.poll_option_count.snapshot();
        if n <= 2 || ix >= n {
            return;
        }
        for i in ix..n - 1 {
            let next = self.poll_option_fields[i + 1].snapshot();
            self.poll_option_fields[i].set(next);
        }
        self.poll_option_fields[n - 1].set_from("");
        self.poll_option_count.set(n - 1);
        let correct = self.poll_correct.snapshot();
        if correct >= n - 1 {
            self.poll_correct.set(0);
        }
    }

    /// Send the poll via `sendMessage inputMessagePoll`; no-op until the
    /// question and at least two non-empty options are filled.
    pub fn send_poll(&self) {
        let chat_id = self.open_chat.get();
        let question = self.poll_question.snapshot().to_string().trim().to_string();
        let n = self.poll_option_count.snapshot();
        let options: Vec<String> = (0..n)
            .map(|i| {
                self.poll_option_fields[i]
                    .snapshot()
                    .to_string()
                    .trim()
                    .to_string()
            })
            .filter(|o| !o.is_empty())
            .collect();
        if chat_id == 0 || question.is_empty() || options.len() < 2 {
            return;
        }
        let quiz = self.poll_quiz.snapshot();
        let correct = self.poll_correct.snapshot().min(options.len() - 1) as i32;
        let multiple = self.poll_multiple.snapshot();
        let anonymous = self.poll_anonymous.snapshot();
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
                    let accent = match &sender {
                        enums::MessageSender::User(u) => store
                            .users
                            .borrow()
                            .get(&u.user_id)
                            .map(|u| u.accent_color_id)
                            .unwrap_or(-1),
                        enums::MessageSender::Chat(c) => store
                            .chat_objs
                            .borrow()
                            .get(&c.chat_id)
                            .map(|ch| ch.accent_color_id)
                            .unwrap_or(-1),
                    };
                    rows.push(MemberRow {
                        key,
                        name: if name.is_empty() { "Blocked".into() } else { name.into() },
                        status: "".into(),
                        sender: sender.clone(),
                        username: "".into(),
                        photo: 0,
                        accent,
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
        let open = !self.scheduled_open.snapshot();
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
        // Only rows that exist as selectable messages can be selected —
        // a folded service line's id, a missing id, or a service row
        // itself all refuse (Desktop doesn't select service events).
        if !self
            .messages
            .snapshot()
            .iter()
            .any(|r| r.id == message_id && !r.is_service)
        {
            return;
        }
        let mut sel = self.selected_msgs.snapshot();
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

    /// Batch-delete the selected messages — through the same confirmation
    /// card as single deletes (Desktop asks before every delete).
    pub fn delete_selected(&self) {
        self.ask_delete_selected();
    }

    /// Stage the selected batch for forwarding; the next chat tap delivers
    /// it (same pick-a-chat flow as single-message forward).
    pub fn forward_selected(&self) {
        let ids = self.selected_msgs.snapshot();
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
            let mut list = self.messages.snapshot();
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
        if self.loading_history.snapshot() || self.no_more_history.snapshot() {
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
        let last = self.messages.snapshot().len().saturating_sub(1);
        self.scroll.scroll_to(last);
    }

    /// Whether a newly arrived row pulls the viewport to the tail: your own
    /// send always does; an incoming row only when the chat has no pending
    /// unread divider — Telegram Desktop anchors you in unread context
    /// instead of yanking to the tail. (DOGFOOD r34-2: a true viewport
    /// anchor needs a scroll-offset readback `ScrollController` does not
    /// expose, so the divider is the available proxy.)
    pub(crate) fn follows_tail(&self, outgoing: bool) -> bool {
        outgoing || !self.messages.snapshot().iter().any(|r| r.unread_divider)
    }

    /// Telegram Desktop's open position: a chat with unread messages lands
    /// on the "Unread messages" divider row; a fully read chat lands on the
    /// newest message.
    pub(crate) fn scroll_to_open(&self, chat_id: i64) {
        let unread = self
            .chats
            .snapshot()
            .iter()
            .find(|r| r.id == chat_id)
            .map(|r| r.unread)
            .unwrap_or(0);
        if unread > 0 {
            let list = self.messages.snapshot();
            if let Some(pos) = list.iter().position(|r| r.unread_divider) {
                self.scroll.scroll_to(pos);
                return;
            }
        }
        self.scroll_bottom();
    }

    /// Open a peer's profile card — Desktop fires this from a sender's
    /// avatar/name and from a "Forwarded from" badge. `user_id` covers
    /// users, `chat_id` channels/chats (hidden senders carry neither and
    /// are no-ops). A store without a TDLib client (demo/tests)
    /// synthesizes the card from the display name.
    pub fn open_peer(&self, user_id: i64, chat_id: i64, name: Str) {
        if self.client_id.get() == 0 {
            if user_id == 0 && chat_id == 0 {
                return;
            }
            self.profile.set(Some(ProfileCard {
                user_id,
                name,
                ..Default::default()
            }));
            self.nav.push(Route::Profile);
            return;
        }
        if user_id != 0 {
            self.open_profile(user_id);
            return;
        }
        if chat_id == 0 {
            return;
        }
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            let mut card = ProfileCard {
                name,
                ..Default::default()
            };
            if let Ok(enums::Chat::Chat(c)) = functions::get_chat(chat_id, client).await {
                card.name = c.title.into();
            }
            store.profile.set(Some(card));
            store.nav.push(Route::Profile);
        })
        .detach();
    }

    /// Open a message URL in the system handler (robius-open — the same
    /// mechanism `waterui::link` uses). `link_opened` records the request
    /// so tests can assert it without a browser.
    pub fn open_link(&self, url: Str) {
        if url.is_empty() {
            return;
        }
        self.link_opened.set(url.clone());
        if let Err(e) = robius_open::Uri::new(url.as_str()).open() {
            error!("open_link {url}: {e:?}");
        }
    }

    /// Trailing `:token` (≥2 chars) in the composer draft — Desktop's
    /// `:smile` emoji autocomplete trigger. Same scan as `mention_token`.
    pub fn emoji_token(s: &str) -> Option<String> {
        let b = s.as_bytes();
        let mut i = b.len();
        while i > 0 {
            let c = b[i - 1];
            if c == b':' {
                return if (i == 1 || b[i - 2].is_ascii_whitespace()) && s.len() - i >= 2 {
                    Some(s[i..].to_string())
                } else {
                    None
                };
            }
            if !c.is_ascii_lowercase() && c != b'_' {
                return None;
            }
            i -= 1;
        }
        None
    }

    /// `:token` → matching shortcode suggestions (prefix match on the
    /// static table).
    pub fn emoji_suggest(s: &str) -> Vec<EmojiSug> {
        let Some(tok) = Self::emoji_token(s) else {
            return Vec::new();
        };
        EMOJI_SHORTCODES
            .iter()
            .filter(|(name, _)| name.starts_with(tok.as_str()))
            .take(6)
            .map(|(name, emoji)| EmojiSug {
                name: Str::from(*name),
                emoji: Str::from(*emoji),
            })
            .collect()
    }

    /// Replace the trailing `:token` in the composer with `emoji`.
    pub fn apply_emoji(&self, emoji: &str) {
        let cur = self.composer.snapshot().to_string();
        if let Some(tok) = Self::emoji_token(&cur) {
            let at = cur.len() - tok.len() - 1;
            let mut s = String::with_capacity(at + emoji.len() + 1);
            s.push_str(&cur[..at]);
            s.push_str(emoji);
            s.push(' ');
            self.composer.set_from(s);
        }
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

    /// Composer Return (`TextField::on_submit`, waterui#1265): accept the
    /// top completion when the @/: popup is open — Enter picks the
    /// suggestion rather than sending, like Desktop — otherwise send.
    pub fn submit_composer(&self) {
        if self.accept_top_completion() {
            return;
        }
        self.send();
    }

    /// Apply the first visible completion row (emoji or @mention). Returns
    /// true when a popup was open and consumed the Enter.
    pub fn accept_top_completion(&self) -> bool {
        if self.completion_off.snapshot() {
            return false;
        }
        let q = self.composer.snapshot();
        if let Some(sug) = Self::emoji_suggest(&q).into_iter().next() {
            self.apply_emoji(&sug.emoji);
            return true;
        }
        if let Some(tok) = Self::mention_token(&q) {
            let t = tok.to_lowercase();
            if let Some(m) = self.members.snapshot().iter().find(|m| {
                !m.username.is_empty()
                    && (m.username.to_lowercase().starts_with(&t)
                        || m.name.to_lowercase().starts_with(&t)
                        || t.is_empty())
            }) {
                let u = m.username.clone();
                self.apply_mention(&u);
                return true;
            }
        }
        false
    }

    /// Composer Escape (Desktop order): dismiss the completion popup, then
    /// cancel edit, then cancel reply. Returns true when it consumed one.
    pub fn composer_escape(&self) -> bool {
        if !self.completion_off.snapshot()
            && (!Self::emoji_suggest(&self.composer.snapshot()).is_empty()
                || Self::mention_token(&self.composer.snapshot()).is_some())
        {
            self.completion_off.set(true);
            return true;
        }
        if self.editing.snapshot().is_some() {
            self.editing.set(None);
            self.composer.set_from("");
            return true;
        }
        if self.reply_to.snapshot().is_some() {
            self.reply_to.set(None);
            return true;
        }
        false
    }

    /// ArrowUp in an empty composer edits the last editable outgoing
    /// message (Telegram Desktop). Returns false when nothing qualifies.
    pub fn edit_last_own(&self) -> bool {
        if !self.composer.snapshot().is_empty() || self.editing.snapshot().is_some() {
            return false;
        }
        let Some(row) = self
            .messages
            .snapshot()
            .iter()
            .rev()
            .find(|r| r.outgoing && r.can_edit && !r.is_service)
            .cloned()
        else {
            return false;
        };
        self.start_edit(&row);
        true
    }

    /// Sidebar ArrowUp/Down: move `list_selection` to the adjacent row in
    /// the current filtered order (search filter applied). Pointer clicks
    /// focus a row's inner press slot rather than the ListRow, so the
    /// framework's own arrow handling never sees them (hydrolysis#220) —
    /// this bubbling `on_key_press` covers that path. Returns whether the
    /// selection moved.
    pub fn list_nav(&self, down: bool) -> bool {
        let q = self.search.snapshot().to_string().to_lowercase();
        let rows: Vec<i64> = self
            .chats
            .snapshot()
            .iter()
            .filter(|r| {
                q.is_empty()
                    || r.title.to_lowercase().contains(&q)
                    || r.preview.to_lowercase().contains(&q)
            })
            .map(|r| r.id)
            .collect();
        if rows.is_empty() {
            return false;
        }
        let cur = self
            .list_selection
            .snapshot()
            .or_else(|| self.selected.snapshot());
        let next = match cur.and_then(|id| rows.iter().position(|&r| r == id)) {
            Some(i) => {
                let j = if down {
                    (i + 1).min(rows.len() - 1)
                } else {
                    i.saturating_sub(1)
                };
                rows[j]
            }
            None => {
                if down {
                    rows[0]
                } else {
                    rows[rows.len() - 1]
                }
            }
        };
        if Some(next) == cur {
            return false;
        }
        self.list_selection.set(Some(next));
        true
    }

    /// Enter in the sidebar search opens the top result — first matching
    /// local chat, else the first global message hit (Desktop behaviour).
    pub fn open_top_hit(&self) -> bool {
        let q = self.search.snapshot().to_string().to_lowercase();
        if q.is_empty() {
            return false;
        }
        if let Some(r) = self.chats.snapshot().iter().find(|r| {
            r.title.to_lowercase().contains(&q) || r.preview.to_lowercase().contains(&q)
        }) {
            self.select_chat(r.id);
            return true;
        }
        if let Some(h) = self.msg_results.snapshot().first().cloned() {
            self.open_hit(&h);
            return true;
        }
        false
    }

    /// Folder chip "Mark all as read" (Desktop's folder context menu):
    /// clears unread on every chat in the folder (-1 = archive).
    pub fn mark_folder_read(&self, folder: i32) {
        let ids: Vec<i64> = self
            .chats
            .snapshot()
            .iter()
            .filter(|r| {
                if folder == -1 {
                    r.in_archive
                } else {
                    r.folder_id == folder
                }
            })
            .map(|r| r.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.mark_read(id);
        }
        self.notify("Marked all as read");
    }

    /// Shared composer send path. `options` applies to text sends only —
    /// attachments always go immediately for now.
    fn send_opt(&self, options: Option<types::MessageSendOptions>) {
        if !self.attach.snapshot().is_empty() {
            self.send_attachment();
            return;
        }
        let chat_id = self.open_chat.get();
        let text = self.composer.snapshot().to_string();
        if chat_id == 0 || text.trim().is_empty() {
            return;
        }
        // Desktop's composer markdown is applied at send time, not while
        // typing (the draft stays plain text).
        let ft = parse_markdown(&text);
        if let Some(msg_id) = self.editing.snapshot() {
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
                            text: ft,
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
        let reply_to_id = self.reply_to.snapshot();
        let reply = reply_to_id.map(|id| {
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
        if client == 0 {
            // Demo: optimistic local echo so the send path — including
            // markdown entities — is exercisable without a connection.
            self.demo_echo(ft, reply_to_id);
            return;
        }
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                reply,
                options,
                enums::InputMessageContent::InputMessageText(
                    types::InputMessageText {
                        text: ft,
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

    /// Demo-mode optimistic echo for `send_opt` — appends the outgoing
    /// MessageRow locally (same row shape `set_messages` produces) so the
    /// send path — including markdown entities — is exercisable without a
    /// TDLib connection.
    fn demo_echo(&self, ft: types::FormattedText, reply_to_id: Option<i64>) {
        let mut rows = self.messages.snapshot();
        let next_id = rows.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        let reply_excerpt = reply_to_id
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| {
                let t = r.text.to_string();
                Str::from(t.chars().take(28).collect::<String>())
            })
            .unwrap_or_default();
        let spoiler = has_spoiler_entity(&ft);
        let mask = spoiler.then(|| Color::from(AccentForeground));
        let now = chrono::Local::now().timestamp() as i32;
        rows.push(MessageRow {
            id: next_id,
            sender: Str::from(""),
            sender_accent: -1,
            text: Str::from(ft.text.clone()),
            time: fmt_time(now),
            outgoing: true,
            read_out: false,
            can_edit: true,
            reply_excerpt,
            reply_to_id: reply_to_id.unwrap_or(0),
            media_file: 0,
            play_file: 0,
            media_label: Str::from(""),
            media_secs: 0,
            reaction_chips: Vec::new(),
            failed: false,
            pending: true,
            highlighted: false,
            unread_divider: false,
            unread_reaction: false,
            group_first: true,
            group_last: true,
            avatar_col: false,
            show_avatar: false,
            sender_photo: 0,
            day_header: false,
            day_label: Str::from(""),
            edited: false,
            my_reaction: Str::from(""),
            styled: styled_from_formatted_mask(&ft, mask),
            styled_open: if spoiler {
                styled_from_formatted_mask(&ft, None)
            } else {
                StyledStr::empty()
            },
            has_spoiler: spoiler,
            search_hit: false,
            search_styled: StyledStr::empty(),
            mentions_me: false,
            link_site: Str::from(""),
            link_title: Str::from(""),
            link_desc: Str::from(""),
            forwarded_from: Str::from(""),
            sender_user: self.my_id.get(),
            sender_chat: 0,
            fwd_user: 0,
            fwd_chat: 0,
            link_url: Str::from(""),
            poll: None,
            is_service: false,
            view_count: 0,
            author_sig: Str::from(""),
            album_id: 0,
            album_files: Vec::new(),
            kb_rows: Vec::new(),
            day: local_day(now),
        });
        self.set_messages(rows);
        // Outgoing always follows the tail (same as the Update::NewMessage path).
        self.scroll_bottom();
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
        let urls = self.attach.snapshot();
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
        let preview_caption = self.attach_caption.snapshot().to_string();
        let caption = if preview_caption.is_empty() {
            self.composer.snapshot().to_string()
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
        if self.video_recording.snapshot() {
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

    /// Telegram Desktop never deletes silently: every delete goes through
    /// a confirmation card that, in private chats, offers "Also delete for
    /// <peer>" (the `deleteMessages` `revoke` flag). The card carries the
    /// pending ids; `confirm_delete_now` runs the delete.
    pub fn ask_delete(&self, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        // The peer checkbox only makes sense in a private chat; groups and
        // channels revoke own messages without asking.
        let peer = self
            .chats
            .snapshot()
            .iter()
            .find(|r| r.id == self.open_chat.get())
            .filter(|r| r.kind_icon.as_str() == "person")
            .map(|r| r.title.clone())
            .unwrap_or_default();
        self.delete_revoke.set(false);
        self.confirm_delete.set(Some(DeleteAsk { ids, peer }));
    }

    pub fn ask_delete_message(&self, message_id: i64) {
        self.ask_delete(vec![message_id]);
    }

    /// Ask first (the batch bar's trash icon). The selection survives the
    /// card so the card can report "Delete N messages?" — cleared on
    /// confirm/cancel.
    pub fn ask_delete_selected(&self) {
        self.ask_delete(self.selected_msgs.snapshot());
    }

    pub fn dismiss_delete(&self) {
        self.confirm_delete.set(None);
        self.selected_msgs.set(Vec::new());
    }

    /// Run the pending delete with the checkbox's `revoke` value.
    pub fn confirm_delete_now(&self) {
        let Some(ask) = self.confirm_delete.snapshot() else {
            return;
        };
        let revoke = self.delete_revoke.snapshot();
        self.confirm_delete.set(None);
        self.selected_msgs.set(Vec::new());
        let chat_id = self.open_chat.get();
        if self.client_id.get() == 0 {
            // Demo: drop the rows the way a real updateDeleteMessages would.
            let mut msgs = self.messages.snapshot();
            msgs.retain(|r| !ask.ids.contains(&r.id));
            self.messages.set(msgs);
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ =
                functions::delete_messages(chat_id, ask.ids, revoke, client).await;
        })
        .detach();
    }

    /// Direct delete kept for tests and non-interactive paths: goes through
    /// the same revoke plumbing with Desktop's default (revoke for
    /// everyone in private chats is user-chosen via `ask_delete`).
    pub fn delete_message(&self, message_id: i64) {
        self.ask_delete_message(message_id);
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

    /// Reveal a message's spoiler spans (Desktop: tap the mask).
    pub fn reveal_spoiler(&self, message_id: i64) {
        self.revealed_spoilers.with_mut(|v| {
            if !v.contains(&message_id) {
                v.push(message_id);
            }
        });
    }

    /// Reactive `true` while this row's spoiler spans stay masked.
    pub fn spoiler_masked(&self, message_id: i64) -> Computed<bool> {
        self.revealed_spoilers
            .map(move |v: Vec<i64>| !v.contains(&message_id))
            .computed()
    }

    pub fn start_edit(&self, row: &MessageRow) {
        if row.can_edit {
            self.editing.set(Some(row.id));
            self.composer.set(row.text.clone());
        }
    }

    /// Push a transient toast message: the root view routes `notice` into
    /// the window's `SnackbarManager`. The sequence counter lets the same
    /// text fire again.
    pub fn notify(&self, msg: impl Into<Str>) {
        let (seq, _) = self.notice.snapshot();
        self.notice.set((seq + 1, msg.into()));
    }

    /// Map `chatActionBar*` to the row's (kind, aux title) strings.
    fn action_bar_parts(ab: Option<&enums::ChatActionBar>) -> (Str, Str) {
        match ab {
            Some(enums::ChatActionBar::ReportSpam(_)) => ("report_spam".into(), "".into()),
            Some(enums::ChatActionBar::ReportAddBlock(_)) => {
                ("report_add_block".into(), "".into())
            }
            Some(enums::ChatActionBar::AddContact) => ("add_contact".into(), "".into()),
            Some(enums::ChatActionBar::SharePhoneNumber) => {
                ("share_phone".into(), "".into())
            }
            Some(enums::ChatActionBar::InviteMembers) => {
                ("invite_members".into(), "".into())
            }
            Some(enums::ChatActionBar::JoinRequest(j)) => {
                ("join_request".into(), j.title.clone().into())
            }
            None => ("".into(), "".into()),
        }
    }

    /// The open chat's action-bar triple (kind, aux title, peer user id),
    /// or None when the chat has no bar (normal composer).
    pub fn open_action_bar(&self) -> Option<(Str, Str, i64)> {
        let id = self.selected.snapshot()?;
        self.chats
            .snapshot()
            .iter()
            .find(|r| r.id == id)
            .filter(|r| !r.action_bar.is_empty())
            .map(|r| (r.action_bar.clone(), r.action_title.clone(), r.peer_user))
    }

    /// Clear the open chat's action bar locally and call
    /// `removeChatActionBar` — Desktop's ✕ / hide path.
    pub fn action_bar_dismiss(&self) {
        let Some(id) = self.selected.snapshot() else {
            return;
        };
        self.update_chat_row(id, |r| {
            r.action_bar = "".into();
            r.action_title = "".into();
        });
        if let Some(c) = self.chat_objs.borrow_mut().get_mut(&id) {
            c.action_bar = None;
        }
        let client = self.client_id.get();
        if client != 0 {
            spawn_local(async move {
                let _ = functions::remove_chat_action_bar(id, client).await;
            })
            .detach();
        }
    }

    /// Button press on the open chat's action bar. `n` indexes the
    /// bar kind's buttons in render order (last rendered = dismiss).
    pub fn action_bar_run(&self, n: i32) {
        let Some((kind, _title, peer)) = self.open_action_bar() else {
            return;
        };
        let Some(chat_id) = self.selected.snapshot() else {
            return;
        };
        let client = self.client_id.get();
        match (kind.as_str(), n) {
            // report_spam: [Report spam and leave, Hide].
            ("report_spam", 0) => {
                self.notify("Reported as spam");
                if client != 0 {
                    spawn_local(async move {
                        let _ = functions::report_chat(
                            chat_id,
                            String::new(),
                            Vec::new(),
                            "spam".to_string(),
                            client,
                        )
                        .await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            // report_add_block: [Report, Add to blocklist, Hide].
            ("report_add_block", 0) => {
                self.notify("Reported");
                if client != 0 {
                    spawn_local(async move {
                        let _ = functions::report_chat(
                            chat_id,
                            String::new(),
                            Vec::new(),
                            String::new(),
                            client,
                        )
                        .await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            ("report_add_block", 1) => {
                self.notify("User blocked");
                if client != 0 && peer != 0 {
                    let sender = enums::MessageSender::User(types::MessageSenderUser {
                        user_id: peer,
                    });
                    spawn_local(async move {
                        let _ = functions::set_message_sender_block_list(
                            sender,
                            Some(enums::BlockList::Main),
                            client,
                        )
                        .await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            // add_contact: [Add contact, Hide].
            ("add_contact", 0) => {
                self.notify("Added to contacts");
                if client != 0 && peer != 0 {
                    let name = self
                        .chats
                        .snapshot()
                        .iter()
                        .find(|r| r.id == chat_id)
                        .map(|r| r.title.to_string())
                        .unwrap_or_default();
                    spawn_local(async move {
                        let _ = functions::add_contact(
                            peer,
                            types::ImportedContact {
                                phone_number: String::new(),
                                first_name: name,
                                last_name: String::new(),
                                note: types::FormattedText::default(),
                            },
                            false,
                            client,
                        )
                        .await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            // share_phone: [Share my phone number, Hide].
            ("share_phone", 0) => {
                self.notify("Phone number shared");
                if client != 0 && peer != 0 {
                    spawn_local(async move {
                        let _ = functions::share_phone_number(peer, client).await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            // invite_members: [Invite members, Hide] — opens the members
            // panel where invites live.
            ("invite_members", 0) => {
                self.members_open.set(true);
                self.action_bar_dismiss();
            }
            // join_request: [Apply to join, Hide].
            ("join_request", 0) => {
                self.notify("Join request sent");
                if client != 0 {
                    spawn_local(async move {
                        let _ = functions::join_chat(chat_id, client).await;
                    })
                    .detach();
                }
                self.action_bar_dismiss();
            }
            // Any trailing button hides the bar (Desktop's ✕).
            _ => self.action_bar_dismiss(),
        }
    }

    /// Inline-keyboard button tap — dispatch by `inlineKeyboardButtonType`.
    pub fn inline_tap(&self, message_id: i64, kind: &KbKind) {
        match kind {
            KbKind::Url(u) => self.open_link(u.clone()),
            KbKind::Copy(t) => {
                let text = t.to_string();
                if !text.is_empty()
                    && let Ok(mut cb) = arboard::Clipboard::new()
                {
                    let _ = cb.set_text(text);
                }
                self.notify("Copied");
            }
            KbKind::SwitchInline(q) => {
                let mut q = q.to_string();
                if !q.is_empty() && !q.ends_with(' ') {
                    q.push(' ');
                }
                self.composer.set_from(q);
            }
            KbKind::User(uid) => {
                let uid = *uid;
                if uid != 0 {
                    self.open_profile(uid);
                }
            }
            KbKind::Callback(data) => {
                let data = data.clone();
                let Some(chat_id) = self.selected.snapshot() else {
                    return;
                };
                if self.client_id.get() == 0 {
                    // Demo: the bot would answer via getCallbackQueryAnswer;
                    // show the toast shape a callback answer produces.
                    self.notify("Callback sent to bot");
                    return;
                }
                let store = self.clone();
                let client = self.client_id.get();
                spawn_local(async move {
                    let payload = enums::CallbackQueryPayload::Data(
                        types::CallbackQueryPayloadData { data: data.to_string() },
                    );
                    if let Ok(enums::CallbackQueryAnswer::CallbackQueryAnswer(ans)) =
                        functions::get_callback_query_answer(
                            chat_id, message_id, payload, client,
                        )
                        .await
                    {
                        if !ans.url.is_empty() {
                            store.open_link(ans.url.clone().into());
                        } else if !ans.text.is_empty() {
                            // show_alert answers open a dialog in Desktop;
                            // the snackbar carries the same text here.
                            store.notify(ans.text.clone());
                        }
                    }
                })
                .detach();
            }
            KbKind::Unsupported => {
                self.notify("This button type is not supported yet");
            }
        }
    }

    /// Context-menu "Info": build the message-info card. Real accounts
    /// ask `getMessageReadDate`/`getMessageViewers`; demo synthesizes the
    /// same fields from the row.
    pub fn open_msg_info(&self, row: &MessageRow) {
        let sent = if row.day_label.is_empty() {
            format!("Sent {}", row.time)
        } else {
            format!("Sent {} {}", row.day_label, row.time)
        };
        let views = if row.view_count > 0 {
            format!("{} views", fmt_count(row.view_count))
        } else {
            String::new()
        };
        let info = MsgInfo {
            from: if row.outgoing {
                "You".into()
            } else {
                row.sender.clone()
            },
            sent: sent.into(),
            read: if row.outgoing {
                if row.read_out { format!("Read {}", row.time).into() } else { "Unread".into() }
            } else {
                String::new().into()
            },
            views: views.into(),
            seen: Vec::new(),
        };
        self.msg_info.set(Some(info.clone()));
        let client = self.client_id.get();
        if client == 0 || !row.outgoing {
            // Demo: group posts list member names as seen-by.
            if client == 0 && row.outgoing && row.view_count == 0 {
                let mut demo = info;
                demo.seen = self
                    .members
                    .snapshot()
                    .iter()
                    .take(3)
                    .map(|m| m.name.clone())
                    .collect();
                self.msg_info.set(Some(demo));
            }
            return;
        }
        let chat_id = self.selected.snapshot().unwrap_or(0);
        let store = self.clone();
        let msg_id = row.id;
        spawn_local(async move {
            let read = match functions::get_message_read_date(chat_id, msg_id, client).await {
                Ok(enums::MessageReadDate::Read(r)) => {
                    format!("Read {}", fmt_time(r.read_date))
                }
                Ok(enums::MessageReadDate::Unread) => "Unread".to_string(),
                Ok(enums::MessageReadDate::TooOld) => "Read date too old".to_string(),
                Ok(enums::MessageReadDate::UserPrivacyRestricted)
                | Ok(enums::MessageReadDate::MyPrivacyRestricted) => {
                    "Read date hidden by privacy settings".to_string()
                }
                _ => String::new(),
            };
            let mut seen = Vec::new();
            if let Ok(enums::MessageViewers::MessageViewers(v)) =
                functions::get_message_viewers(chat_id, msg_id, client).await
            {
                seen = v
                    .viewers
                    .iter()
                    .map(|vw| {
                        let name = store.user_name(vw.user_id).to_string();
                        Str::from(format!("{name} · {}", fmt_time(vw.view_date)))
                    })
                    .collect();
            }
            if let Some(mut cur) = store.msg_info.snapshot() {
                cur.read = read.into();
                cur.seen = seen;
                store.msg_info.set(Some(cur));
            }
        })
        .detach();
    }

    pub fn dismiss_msg_info(&self) {
        self.msg_info.set(None);
    }

    /// Folder editor chat-picker checkbox.
    pub fn toggle_folder_chat(&self, id: i64) {
        self.folder_chats.with_mut(|ids| {
            if let Some(i) = ids.iter().position(|c| *c == id) {
                ids.remove(i);
            } else {
                ids.push(id);
            }
        });
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
        self.notify("Text copied");
    }

    /// Translate a message into English (`translateMessageText`); the
    /// bubble swaps the body for the result until "Show original".
    pub fn translate_message(&self, row: &MessageRow) {
        let id = row.id;
        if self.client_id.get() == 0 {
            // Demo: canned translation so the bubble visibly swaps without
            // a TDLib round-trip.
            let t = Str::from(format!("[EN] {}", row.text));
            self.translated.with_mut(|m| {
                m.insert(id, t);
            });
            return;
        }
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        let tr = self.translated.clone();
        spawn_local(async move {
            if let Ok(enums::FormattedText::FormattedText(ft)) =
                functions::translate_message_text(chat_id, id, "en".into(), client).await
            {
                tr.with_mut(|m| {
                    m.insert(id, ft.text.into());
                });
            }
        })
        .detach();
    }

    /// Restore the original text — removes the inline translation.
    pub fn untranslate(&self, message_id: i64) {
        self.translated.with_mut(|m| {
            m.remove(&message_id);
        });
    }

    /// Copy the message's public `t.me` link (`getMessageLink`) to the
    /// clipboard — private chats return an error, hence the notify path.
    pub fn copy_link(&self, row: &MessageRow) {
        let id = row.id;
        if self.client_id.get() == 0 {
            let link = format!("https://t.me/watergram/{id}");
            self.clipboard.set_from(link.clone());
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(link);
            }
            self.notify("Link copied");
            return;
        }
        let chat_id = self.open_chat.get();
        let client = self.client_id.get();
        let store = self.clone();
        spawn_local(async move {
            match functions::get_message_link(chat_id, id, 0, false, false, client).await {
                Ok(enums::MessageLink::MessageLink(l)) => {
                    store.clipboard.set_from(l.link.clone());
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(l.link);
                    }
                    store.notify("Link copied");
                }
                _ => store.notify("No link for this message"),
            }
        })
        .detach();
    }

    /// Copy a photo message's decoded image to the system clipboard
    /// (Desktop's "Copy Image" on photo bubbles).
    pub fn copy_image(&self, row: &MessageRow) {
        let path = self.file_signal(row.media_file).snapshot().to_string();
        if path.is_empty() {
            self.notify("No image to copy");
            return;
        }
        match image::open(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                let data = arboard::ImageData {
                    width: w,
                    height: h,
                    bytes: rgba.into_raw().into(),
                };
                let ok = arboard::Clipboard::new()
                    .and_then(|mut cb| cb.set_image(data))
                    .is_ok();
                self.notify(if ok { "Image copied" } else { "Clipboard unavailable" });
            }
            Err(_) => self.notify("Could not decode image"),
        }
    }

    /// Chat auto-delete timer (`setChatMessageAutoDeleteTime`, seconds).
    pub fn set_auto_delete(&self, chat_id: i64, secs: i32) {
        if self.client_id.get() == 0 {
            self.update_chat_row(chat_id, |r| r.auto_delete = secs);
            let msg: Str = if secs == 0 {
                "Auto-delete off".into()
            } else {
                format!("Auto-delete: {}", auto_delete_label(secs)).into()
            };
            self.notify(msg);
            return;
        }
        let client = self.client_id.get();
        spawn_local(async move {
            let _ = functions::set_chat_message_auto_delete_time(chat_id, secs, client).await;
        })
        .detach();
    }

    /// Batch-copy the selected messages: texts joined by newlines in
    /// message order (Desktop's multi-select Copy keeps the selection).
    pub fn copy_selected(&self) {
        let ids = self.selected_msgs.snapshot();
        if ids.is_empty() {
            return;
        }
        let joined = self
            .messages
            .snapshot()
            .iter()
            .filter(|r| ids.contains(&r.id))
            .map(|r| r.text.to_string())
            .filter(|t| !t.is_empty())
            .collect::<Vec<String>>()
            .join("\n");
        if joined.is_empty() {
            return;
        }
        self.clipboard.set_from(joined.clone());
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(joined);
        }
        let n = self.selected_msgs.snapshot().len();
        self.notify(format!("{n} copied"));
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
            .snapshot()
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
        if self.client_id.get() == 0 {
            // Demo: no notification settings to round-trip — flip the
            // badge style the way `updateChatNotificationSettings` would.
            self.update_chat_row(chat_id, |r| r.muted = !r.muted);
            return;
        }
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
        if self.client_id.get() == 0 {
            // Demo: no `viewMessages` — clear the badges the way the
            // server-side update would.
            self.update_chat_row(chat_id, |r| {
                r.unread = 0;
                r.unread_mentions = 0;
                r.unread_reactions = 0;
                r.marked_unread = false;
            });
            self.demo_recount_folders();
            return;
        }
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

    /// Floating "N unread ↓" catch-up chip: scroll the open chat to its
    /// newest message and mark everything read (Telegram Desktop's
    /// bottom-right button while the chat carries unread).
    pub fn catch_up(&self) {
        let chat_id = self.open_chat.get();
        if chat_id == 0 {
            return;
        }
        self.scroll_bottom();
        // Clear the chip locally so it hides on tap rather than on the
        // server round-trip (the `updateChatReadInbox` that follows agrees).
        self.update_chat_row(chat_id, |r| {
            r.unread = 0;
            r.unread_mentions = 0;
            r.unread_reactions = 0;
            r.marked_unread = false;
        });
        self.demo_recount_folders();
        self.mark_read(chat_id);
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
        let next = !self.archive_mode.snapshot();
        self.set_list(if next { -1 } else { 0 });
    }

    /// Toggle the accounts panel from the sidebar menu.
    pub fn toggle_accounts(&self) {
        self.accounts_open.toggle();
    }

    /// Switch the sidebar list: 0 = All chats, -1 = Archive, n = folder.
    pub fn set_list(&self, list_id: i32) {
        if self.active_folder.snapshot() == list_id && self.archive_mode.snapshot() == (list_id == -1) {
            return;
        }
        self.active_folder.set(list_id);
        self.archive_mode.set(list_id == -1);
        if self.client_id.get() == 0 {
            // Demo: `chat_objs` is empty — filter the seeded roster by
            // `folder_id`/`in_archive` like TDLib's per-list positions do.
            let roster = self.demo_roster.borrow().clone();
            self.chats.set(
                roster
                    .into_iter()
                    .filter(|r| match list_id {
                        -1 => r.in_archive,
                        0 => !r.in_archive,
                        n => r.folder_id == n,
                    })
                    .collect(),
            );
            return;
        }
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
        let cur = self.composer.snapshot().to_string();
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
        if Self::mention_token(&self.composer.snapshot()).is_some()
            && self.members.snapshot().is_empty()
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
                    let (key, name, sender, username, photo, accent) = match &member.member_id {
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
                            let accent = user
                                .as_ref()
                                .map(|u| u.accent_color_id)
                                .unwrap_or(-1);
                            (uid, name, member.member_id.clone(), uname, photo, accent)
                        }
                        enums::MessageSender::Chat(c) => {
                            let (title, photo, accent) = store
                                .chat_objs
                                .borrow()
                                .get(&c.chat_id)
                                .map(|ch| {
                                    (
                                        ch.title.clone(),
                                        ch.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
                                        ch.accent_color_id,
                                    )
                                })
                                .unwrap_or_else(|| (format!("Chat {}", c.chat_id), 0, -1));
                            (c.chat_id, title, member.member_id.clone(), String::new(), photo, accent)
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
                        accent,
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
                            accent: user.accent_color_id,
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
                let q = self.sticker_query.snapshot().to_string();
                self.search_gifs_by(q);
            }
            _ => {}
        }
    }

    /// Toggle the sticker/GIF picker; loads recent stickers + saved GIFs
    /// on first open.
    pub fn toggle_stickers(&self) {
        let open = !self.stickers_open.snapshot();
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
        let phone = self.nc_phone.snapshot().to_string();
        let first = self.nc_first.snapshot().to_string();
        let last = self.nc_last.snapshot().to_string();
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
            .snapshot()
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
                    let mut rows = store.accounts.snapshot();
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
        let mut rows = self.accounts.snapshot();
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
            self.folder_chats.set(Vec::new());
        } else {
            let f = self
                .folders
                .snapshot()
                .iter()
                .find(|f| f.id == id)
                .map(|f| (f.title.clone(), f.include.clone()))
                .unwrap_or_default();
            self.folder_name.set(f.0);
            self.folder_chats.set(f.1);
            // ChatFolderInfo doesn't carry membership — refetch the full
            // folder so the chat picker opens with the real includes.
            let client = self.client_id.get();
            if client != 0 {
                let store = self.clone();
                spawn_local(async move {
                    if let Ok(enums::ChatFolder::ChatFolder(full)) =
                        functions::get_chat_folder(id, client).await
                    {
                        store.folder_chats.set(full.included_chat_ids);
                    }
                })
                .detach();
            }
        }
        self.folder_open.set(true);
    }

    /// Create or rename a chat folder from the editor form.
    pub fn save_folder(&self) {
        let client = self.client_id.get();
        let name = self.folder_name.snapshot().to_string();
        if name.trim().is_empty() {
            return;
        }
        if client == 0 {
            // Demo: apply the picked memberships locally so the folder
            // chips and the picker round-trip without TDLib.
            let included = self.folder_chats.snapshot();
            let fid = self.editing_folder.snapshot();
            let fid = if fid == 0 {
                let new_id = self
                    .folders
                    .snapshot()
                    .iter()
                    .map(|f| f.id)
                    .max()
                    .unwrap_or(1)
                    + 1;
                self.folders.append(FolderRow {
                    id: new_id,
                    title: name.clone().into(),
                    active: false,
                    unread: 0,
                    include: included.clone(),
                });
                new_id
            } else {
                self.folders.with_mut(|fs| {
                    if let Some(f) = fs.iter_mut().find(|f| f.id == fid) {
                        f.title = name.clone().into();
                        f.include = included.clone();
                    }
                });
                fid
            };
            let apply = |r: &mut ChatRow| {
                if r.folder_id == fid {
                    r.folder_id = 0;
                }
                if included.contains(&r.id) {
                    r.folder_id = fid;
                }
            };
            self.chats.with_mut(|rows| rows.iter_mut().for_each(&apply));
            self.demo_roster.borrow_mut().iter_mut().for_each(apply);
            self.demo_recount_folders();
            self.folder_open.set(false);
            self.folder_chats.set(Vec::new());
            return;
        }
        let included = self.folder_chats.snapshot();
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
            included_chat_ids: included.clone(),
            excluded_chat_ids: Vec::new(),
            exclude_muted: false,
            exclude_read: false,
            exclude_archived: false,
            include_contacts: self.folder_contacts.snapshot(),
            include_non_contacts: false,
            include_bots: false,
            include_groups: self.folder_groups.snapshot(),
            include_channels: self.folder_channels.snapshot(),
        };
        let id = self.editing_folder.snapshot();
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
            store.folder_chats.set(Vec::new());
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
            if store.active_folder.snapshot() == id {
                store.set_list(0);
            }
        })
        .detach();
    }

    /// Search stickers by emoji text and show results in the picker.
    pub fn search_stickers_by(&self) {
        let client = self.client_id.get();
        let emoji = self.sticker_query.snapshot().to_string();
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
        let urls = self.chat_avatar_pick.snapshot();
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
        let urls = self.avatar_pick.snapshot();
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
        let mut list = self.messages.snapshot();
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
        let old = self.twofa_old.snapshot().expose().to_string();
        let new = self.twofa_new.snapshot().expose().to_string();
        if old.is_empty() && new.is_empty() {
            self.twofa_note
                .set_from("Enter your current password to disable 2FA");
            return;
        }
        let hint = self.twofa_hint_in.snapshot().to_string();
        let email = self.twofa_email.snapshot().to_string();
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
        match self.lang_strings.snapshot().get(key) {
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
            let mut current = store.lang_id.snapshot().to_string();
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
            let mut packs = store.lang_packs.snapshot();
            for p in packs.iter_mut() {
                p.active = p.id.as_str() == id;
            }
            store.lang_packs.set(packs);
        })
        .detach();
    }

    /// Merge pushed `Update::LanguagePackStrings` for the applied pack.
    fn merge_language_strings(&self, u: &types::UpdateLanguagePackStrings) {
        if u.language_pack_id != self.lang_id.snapshot().as_str() {
            return;
        }
        let mut map = self.lang_strings.snapshot();
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
            self.edit_first.snapshot().to_string(),
            self.edit_last.snapshot().to_string(),
            self.edit_bio.snapshot().to_string(),
            self.edit_username.snapshot().to_string(),
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
                let mut me = store.me.snapshot();
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
        let title = self.admin_title.snapshot().to_string();
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
        let desc = self.admin_desc.snapshot().to_string();
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
            self.msg_results.set(Vec::new());
            return;
        }
        let client = self.client_id.get();
        let filter = self.search_filter.snapshot();
        if client == 0 {
            // Demo/tests: scan the seeded corpus; the real path fires
            // `searchMessages` for the same "Messages" section.
            self.server_results.set(Vec::new());
            let q = query.to_lowercase();
            let rows = self.chats.snapshot();
            let roster = self.demo_roster.borrow().clone();
            let title_of = |id: i64| {
                rows.iter()
                    .chain(roster.iter())
                    .find(|r| r.id == id)
                    .map(|r| r.title.clone())
                    .unwrap_or_default()
            };
            // Filter tabs: 1 = Chats (no message hits), 2 Media, 3 Files,
            // 4 Links map onto the corpus kind column; 0 = All.
            let want_kind = |k: i32| match filter {
                1 => false,
                2 => k == 1,
                3 => k == 2,
                4 => k == 3,
                _ => true,
            };
            self.msg_results.set(
                self.demo_corpus
                    .borrow()
                    .iter()
                    .filter(|(_, _, sender, text, kind)| {
                        want_kind(*kind)
                            && (text.to_lowercase().contains(&q)
                                || sender.to_lowercase().contains(&q))
                    })
                    .map(|(chat_id, msg_id, sender, text, kind)| MsgHit {
                        key: chat_id * 1_000_000 + msg_id,
                        chat_id: *chat_id,
                        message_id: *msg_id,
                        title: title_of(*chat_id),
                        sender: sender.clone(),
                        snippet: text.clone(),
                        time: "".into(),
                        kind: *kind,
                    })
                    .collect(),
            );
            return;
        }
        let store = self.clone();
        let q_msgs = query.to_string();
        let store_msgs = self.clone();
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
        // "Messages" section — TDLib `searchMessages` over the Main list
        // (the in-chat path uses `searchChatMessages` instead). The
        // Chats tab (1) skips the message request; Media/Files/Links map
        // onto the matching `SearchMessagesFilter`.
        let msg_filter = match filter {
            2 => enums::SearchMessagesFilter::PhotoAndVideo,
            3 => enums::SearchMessagesFilter::Document,
            4 => enums::SearchMessagesFilter::Url,
            _ => enums::SearchMessagesFilter::Empty,
        };
        if filter == 1 {
            self.msg_results.set(Vec::new());
        }
        // Media/Files/Links are message kinds — no chat section there.
        if filter >= 2 {
            self.server_results.set(Vec::new());
        }
        spawn_local(async move {
            if filter == 1 {
                return;
            }
            if let Ok(enums::FoundMessages::FoundMessages(found)) = functions::search_messages(
                Some(enums::ChatList::Main),
                q_msgs,
                String::new(),
                20,
                Some(msg_filter),
                None,
                0,
                0,
                client,
            )
            .await
            {
                let mut hits = Vec::new();
                for m in &found.messages {
                    let title = {
                        let cached = store_msgs
                            .chat_objs
                            .borrow()
                            .get(&m.chat_id)
                            .map(|c| c.title.clone());
                        match cached {
                            Some(t) => t,
                            None => match functions::get_chat(m.chat_id, client).await {
                                Ok(enums::Chat::Chat(c)) => {
                                    store_msgs
                                        .chat_objs
                                        .borrow_mut()
                                        .insert(m.chat_id, c.clone());
                                    c.title
                                }
                                _ => String::new(),
                            },
                        }
                    };
                    let snippet = store_msgs.preview_text(m);
                    let sender = store_msgs.sender_name(&m.sender_id);
                    hits.push(MsgHit {
                        key: m.chat_id.saturating_mul(1_000_000).saturating_add(m.id % 1_000_000),
                        chat_id: m.chat_id,
                        message_id: m.id,
                        title: title.into(),
                        sender,
                        snippet,
                        time: fmt_time(m.date),
                        kind: Self::hit_kind(&m.content),
                    });
                }
                store_msgs.msg_results.set(hits);
            }
        })
        .detach();
    }

    /// Search-tab kind of a message's content: 1 media, 2 file, 3 link,
    /// 0 plain text — mirrors the demo corpus's kind column.
    fn hit_kind(c: &enums::MessageContent) -> i32 {
        match c {
            enums::MessageContent::MessagePhoto(_)
            | enums::MessageContent::MessageVideo(_)
            | enums::MessageContent::MessageAnimation(_)
            | enums::MessageContent::MessageVideoNote(_) => 1,
            enums::MessageContent::MessageDocument(_)
            | enums::MessageContent::MessageAudio(_)
            | enums::MessageContent::MessageVoiceNote(_) => 2,
            enums::MessageContent::MessageText(t) if t.link_preview.is_some() => 3,
            _ => 0,
        }
    }

    /// Sidebar "Messages" hit: open the chat and land on the message with
    /// the same flash-highlight `jump_to_message` uses for quote taps.
    pub fn open_hit(&self, hit: &MsgHit) {
        let chat_id = hit.chat_id;
        let message_id = hit.message_id;
        self.select_chat(chat_id);
        self.jump_to_message(message_id);
    }

    fn upsert_result_row(&self, chat: types::Chat) {
        if self.chats.snapshot().iter().any(|r| r.id == chat.id) {
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
            unread_mentions: 0,
            unread_reactions: 0,
            auto_delete: 0,
            pinned: false,
            muted: false,
            marked_unread: false,
            in_archive: position_in(&chat, &enums::ChatList::Archive).is_some(),
            folder_id: chat
                .positions
                .iter()
                .find_map(|p| match &p.list {
                    enums::ChatList::Folder(f) => Some(f.chat_folder_id),
                    _ => None,
                })
                .unwrap_or(0),
            photo_file: chat.photo.as_ref().map(|p| p.small.id).unwrap_or(0),
            accent: chat.accent_color_id,
            time: "".into(),
            typing: false,
            online: false,
            title_styled: StyledStr::empty(),
            preview_styled: StyledStr::empty(),
            kind_icon: "person".into(),
            action_bar: "".into(),
            action_title: "".into(),
            peer_user: 0,
        };
        let mut list = self.server_results.snapshot();
        if !list.iter().any(|r| r.id == row.id) {
            list.push(row);
            self.server_results.set(list);
        }
    }

    /// Create (or open) a chat from the New Chat page. `kind`: 0 private
    /// (input = @username or user id), 1 group (title), 2 channel (title).
    pub fn create_chat(&self) {
        let input = self.new_chat_input.snapshot().to_string();
        let kind = self.new_chat_kind.snapshot();
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

/// Demo asset: writes a small procedural PNG into the app's data dir so
/// the seeded photo message's `file_signal(1)` resolves a real path and
/// the media slot renders the actual image (the demo path has no TDLib
/// download pipeline). The zlib stream uses stored deflate blocks, so no
/// codec dependency is needed to produce it.
fn demo_photo_png() -> Option<PathBuf> {
    const W: usize = 640;
    const H: usize = 360;
    // RGB dusk scene: vertical gradient sky, a sun disc, a darker
    // shoreline with a soft sine edge.
    let mut raw = Vec::with_capacity(H * (1 + W * 3));
    for y in 0..H {
        raw.push(0u8); // scanline filter: none
        for x in 0..W {
            let t = y as f32 / (H - 1) as f32;
            let (mut r, mut g, mut b) = (28.0 + 110.0 * t, 52.0 + 70.0 * t, 150.0 - 70.0 * t);
            let (dx, dy) = (x as f32 - 470.0, y as f32 - 130.0);
            let sun = 1.0 - ((dx * dx + dy * dy).sqrt() / 58.0).min(1.0);
            if sun > 0.0 {
                r += 210.0 * sun;
                g += 150.0 * sun;
                b += 70.0 * sun;
            }
            if y > 250 {
                let shore = 250.0 + (x as f32 / 18.0).sin() * 6.0;
                if y as f32 > shore {
                    r *= 0.45;
                    g *= 0.6;
                    b *= 0.9;
                }
            }
            raw.extend_from_slice(&[
                r.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                b.clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    // zlib stream of stored (uncompressed) deflate blocks.
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let mut i = 0;
    while i < raw.len() {
        let n = (raw.len() - i).min(65535);
        z.push(u8::from(i + n == raw.len()));
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[i..i + n]);
        i += n;
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = Vec::with_capacity(z.len() + 64);
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(W as u32).to_be_bytes());
    ihdr.extend_from_slice(&(H as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolor RGB
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &z);
    png_chunk(&mut png, b"IEND", &[]);
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("watergram")
        .join("demo");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("photo.png");
    std::fs::write(&path, &png).ok()?;
    Some(path)
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}
