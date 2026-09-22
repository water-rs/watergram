//! Application state and the TDLib update dispatch.
//!
//! All state lives in [`Store`]: `Binding`s for scalars and `Binding<Vec<_>>`s for
//! rows. Every mutation happens on the UI executor — the TDLib receive thread
//! only feeds an `async_channel` that `Store::run` drains here, and TDLib
//! request futures are spawned with `spawn_local`. That keeps every `nami`
//! type (which is `Rc`-backed and `!Send`) on one thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use tdlib_rs::{enums, functions, types};
use waterui::form::secure::Secure;
use waterui::layout::{Point, ScrollController};
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
    pub order: i64,
    pub unread: i32,
    pub pinned: bool,
    pub muted: bool,
    pub marked_unread: bool,
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
    pub media_label: Str,
    pub reactions: Str,
    /// Emoji the current user has chosen on this message, if any.
    pub my_reaction: Str,
    pub failed: bool,
    pub pending: bool,
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
}

#[state]
#[derive(Clone)]
pub struct Store {
    pub client_id: i32,
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
    pub messages: Binding<Vec<MessageRow>>,
    pub scroll: ScrollController<Point>,
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

fn main_position(chat: &types::Chat) -> Option<&types::ChatPosition> {
    chat.positions
        .iter()
        .find(|p| p.list == enums::ChatList::Main)
}

impl Store {
    pub fn new(client_id: i32) -> Self {
        Self {
            client_id,
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
            messages: Binding::<Vec<MessageRow>>::default(),
            scroll: ScrollController::<Point>::new(Point::new(0.0, 0.0)),
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
        }
    }

    /// Kick off the authorization flow once the view is mounted.
    pub fn start(&self) {
        let client = self.client_id;
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
            let (client, id) = (self.client_id, file.id);
            spawn_local(async move {
                let _ = functions::download_file(id, 8, 0, 0, false, client).await;
            })
            .detach();
        }
    }

    /// Same as `want_file` but resolves the `File` object through `getFile`
    /// first — message content only carries raw file ids.
    pub fn want_file_id(&self, file_id: i32) {
        let client = self.client_id;
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

    /// Signal of `file_id -> download progress percent` (0-100).
    pub fn file_progress_signal(&self, file_id: i32) -> Computed<i32> {
        let progress = self.file_progress.clone();
        self.files_version
            .map(move |_| progress.borrow().get(&file_id).copied().unwrap_or(0))
            .computed()
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
    fn content_preview(content: &enums::MessageContent) -> (Str, i32, Str) {
        // (text/caption, media file id, media label)
        match content {
            enums::MessageContent::MessageText(t) => {
                (t.text.text.clone().into(), 0, Str::from(""))
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
            ),
            enums::MessageContent::MessageDocument(d) => (
                format!("Document: {}", d.document.file_name).into(),
                d.document
                    .thumbnail
                    .as_ref()
                    .map(|t| t.file.id)
                    .unwrap_or_default(),
                "Document".into(),
            ),
            enums::MessageContent::MessageAudio(a) => (
                format!("Audio: {}", a.audio.title).into(),
                0,
                "Audio".into(),
            ),
            enums::MessageContent::MessageVoiceNote(_) => {
                ("Voice message".into(), 0, "Voice".into())
            }
            enums::MessageContent::MessageVideoNote(_) => {
                ("Video note".into(), 0, "Video note".into())
            }
            enums::MessageContent::MessageSticker(s) => (
                format!("{} Sticker", s.sticker.emoji).into(),
                0,
                "Sticker".into(),
            ),
            enums::MessageContent::MessageAnimation(_) => ("GIF".into(), 0, "GIF".into()),
            enums::MessageContent::MessageLocation(_) => {
                ("Location".into(), 0, "Location".into())
            }
            enums::MessageContent::MessageContact(c) => (
                format!("Contact: {} {}", c.contact.first_name, c.contact.last_name)
                    .into(),
                0,
                "Contact".into(),
            ),
            enums::MessageContent::MessagePoll(p) => {
                (format!("Poll: {}", p.poll.question.text).into(), 0, "Poll".into())
            }
            enums::MessageContent::MessageCall(c) => (
                format!(
                    "Call ({})",
                    if c.is_video { "video" } else { "voice" }
                )
                .into(),
                0,
                "Call".into(),
            ),
            enums::MessageContent::MessageChatAddMembers(_) => {
                ("New members joined".into(), 0, Str::from(""))
            }
            enums::MessageContent::MessageChatJoinByLink
            | enums::MessageContent::MessageChatJoinByRequest => {
                ("Joined the chat".into(), 0, Str::from(""))
            }
            enums::MessageContent::MessagePinMessage(_) => {
                ("Pinned a message".into(), 0, Str::from(""))
            }
            _ => ("Unsupported message".into(), 0, Str::from("")),
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
        let (mut text, media_file, media_label) = Self::content_preview(&m.content);
        if let enums::MessageContent::MessageText(t) = &m.content {
            text = t.text.text.clone().into();
        }
        if media_file != 0 {
            self.want_file_id(media_file);
        }
        let reply_excerpt = match &m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) => r
                .content
                .as_ref()
                .map(|c| {
                    let (t, _, l) = Self::content_preview(c);
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
        MessageRow {
            id: m.id,
            sender: self.sender_name(&m.sender_id),
            text,
            time: fmt_time(m.date),
            outgoing: m.is_outgoing,
            read_out,
            can_edit: m.is_outgoing,
            reply_excerpt,
            media_file,
            media_label,
            reactions,
            my_reaction,
            failed,
            pending,
        }
    }

#[allow(if_else_view)] // when() requires a signal; conditions here are plain bools
    fn preview_text(&self, m: &types::Message) -> Str {
        let (t, _, label) = Self::content_preview(&m.content);
        if matches!(m.content, enums::MessageContent::MessageText(_)) {
            t
        } else if t.is_empty() {
            format!("[{label}]").into()
        } else {
            format!("[{label}] {t}").into()
        }
    }

    fn upsert_chat(&self, chat: types::Chat) {
        let pos = main_position(&chat).cloned();
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
            order,
            unread: chat.unread_count,
            pinned,
            muted,
            marked_unread: chat.is_marked_as_unread,
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
            self.messages.set(list);
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
        if let Some(p) = positions
            .iter()
            .find(|p| p.list == enums::ChatList::Main)
            .cloned()
        {
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
    pub fn update(&self, u: enums::Update) {
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
                self.update_chat_row(u.chat_id, |r| r.unread = u.unread_count);
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
                    self.messages.set(list);
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
                        self.messages.set(list);
                        self.scroll_bottom();
                    }
                    if !m.is_outgoing {
                        let (client, chat_id, mid) = (self.client_id, m.chat_id, m.id);
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
                    self.messages.set(list);
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
                    let (text, media, label) = Self::content_preview(&u.new_content);
                    self.update_message_row(u.message_id, |r| {
                        r.text = text.clone();
                        r.media_file = media;
                        r.media_label = label.clone();
                    });
                }
            }
            enums::Update::MessageEdited(u) => {
                if u.chat_id == self.open_chat.get() {
                    let client = self.client_id;
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
                    self.messages.set(list);
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
                self.drafts.borrow_mut().insert(u.chat_id, text.into());
            }
            enums::Update::File(f) => {
                self.register_file(&f.file);
            }
            enums::Update::Option(o) => {
                if o.name == "my_id"
                    && let enums::OptionValue::Integer(v) = o.value
                {
                    self.my_id.set(v.value);
                }
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
        let client = self.client_id;
        let db_root = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("watergram");
        let db = db_root.join("db").to_string_lossy().to_string();
        let files = db_root.join("files").to_string_lossy().to_string();
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
        let client = self.client_id;
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
    }

    fn reset(&self) {
        self.chats.set(Vec::new());
        self.messages.set(Vec::new());
        self.server_results.set(Vec::new());
        self.chat_objs.borrow_mut().clear();
        self.users.borrow_mut().clear();
        self.files.borrow_mut().clear();
        self.drafts.borrow_mut().clear();
        self.open_chat.set(0);
        self.selected.set(None);
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
        spawn_local(async move {
            let _ = functions::log_out(client).await;
        })
        .detach();
    }

    pub fn select_chat(&self, chat_id: i64) {
        // Forwarding mode: a pending forwarded message routes the tap to
        // forwardMessages instead of opening the chat.
        if let Some((from, msg_id)) = self.forward_message.get() {
            self.forward_message.set(None);
            let client = self.client_id;
            spawn_local(async move {
                let _ = functions::forward_messages(
                    chat_id,
                    None,
                    from,
                    vec![msg_id],
                    None,
                    false,
                    false,
                    client,
                )
                .await;
            })
            .detach();
            return;
        }
        if self.open_chat.get() == chat_id {
            self.selected.set(Some(chat_id));
            return;
        }
        let prev = self.open_chat.replace(chat_id);
        self.selected.set(Some(chat_id));
        self.messages.set(Vec::new());
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
        if let Some(d) = self.drafts.borrow().get(&chat_id) {
            self.composer.set(d.clone());
        } else {
            self.composer.set_from("");
        }
        let client = self.client_id;
        let store = self.clone();
        spawn_local(async move {
            if prev != 0 {
                let _ = functions::close_chat(prev, client).await;
            }
            let _ = functions::open_chat(chat_id, client).await;
            store.load_history(chat_id, 0, 0).await;
            store.refresh_pinned(chat_id).await;
        })
        .detach();
    }

    /// Fetch the chat's pinned message into `pinned_label`/`pinned_id`.
    #[allow(if_else_view)] // string pick, not a view
    async fn refresh_pinned(&self, chat_id: i64) {
        match functions::get_chat_pinned_message(chat_id, self.client_id).await {
            Ok(enums::Message::Message(m)) => {
                let (t, _, label) = Self::content_preview(&m.content);
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let (client, mid) = (self.client_id, row.id);
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
        let client = self.client_id;
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
            return;
        }
        let client = self.client_id;
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

    /// Scroll/jump to a message: loads a window of history centered on it so
    /// the bubble is on screen (best-effort without per-row scrolling).
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
                store.client_id,
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
                store.messages.set(list);
            }
        })
        .detach();
    }

    async fn load_history(&self, chat_id: i64, from: i64, offset: i32) {
        self.loading_history.set(true);
        if let Ok(enums::Messages::Messages(msgs)) = functions::get_chat_history(
            chat_id,
            from,
            offset,
            50,
            false,
            self.client_id,
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
            self.messages.set(list);
            if offset == 0 && from == 0 {
                self.scroll_bottom();
            }
            if let Some(last) = msgs.messages.iter().flatten().next()
                && !last.is_outgoing
            {
                let client = self.client_id;
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

    fn scroll_bottom(&self) {
        self.scroll.scroll_to(Point::new(0.0, f32::MAX / 2.0));
    }

    pub fn send(&self) {
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
            let client = self.client_id;
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
        let client = self.client_id;
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                reply,
                None,
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
            "mp3" | "ogg" | "m4a" | "flac" | "wav" | "opus" => {
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
        let Some(url) = urls.first() else { return };
        if chat_id == 0 {
            return;
        }
        let path = url.path().to_string();
        if path.is_empty() {
            return;
        }
        let caption = self.composer.get().to_string();
        self.attach.set(Vec::new());
        self.composer.set_from("");
        let client = self.client_id;
        spawn_local(async move {
            let _ = functions::send_message(
                chat_id,
                None,
                None,
                None,
                Self::attachment_content(path, caption),
                client,
            )
            .await;
        })
        .detach();
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
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
        let client = self.client_id;
        spawn_local(async move {
            let _ = functions::view_messages(chat_id, vec![last], None, true, client).await;
        })
        .detach();
    }

    pub fn leave(&self, chat_id: i64) {
        let client = self.client_id;
        spawn_local(async move {
            let _ = functions::leave_chat(chat_id, client).await;
        })
        .detach();
    }

    pub fn run_search(&self, query: Str) {
        if query.is_empty() {
            self.server_results.set(Vec::new());
            return;
        }
        let client = self.client_id;
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
            order: 0,
            unread: 0,
            pinned: false,
            muted: false,
            marked_unread: false,
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
        let client = self.client_id;
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
