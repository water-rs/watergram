//! View layer for Watergram.
//!
//! All views are pure declarations over [`Store`]; every handler takes
//! `store: Store` as a `#[state]` extractor injected once at the root.

use std::num::NonZeroUsize;
use std::time::Duration;

use crate::capture::VideoNoteGpu;
use waterui::Url;
use waterui::accessibility::{AccessibilityRole, AccessibilityState};
use waterui::component::list::{List, ListItem};
use waterui::component::menu::{Command, MenuView, Shortcut};
use waterui::drag_drop::Files;
use waterui::form::picker::file::FilePicker;
use waterui::form::picker::{PickerItem, picker};
use waterui::graphics::GpuContentView;
use waterui::graphics::color::signal_color;
use waterui::graphics::color::{BorderColor, Srgb, WithOpacity};
use waterui::handler::SharedAction;
use waterui::key::{Key, KeyHandling, KeyPress, Modifiers, NamedKey};
use waterui::layout::frame::Frame;
use waterui::media::Photo;
use waterui::navigation::{
    ColumnWidth, NavigationSplitView, NavigationToolbar, NavigationToolbarItem,
    NavigationToolbarPlacement, NavigationView,
};
use waterui::prelude::*;
use waterui::reactive::collection::SignalCollection;
use waterui::reactive::signal::IntoComputed;
use waterui::shape::{Circle, Path, Rectangle, RoundedRectangle, ShapeExt, UnevenRoundedRectangle};
use waterui::snackbar::{Snackbar, SnackbarManager};
use waterui::text::highlight::Language;
use waterui::text::{IntoText, code};
use waterui::theme::color::{
    Accent, AccentContainer, AccentForeground, Error, Foreground, MutedForeground,
    SelectionForeground, Surface, SurfaceVariant, TertiaryContainer,
};
use waterui::video::video_player;
use waterui::widget::condition::when;
use waterui::window::WindowState;
use waterui_backend_core::widget::ModalInteraction;
use waterui_image::ContentMode;

/// Escape-key dispatch for overlay layers: `ModalInteraction` marks a
/// subtree as a modal scope, so the backend sends Escape to the topmost
/// open overlay before any other handler (Telegram Desktop parity).
fn modal_escape(store: Store, close: fn(&Store)) -> ModalInteraction {
    ModalInteraction::new(true, SharedAction::new(move |_: Environment| close(&store)))
}
use tdlib_rs::enums;
use waterui::barcode::Barcode;
use waterui_icons_material_icon as mdi;

use crate::state::{
    AccountRow, BotCmd, ChatRow, CommentRow, DayRow, DeleteAsk, EmojiSug, FolderRow, LangRow,
    MediaChunkRow, MemberRow, MessageRow, PackRow, PinnedRow, PollRow, PrivacyRow, ReactionChip,
    RecentEmoji, Route, Screen, SearchRow, SessionRow, SharedLinkRow, SharedMediaRow, StickerItem,
    Store, Verification, auto_delete_label, highlight_styled,
};
use mdi::account_group;
use mdi::account_plus;
use mdi::alert_circle;
use mdi::alert_decagram;
use mdi::at;
use mdi::bell_off;
use mdi::calendar;
use mdi::camera;
use mdi::check;
use mdi::check_decagram;
use mdi::chevron_down;
use mdi::chevron_left;
use mdi::chevron_right;
use mdi::chevron_up;
use mdi::clock_outline;
use mdi::close;
use mdi::content_copy;
use mdi::delete_sweep;
use mdi::dots_vertical;
use mdi::emoticon;
use mdi::file;
use mdi::file_document_outline;
use mdi::file_gif_box;
use mdi::folder_plus;
use mdi::heart;
use mdi::image;
use mdi::image_outline;
use mdi::information;
use mdi::link_variant;
use mdi::lock;
use mdi::magnify;
use mdi::menu;
use mdi::microphone;
use mdi::paperclip;
use mdi::pencil;
use mdi::pin;
use mdi::plus;
use mdi::poll;
use mdi::reply;
use mdi::send;
use mdi::share_variant;
use waterui::text::styled::StyledStr;

const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const TWO: NonZeroUsize = NonZeroUsize::new(2).unwrap();
const THREE: NonZeroUsize = NonZeroUsize::new(3).unwrap();

// ---------------------------------------------------------------------------
// Root: switch on the top-level screen
// ---------------------------------------------------------------------------

pub fn root(store: Store) -> impl View {
    let inner = store.clone();
    // OS file drops land anywhere in the window (Telegram Desktop): the
    // typed `Files` payload (hydrolysis#275) fills the attach preview.
    let drop_hover = Binding::bool(false);
    let dh = drop_hover.clone();
    zstack((
        watch(store.screen.clone(), move |screen| match screen {
            Screen::Loading => loading_screen().anyview(),
            Screen::ApiKeys => api_keys_screen(inner.clone()).anyview(),
            Screen::Phone => phone_screen(inner.clone()).anyview(),
            Screen::Code => code_screen(inner.clone()).anyview(),
            Screen::Password => password_screen(inner.clone()).anyview(),
            Screen::Register => register_screen(inner.clone()).anyview(),
            Screen::Qr => qr_screen(inner.clone()).anyview(),
            Screen::Main => main_screen(inner.clone()).anyview(),
        })
        .drop_destination(|files: Files, store: Store| {
            let n = files.urls().len();
            store.attach.set(files.into_urls());
            store.notify(Str::from(format!(
                "{n} file{} ready — press Send",
                if n == 1 { "" } else { "s" }
            )))
        })
        .drop_hover(&drop_hover)
        // Window-level chords, bubbled from whatever is focused: Alt+↑/↓
        // switch between chats (Telegram Desktop). Handlers extract `Store`
        // from the env — `.state(&store)` on the zstack below reaches every
        // descendant node.
        .on_key_press(|Use(press): Use<KeyPress>, store: Store| {
            if !press.modifiers.contains(Modifiers::ALT) {
                return KeyHandling::Ignored;
            }
            match press.key {
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp) => {
                    if store.chat_switch(press.key == Key::Named(NamedKey::ArrowDown)) {
                        KeyHandling::Handled
                    } else {
                        KeyHandling::Ignored
                    }
                }
                _ => KeyHandling::Ignored,
            }
        }),
        when(dh, || {
            vstack((
                paperclip().tint(Accent).size(28.0, 28.0),
                text("Drop files to send").body(),
            ))
            .spacing(8.0)
            .padding_with(24.0)
            .background(RoundedRectangle::new(0.12).fill(Surface))
        }),
    ))
    .state(&store)
    // Transient toasts: store.notify() writes (seq, msg); the window's
    // SnackbarManager shows them (Option extractor — absent under the
    // semantic test runtime, which mounts no window).
    .on_change(
        &store.notice,
        |(_, msg): (u64, Str), sb: Option<SnackbarManager>| {
            if let Some(sb) = sb {
                sb.show(Snackbar::new(msg));
            }
        },
    )
    // Delete snackbar with Undo (Telegram Desktop). `undo_notice` bumps
    // only on a new batch, so a snackbar never re-fires on idle writes.
    .on_change(
        &store.undo_notice,
        |(_, n): (u64, i32), sb: Option<SnackbarManager>, store: Store| {
            if let Some(sb) = sb {
                sb.show(
                    Snackbar::new(format!(
                        "Deleted {n} message{}",
                        if n == 1 { "" } else { "s" }
                    ))
                    .action("Undo", move || store.undo_delete()),
                );
            }
        },
    )
    // In-app new-message banner (Desktop toast): a message landing in a
    // non-selected chat surfaces "Chat · Sender: preview" + Open — the
    // action selects the chat, matching Desktop's click-to-jump.
    .on_change(
        &store.msg_banner,
        |(_, chat_id, label): (u64, i64, Str), sb: Option<SnackbarManager>, store: Store| {
            if let Some(sb) = sb {
                sb.show(Snackbar::new(label).action("Open", move || store.select_chat(chat_id)));
            }
        },
    )
}

fn loading_screen() -> impl View {
    vstack((
        text("Watergram").title().foreground(Accent),
        text("Connecting…").body().muted(),
    ))
    .spacing(10.0)
}

pub(crate) fn auth_scaffold(children: impl View + 'static) -> impl View {
    vstack((text("Watergram").title().foreground(Accent), children))
        .spacing(14.0)
        .padding_with(32.0)
}

pub(crate) fn note_line(note: &Binding<Str>, busy: &Binding<bool>) -> impl View + use<> {
    vstack((
        text!("{note}", note = note.clone())
            .caption()
            .foreground(Error),
        when(busy.clone(), || text("Working…").caption().muted()),
    ))
    .spacing(4.0)
}

pub(crate) fn api_keys_screen(store: Store) -> impl View {
    let api_id = store.api_id.clone();
    let api_hash = store.api_hash.clone();
    let test_dc = store.test_dc.clone();
    auth_scaffold(vstack((
        text("Telegram API credentials")
            .headline()
            .foreground(Foreground),
        text("Create api_id/api_hash at my.telegram.org → API development tools. They are stored locally.")
            .caption()
            .muted(),
        field("API ID", &api_id).prompt("e.g. 94575").hide_label(),
        field("API hash", &api_hash)
            .prompt("api hash")
            .hide_label(),
        toggle(
            "Use Telegram test servers (recommended for development)",
            &test_dc,
        ),
        note_line(&store.auth_note, &store.busy),
        button("Continue").action(|store: Store| store.submit_api_keys()),
    )))
}

pub(crate) fn phone_screen(store: Store) -> impl View {
    let phone_b = store.phone.clone();
    auth_scaffold(vstack((
        text("Your phone number").headline().foreground(Foreground),
        text("Enter your phone number in international format, e.g. +9996612345 on the test servers.")
            .caption()
            .muted(),
        field("Phone number", &phone_b)
            .prompt("+9996612345")
            .hide_label(),
        note_line(&store.auth_note, &store.busy),
        button("Next").action(|store: Store| store.submit_phone()),
        button("Log in by QR code").action(|store: Store| store.request_qr()),
    )))
}

pub(crate) fn code_screen(store: Store) -> impl View {
    let phone = store.phone.clone();
    let code_b = store.code.clone();
    auth_scaffold(vstack((
        text!("Code sent to {phone}")
            .headline()
            .foreground(Foreground),
        text(
            "On the test servers the code equals the digits after 99966, e.g. 99966XXXXX → XXXXX.",
        )
        .caption()
        .muted(),
        field("Code", &code_b).prompt("12345").hide_label(),
        note_line(&store.auth_note, &store.busy),
        button("Next").action(|store: Store| store.submit_code()),
        button("Back").action(|store: Store| store.screen.set(Screen::Phone)),
    )))
}

pub(crate) fn password_screen(store: Store) -> impl View {
    let hint = store.password_hint.clone();
    let password = store.password.clone();
    auth_scaffold(vstack((
        text("Two-step verification")
            .headline()
            .foreground(Foreground),
        text!("{hint}").caption().muted(),
        SecureField::new("Password", &password).hide_label(),
        note_line(&store.auth_note, &store.busy),
        button("Sign in").action(|store: Store| store.submit_password()),
    )))
}

pub(crate) fn register_screen(store: Store) -> impl View {
    let first = store.first_name.clone();
    let last = store.last_name.clone();
    auth_scaffold(vstack((
        text("Your name").headline().foreground(Foreground),
        field("First name", &first)
            .prompt("First name")
            .hide_label(),
        field("Last name", &last)
            .prompt("Last name (optional)")
            .hide_label(),
        note_line(&store.auth_note, &store.busy),
        button("Sign up").action(|store: Store| store.register()),
    )))
}

pub(crate) fn qr_screen(store: Store) -> impl View {
    let link = store.qr_link.clone();
    auth_scaffold(vstack((
        text("Log in by QR code").headline().foreground(Foreground),
        text("On your phone: Settings → Devices → Link Desktop Device, then scan:")
            .caption()
            .muted(),
        Barcode::qr(link).size(220.0, 220.0),
        note_line(&store.auth_note, &store.busy),
        button("Log in by phone instead").action(|store: Store| store.screen.set(Screen::Phone)),
    )))
}

// ---------------------------------------------------------------------------
// Main screen
// ---------------------------------------------------------------------------

pub(crate) fn main_screen(store: Store) -> impl View {
    let sb = store.clone();
    let sel = store.selected.clone();
    NavigationSplitView::new(
        &sel,
        move || sidebar_stack(sb.clone()),
        move |chat_id: i64| chat_detail(store.clone(), chat_id),
    )
    .sidebar_width(ColumnWidth::new(260.0, 340.0, 420.0))
    .placeholder(|| {
        // A content-sized stack is placed at its own answer (§7) — top of
        // the detail rect. The spacers make it stretch on the main axis so
        // it fills the pane and centres the empty state like Desktop does.
        vstack((
            spacer(),
            text("Watergram").title().foreground(Foreground),
            text("Select a chat to start messaging").body().muted(),
            spacer(),
        ))
        .spacing(6.0)
    })
}

pub(crate) fn sidebar_stack(store: Store) -> impl View {
    let nav = store.nav.clone();
    let inner = store.clone();
    let search = store.search.clone();
    NavigationStack::with_path(
        nav,
        sidebar_view(store.clone())
            .title("Watergram")
            .searchable(&search, "Search chats")
            .navigation_toolbar(
                // Telegram Desktop: the hamburger sits at the leading edge
                // of the title/search row; New chat lives inside the menu.
                NavigationToolbar::default().item(NavigationToolbarItem::new(
                    NavigationToolbarPlacement::TopBarLeading,
                    Menu::new(
                        label("Menu").icon(menu()).icon_only(),
                        (
                            {
                                let s = store.clone();
                                store
                                    .tr("NewChat", 0, "New chat")
                                    .action(move || s.nav.push(Route::NewChat))
                            },
                            {
                                let s = store.clone();
                                store
                                    .tr("ArchivedChats", 0, "Archive")
                                    .action(move || s.toggle_archive_view())
                            },
                            {
                                let s = store.clone();
                                store
                                    .tr("Accounts", 0, "Accounts")
                                    .action(move || s.toggle_accounts())
                            },
                            {
                                let s = store.clone();
                                store
                                    .tr("Settings", 0, "Settings")
                                    .action(move || s.nav.push(Route::Settings))
                            },
                            {
                                // Telegram Desktop's hamburger carries the dark
                                // toggle directly (its row shows a checkmark —
                                // `Command::selected`).
                                store
                                    .tr("NightMode", 0, "Night mode")
                                    .action(|s: Store| s.dark.set(!s.dark.snapshot()))
                                    .selected(store.dark.clone())
                            },
                            {
                                // Desktop's window menu keeps Quit as a pointer
                                // affordance; the Ctrl+W chord itself lives on
                                // `App::menu_bar` (lib.rs) since hydrolysis
                                // devin/menu-bar 66501fc — one chord, one source,
                                // no duplicate registration.
                                store
                                    .tr("Quit", 0, "Quit Telegram")
                                    .action(move |store: Store| {
                                        store.win_state.set(WindowState::Closed)
                                    })
                            },
                        ),
                    ),
                )),
            ),
    )
    .destination(move |route| match route {
        Route::Settings => settings_view(inner.clone()),
        Route::NewChat => new_chat_view(inner.clone()),
        Route::Profile => profile_view(inner.clone()),
    })
}

#[allow(if_else_view)]
// tab styling picks between two text looks, not reactive content
// MsgHit/ChatRow fields are immutable row data — hits are re-listed on
// each new search, not mutated in place.
#[expect(
    collection_item_snapshot,
    reason = "MsgHit/ChatRow fields are immutable row data; a hit updates by list replacement, not in-place mutation"
)]
pub(crate) fn sidebar_view(store: Store) -> impl View {
    let conn = store.connection.clone();
    // Single-message forward (context menu) sets `forward_message`; the
    // multi-select bar sets `forward_ids`. The banner shows for either.
    let forward_mode = store
        .forward_message
        .is_some()
        .or(&store.forward_ids.map(|ids| !ids.is_empty()))
        .distinct();
    let filtered_rows = store.chats.zip(&store.search).map(|(mut rows, q)| {
        let q = q.to_lowercase();
        if q.is_empty() {
            for r in &mut rows {
                r.title_styled = StyledStr::empty();
                r.preview_styled = StyledStr::empty();
            }
            rows
        } else {
            // r35: Desktop highlights the matched substring in the
            // title/preview (span `background` — hydrolysis#212).
            rows.into_iter()
                .filter(|r| {
                    r.title.to_lowercase().contains(&q) || r.preview.to_lowercase().contains(&q)
                })
                .map(|mut r| {
                    // r36: tertiary-container box + light selection
                    // foreground (secondary_container blends into the
                    // row fill; the tertiary hue stays visible).
                    let (bg, fg) = (
                        Color::from(TertiaryContainer),
                        Color::from(SelectionForeground),
                    );
                    r.title_styled = highlight_styled(
                        &StyledStr::plain(r.title.clone()),
                        &q,
                        bg.clone(),
                        fg.clone(),
                    );
                    r.preview_styled =
                        highlight_styled(&StyledStr::plain(r.preview.clone()), &q, bg, fg);
                    r
                })
                .collect()
        }
    });
    let filtered: SignalCollection<_> = SignalCollection::new(filtered_rows.clone());
    // Desktop's search-scope tabs under the field: All / Chats / Media /
    // Files / Links — visible whenever a query is active, even when a tab
    // yields no hits (the strip must stay so the user can switch back).
    let searching = store.search.map(|q: Str| !q.is_empty()).distinct();
    // The whole results pane is ONE `SignalCollection` of `SearchRow`s —
    // sections/labels/empty-state are data, so a tab switch is a
    // collection update (no nested `when`s inside the `when` payload).
    let search_rows = filtered_rows
        .zip(&store.server_results)
        .zip(&store.msg_results)
        .zip(&store.search_filter)
        .map(|(((locals, server), hits), f)| {
            let mut out: Vec<SearchRow> = Vec::new();
            if f <= 1 {
                out.extend(locals.into_iter().map(SearchRow::Chat));
                if !server.is_empty() {
                    out.push(SearchRow::Header("Global search results".into()));
                    out.extend(server.into_iter().map(SearchRow::Chat));
                }
            }
            if f != 1 && !hits.is_empty() {
                out.push(SearchRow::Header("Messages".into()));
                out.extend(hits.into_iter().map(SearchRow::Hit));
            }
            if !out.iter().any(|r| !matches!(r, SearchRow::Header(_))) {
                out.push(SearchRow::Empty);
            }
            out
        });
    let search_now = store.search.debounce(Duration::from_millis(400));
    let search_tabs = store.search_filter.map(|f| {
        [
            ("All", 0),
            ("Chats", 1),
            ("Media", 2),
            ("Files", 3),
            ("Links", 4),
        ]
        .iter()
        .map(|(t, i)| FolderRow {
            id: *i,
            title: (*t).into(),
            active: f == *i,
            unread: 0,
            include: Vec::new(),
        })
        .collect::<Vec<_>>()
    });
    let rows_store = store.clone();
    // Folder chips carry each list's unread-chat count (TDLib
    // `updateUnreadChatCount`) like Desktop's folder bar.
    let folder_tabs = store
        .folders
        .zip(&store.active_folder)
        .zip(&store.folder_unreads)
        .map(|((fs, active), unreads)| {
            let badge = |id: i32| unreads.get(&id).copied().unwrap_or(0);
            let mut tabs: Vec<FolderRow> = vec![FolderRow {
                id: 0,
                title: "All".into(),
                active: active == 0,
                unread: badge(0),
                include: Vec::new(),
            }];
            tabs.extend(fs.iter().map(|f| FolderRow {
                id: f.id,
                title: f.title.clone(),
                active: f.id == active,
                unread: badge(f.id),
                include: f.include.clone(),
            }));
            tabs.push(FolderRow {
                id: -1,
                title: "Archive".into(),
                active: active == -1,
                unread: badge(-1),
                include: Vec::new(),
            });
            tabs
        });
    let has_folders = store.folders.map(|f| !f.is_empty()).distinct();

    vstack((
        // Connection state: Desktop only surfaces it while reconnecting —
        // the binding carries a label only for non-Ready states.
        when(conn.map(|c: Str| !c.as_str().is_empty()), move || {
            text!("{conn}").caption().muted().padding_with((6.0, 12.0))
        }),
        // Collapsed `when` children would each still eat a vstack spacing
        // slot; keep the optional sections in one zero-spacing stack so a
        // hidden panel costs no gap.
        vstack((
            when(store.accounts_open.clone(), move || {
                let rows = store.accounts.clone();
                vstack((
                    scroll(VStack::for_each(
                        SignalCollection::new(rows.clone()),
                        move |acc: AccountRow| {
                            let id = acc.id;
                            hstack((text(acc.label.clone()).caption(), spacer()))
                                .padding_with((4.0, 12.0))
                                .on_tap(move |store: Store| store.switch_account(id))
                        },
                    )),
                    hstack((
                        plus().tint(Accent).size(14.0, 14.0),
                        text("Add account").caption().foreground(Accent),
                        spacer(),
                    ))
                    .padding_with((4.0, 12.0))
                    .on_tap(|store: Store| store.add_account()),
                ))
                .background(Surface)
            }),
            when(forward_mode, move || {
                let noattr = store.forward_noattr.clone();
                let comment_b = store.forward_comment.clone();
                // Two rows: the comment field + no-attribution chip alone
                // overfill the 340pt sidebar, wrapping the caption into a
                // narrow shard.
                vstack((
                    hstack((
                        share_variant().tint(Accent).size(16.0, 16.0),
                        text("Select a chat to forward to")
                            .caption()
                            .foreground(Accent),
                        spacer(),
                        icon_button(close(), "Cancel", |store: Store| {
                            store.forward_message.set(None);
                            store.forward_ids.set(Vec::new());
                            store.forward_comment.set_from("");
                        }),
                    )),
                    hstack((
                        field("Comment", &comment_b)
                            .prompt("Add a comment…")
                            .hide_label(),
                        text!(
                            "Without attribution{mark}",
                            mark = noattr.map(|b: bool| Str::from(if b { " ✓" } else { "" }))
                        )
                        .caption()
                        .padding_with((2.0, 8.0))
                        .background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
                        .on_tap(|store: Store| {
                            let v = !store.forward_noattr.snapshot();
                            store.forward_noattr.set(v);
                        })
                        .a11y_label("Forward without attribution")
                        .a11y_role(AccessibilityRole::Button),
                    )),
                ))
                .padding_with((8.0, 12.0))
                .background(Surface)
            }),
            when(has_folders, move || {
                let folder_edit = store.folder_open.clone();
                // Bound here (outer body) so the `when(folder_edit)` builder
                // only consumes locals — field-path captures of `store` would
                // partially move it and demote this closure to FnOnce.
                let picked = store.folder_chats.clone();
                let chat_rows = SignalCollection::new(store.chats.clone());
                // Chip metrics: caption line ~17dp + 2×3dp vertical chip padding.
                // ScrollView reports StretchAxis::Both unconditionally
                // (raw_view!, axis not consulted — water-rs/waterui#1208), and View has no
                // propose-None/fixed-size-axis modifier, so a literal content
                // height is the only way to stop the horizontal scroller taking
                // the sidebar surplus. Breaks under font scaling — filed as a gap.
                let chip_pad_v = 3.0_f32;
                let chip_row_h = 17.0 + 2.0 * chip_pad_v;
                vstack((
                    hstack((
                        scroll_horizontal(HStack::for_each(
                            SignalCollection::new(folder_tabs.clone()),
                            move |tab: FolderRow| {
                                let label = tab.title.clone();
                                let a11y = label.clone();
                                let id = tab.id;
                                // Desktop's folder chips append the list's
                                // unread-chat count after the title.
                                let fg = if tab.active {
                                    Color::from(Accent)
                                } else {
                                    Color::from(MutedForeground)
                                };
                                let chip = if tab.unread > 0 {
                                    hstack((
                                        text(label).caption().foreground(fg.clone()),
                                        text(tab.unread.to_string())
                                            .caption()
                                            .bold()
                                            .foreground(fg.clone()),
                                    ))
                                    .spacing(4.0)
                                    .anyview()
                                } else if tab.active {
                                    text(label).caption().bold().foreground(fg).anyview()
                                } else {
                                    text(label).caption().foreground(fg).anyview()
                                };
                                let chip = chip
                                    .padding_with((chip_pad_v, 10.0_f32))
                                    .background(if tab.active {
                                        RoundedRectangle::new(0.5).fill(SurfaceVariant).anyview()
                                    } else {
                                        AnyView::default()
                                    })
                                    .on_tap(move |store: Store| store.set_list(id))
                                    .a11y_label(a11y)
                                    .a11y_role(AccessibilityRole::Button);
                                if id > 0 {
                                    chip.context_menu((
                                        "Edit folder".action(move |store: Store| {
                                            store.open_folder_editor(id)
                                        }),
                                        // Desktop folder menu: "Mark all as read".
                                        "Mark all as read"
                                            .action(move |store: Store| store.mark_folder_read(id)),
                                        "Delete folder"
                                            .action(move |store: Store| store.delete_folder(id)),
                                    ))
                                    .anyview()
                                } else {
                                    chip.anyview()
                                }
                            },
                        )),
                        // Same chip metrics as the tab chips so the row stays
                        // at chip height. A Plain `button` with an icon-only
                        // label reserves the M3 icon-button 48dp touch
                        // target as its minimum box (hydrolysis-m3
                        // icon_button::metrics, propagated by hydrolysis'
                        // measure_button_node → button_chrome_size) plus
                        // label padding — ~74pt claimed for a 14pt icon,
                        // which squeezed the chip scroll to ~240pt in the
                        // 340pt sidebar (r51 D10). A tap chip like the
                        // folder tabs claims only the ~30pt it draws.
                        folder_plus()
                            .foreground(Accent)
                            .size(14.0, 14.0)
                            .a11y_hidden(true)
                            .padding_with((chip_pad_v, 8.0_f32))
                            .on_tap(|store: Store| store.open_folder_editor(0))
                            .a11y_role(AccessibilityRole::Button)
                            .a11y_label("New folder"),
                    ))
                    .height(chip_row_h)
                    .padding_with((4.0, 8.0)),
                    {
                        let fname = store.folder_name.clone();
                        let fc = store.folder_contacts.clone();
                        let fg = store.folder_groups.clone();
                        let fch = store.folder_channels.clone();
                        when(folder_edit, move || {
                            vstack((
                                field("Folder name", &fname).hide_label(),
                                toggle("Contacts", &fc),
                                toggle("Groups", &fg),
                                toggle("Channels", &fch),
                                // Desktop's folder editor lists the chats the folder
                                // includes, with a check each (`included_chat_ids`).
                                text("Included chats").caption().muted(),
                                scroll(VStack::for_each(chat_rows.clone(), {
                                    let picked = picked.clone();
                                    move |row: ChatRow| {
                                        let id = row.id;
                                        let on = picked.map(move |ids: Vec<i64>| {
                                            AccessibilityState::new()
                                                .checked(Some(ids.contains(&id)))
                                        });
                                        let mark = picked.map(move |ids: Vec<i64>| {
                                            Str::from(if ids.contains(&id) {
                                                "☑ "
                                            } else {
                                                "☐ "
                                            })
                                        });
                                        hstack((
                                            text!("{mark}").body(),
                                            text(row.title.clone()).body().line_limit(ONE),
                                            spacer(),
                                        ))
                                        .spacing(4.0)
                                        .padding_with((3.0, 6.0))
                                        .on_tap(move |store: Store| store.toggle_folder_chat(id))
                                        .a11y_label(row.title.clone())
                                        .a11y_state_signal(on)
                                        .a11y_role(AccessibilityRole::Button)
                                    }
                                }))
                                .max_height(140.0),
                                hstack((
                                    spacer(),
                                    button("Save folder")
                                        .action(|store: Store| store.save_folder()),
                                )),
                            ))
                            .spacing(6.0)
                            .padding_with((4.0, 10.0))
                        })
                    },
                ))
            }),
        ))
        .spacing(0.0),
        // Search-scope tabs (All / Chats / Media / Files / Links) under the
        // field while a query is active — Desktop parity.
        when(searching.clone(), move || {
            let chip_pad_v: f32 = 3.0;
            scroll_horizontal(HStack::for_each(
                SignalCollection::new(search_tabs.clone()),
                move |row: FolderRow| {
                    let id = row.id;
                    // Same chip styling as the folder chips above.
                    let fg = if row.active {
                        Color::from(Accent)
                    } else {
                        Color::from(MutedForeground)
                    };
                    let chip = if row.active {
                        text(row.title.clone())
                            .caption()
                            .bold()
                            .foreground(fg)
                            .anyview()
                    } else {
                        text(row.title.clone()).caption().foreground(fg).anyview()
                    };
                    chip.padding_with((chip_pad_v, 10.0_f32))
                        .background(if row.active {
                            RoundedRectangle::new(0.5).fill(SurfaceVariant).anyview()
                        } else {
                            AnyView::default()
                        })
                        .on_tap(move |store: Store| {
                            store.search_filter.set(id);
                            store.run_search(store.search.snapshot());
                        })
                        .a11y_label(row.title.clone())
                        .a11y_role(AccessibilityRole::Button)
                },
            ))
            .padding_with((4.0, 8.0))
        }),
        // `List` reports StretchAxis::Both and fills the leftover region;
        // a `Lazy` stack inside `scroll(vstack)` reports None and is sized to
        // its realized rows, which clipped the list mid-pane.
        zstack((
            {
                let filtered_else = filtered.clone();
                let list_sel = store.list_selection.clone();
                let rows_else = rows_store.clone();
                // r41-1's `when` workaround reverted to `.visible()` on
                // hydrolysis 437ef045 (#274): both List panes stay mounted and
                // visibility gates which paints — toggling `.visible(false)` on
                // a mounted subtree containing a `List` used to panic the a11y
                // flush (accessibility_impl.rs:567).
                let row_store = rows_else.clone();
                zstack((
                    // One List over `SearchRow`s — the section composition is
                    // pure data (see the `search_rows` binding above).
                    List::for_each(
                        SignalCollection::new(search_rows.clone()),
                        move |row: SearchRow| match row {
                            SearchRow::Header(t) => {
                                ListItem::new(text(t).caption().muted().padding_with((4.0, 12.0)))
                            }
                            SearchRow::Empty => ListItem::new(
                                text("No results").body().muted().padding_with((8.0, 12.0)),
                            ),
                            SearchRow::Chat(r) => {
                                let id = r.id;
                                ListItem::new(
                                    chat_row(row_store.clone(), r)
                                        .on_tap(move |store: Store| store.select_chat(id)),
                                )
                            }
                            SearchRow::Hit(hit) => {
                                let hit2 = hit.clone();
                                // Leading kind glyph — Desktop shows a
                                // media/file thumbnail on media, file and
                                // link hits.
                                let kind_icon = match hit.kind {
                                    1 => image().tint(MutedForeground).size(16.0, 16.0).anyview(),
                                    2 => file_document_outline()
                                        .tint(MutedForeground)
                                        .size(16.0, 16.0)
                                        .anyview(),
                                    3 => link_variant()
                                        .tint(MutedForeground)
                                        .size(16.0, 16.0)
                                        .anyview(),
                                    _ => AnyView::default(),
                                };
                                ListItem::new(
                                    hstack((
                                        kind_icon,
                                        vstack((
                                            hstack((
                                                text(hit.title.clone()).body().line_limit(ONE),
                                                spacer(),
                                                text(hit.time.clone()).caption().muted(),
                                            ))
                                            .spacing(4.0),
                                            text(if hit.sender.is_empty() {
                                                hit.snippet.clone()
                                            } else {
                                                format!("{}: {}", hit.sender, hit.snippet).into()
                                            })
                                            .caption()
                                            .line_limit(ONE)
                                            .muted(),
                                        ))
                                        .spacing(2.0)
                                        .leading(),
                                    ))
                                    .spacing(6.0)
                                    .padding_with((4.0, 10.0))
                                    .on_tap(move |store: Store| store.open_hit(&hit2))
                                    .a11y_label(format!(
                                        "Message in {}: {}",
                                        hit.title, hit.snippet
                                    )),
                                )
                            }
                        },
                    )
                    .visible(searching.clone()),
                    {
                        let rows_l = rows_else.clone();
                        // Framework-owned selection (waterui#1233): pointer taps
                        // and Up/Down write `list_selection`; `on_change` below
                        // routes it through `select_chat`.
                        List::for_each(filtered_else.clone(), move |row: ChatRow| {
                            ListItem::new(chat_row(rows_l.clone(), row))
                        })
                        .selection(&list_sel)
                        .visible(searching.not())
                    },
                ))
            },
            // Desktop's bottom-right FAB: the pencil opens the New chat
            // picker from anywhere in the chat list.
            icon_button(
                pencil().tint(AccentForeground),
                "New chat",
                |store: Store| store.nav.push(Route::NewChat),
            )
            .padding_with((12.0, 12.0))
            .background(Circle.fill(Accent))
            .padding_with((16.0, 16.0)),
        ))
        .alignment(BottomTrailing),
    ))
    // ArrowUp/Down and Enter bubble up from whatever is focused in the
    // sidebar (search field, a row's inner press slot — hydrolysis#220)
    // to drive the list selection like Desktop.
    .on_key_press(|Use(press): Use<KeyPress>, store: Store| match press.key {
        Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp) => {
            if store.list_nav(press.key == Key::Named(NamedKey::ArrowDown)) {
                KeyHandling::Handled
            } else {
                KeyHandling::Ignored
            }
        }
        Key::Named(NamedKey::Enter) => {
            if store.open_top_hit() {
                KeyHandling::Handled
            } else {
                KeyHandling::Ignored
            }
        }
        _ => KeyHandling::Ignored,
    })
    .on_change(&search_now, |q: Str, store: Store| store.run_search(q))
    .on_change(&store.list_selection, |v: Option<i64>, store: Store| {
        if let Some(id) = v {
            store.select_chat(id)
        }
    })
}

// M3 icon button: an icon-only `Label` keeps `name` as the semantic identity
// (a11y) while rendering only the icon. Until water-rs/hydrolysis#115 gives
// icon-only buttons the 40dp icon-button box they keep the generic ~58-72dp
// minimum.
pub(crate) fn icon_button<F>(
    icon: impl View + Clone + 'static,
    name: &'static str,
    on_tap: F,
) -> impl View
where
    F: Fn(Store) + 'static,
{
    button(label(name).icon(icon).icon_only())
        .style(ButtonStyle::Plain)
        .action(move |store: Store| on_tap(store))
}

/// Chat-list kind glyph: Telegram Desktop shows none for private chats,
/// groups, or channels (the avatar carries the type) — only a lock marks a
/// secret chat.
pub(crate) fn kind_icon(kind: &str) -> impl View {
    when(kind == "secret", || {
        lock().tint(MutedForeground).size(12.0, 12.0)
    })
    .otherwise(|| ())
    .a11y_hidden(true)
}

pub(crate) fn initials(name: &str) -> Str {
    let mut it = name.split_whitespace().filter_map(|w| w.chars().next());
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a}{b}").to_uppercase().into(),
        (Some(a), None) => format!("{a}").to_uppercase().into(),
        _ => "·".into(),
    }
}

/// Telegram Desktop assigns every peer one of seven accent colors — the
/// colored userpics and the group sender names. A peer's slot comes from
/// TDLib's `accent_color_id` (`id % 7` over the userpic palette); when the
/// peer carries none (demo rows, unresolved senders) the display name is
/// hashed instead, so a peer keeps one color everywhere it is drawn.
pub(crate) fn peer_color(accent: i32, name: &str) -> Color {
    const PALETTE: [&str; 7] = [
        "#CC5049", // red
        "#D67722", // orange
        "#955CDB", // violet
        "#40A920", // green
        "#309EBA", // cyan
        "#368AD1", // blue
        "#C7508B", // pink
    ];
    let slot = if accent >= 0 {
        accent as usize % 7
    } else {
        (name
            .bytes()
            .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64))
            % 7) as usize
    };
    Srgb::from_hex(PALETTE[slot]).into()
}

pub(crate) fn avatar(store: Store, file_id: i32, title: &str, size: f32, accent: i32) -> impl View {
    let path_a = store.file_signal(file_id);
    let path_b = store.file_signal(file_id);
    let has = path_a.map(|p: Str| !p.is_empty()).distinct();
    let url = path_b.map(Url::from_file_path_str);
    let label = initials(title);
    let fill = peer_color(accent, title);
    // Decorative: the avatar sits beside the chat/member name that already
    // labels the row — hide the shape/photo leaves from the a11y tree.
    when(has, move || {
        Photo::new(url.clone()).size(size, size).clip(Circle)
    })
    .otherwise(move || {
        text(label.clone())
            .caption()
            .bold()
            .foreground(AccentForeground)
            .size(size, size)
            .background(Circle.fill(fill.clone()))
    })
    .a11y_hidden(true)
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn chat_row(store: Store, row: ChatRow) -> impl View {
    let unread = row.unread;
    // Telegram Desktop: a saved draft replaces the last-message preview with
    // a red "Draft: <text>" line; a live typing indicator still wins.
    let preview_line: AnyView = match (row.typing, row.draft.is_empty()) {
        (true, _) => text("typing…").caption().line_limit(ONE).muted().anyview(),
        (false, false) => hstack((
            text("Draft: ").caption().foreground(Error),
            text(row.draft.clone()).caption().line_limit(ONE).muted(),
        ))
        .spacing(0.0)
        .anyview(),
        _ => {
            if row.preview_styled.is_empty() {
                text(row.preview.clone())
                    .caption()
                    .line_limit(ONE)
                    .muted()
                    .anyview()
            } else {
                text(row.preview_styled.clone())
                    .caption()
                    .line_limit(ONE)
                    .muted()
                    .anyview()
            }
        }
    };
    // Unread-reaction "❤" badge: same M3 badge pill, left of the "@"
    // badge (Telegram Desktop shows both for unread mentions/reactions).
    let reaction_badge: AnyView = if row.unread_reactions > 0 {
        heart()
            .tint(AccentForeground)
            .size(10.0, 10.0)
            .padding_with((4.0, 4.0))
            .background(RoundedRectangle::new(0.5).fill(Accent))
            .a11y_label(format!("{} unread reactions", row.unread_reactions))
            .anyview()
    } else {
        spacer().width(0.0).anyview()
    };
    // Unread-mention "@" badge: same M3 badge pill as the unread count,
    // sitting to its left (Telegram Desktop's row layout).
    let mention_badge: AnyView = if row.unread_mentions > 0 {
        text("@")
            .caption()
            .bold()
            .foreground(AccentForeground)
            .padding_with((1.0, 4.0))
            .background(RoundedRectangle::new(0.5).fill(Accent))
            .a11y_label(format!("{} unread mentions", row.unread_mentions))
            .anyview()
    } else {
        spacer().width(0.0).anyview()
    };
    let badge: AnyView = if row.pinned {
        pin()
            .tint(MutedForeground)
            .size(14.0, 14.0)
            .a11y_hidden(true)
            .anyview()
    } else if unread > 0 || row.marked_unread {
        if row.marked_unread && unread == 0 {
            text("●").caption().foreground(Accent).anyview()
        } else {
            text!("{unread}")
                .caption()
                .bold()
                .foreground(if row.muted {
                    Color::from(MutedForeground)
                } else {
                    Color::from(AccentForeground)
                })
                // M3 large badge: 16dp-tall pill growing horizontally,
                // 4dp horizontal padding — not a circle.
                .padding_with((1.0, 4.0))
                .background(if row.muted {
                    RoundedRectangle::new(0.5).fill(SurfaceVariant)
                } else {
                    RoundedRectangle::new(0.5).fill(Accent)
                })
                .anyview()
        }
    } else {
        spacer().width(1.0).anyview()
    };

    // Hover ⋮ at the row's top-trailing edge (Desktop reveals it over the
    // timestamp); same action list as the right-click context menu.
    let hov = Binding::bool(false);
    let hov_show = hov.clone();
    let hov_off = hov.not();
    let row_for_menu = row.clone();
    zstack((
        hstack((
            avatar(store, row.photo_file, &row.title, 44.0, row.accent),
            vstack((
                hstack((
                    kind_icon(&row.kind_icon),
                    // Muted chats dim the title and carry a bell-off glyph
                    // after it — Telegram Desktop's muted-row style.
                    if row.title_styled.is_empty() {
                        text(row.title.clone())
                            .body()
                            .line_limit(ONE)
                            .foreground(if row.muted {
                                Color::from(MutedForeground)
                            } else {
                                Color::from(Foreground)
                            })
                    } else {
                        text(row.title_styled.clone())
                            .body()
                            .line_limit(ONE)
                            .foreground(if row.muted {
                                Color::from(MutedForeground)
                            } else {
                                Color::from(Foreground)
                            })
                    },
                    // Verified / scam badge right after the title —
                    // Telegram Desktop's blue check / red warning marks.
                    match row.badge {
                        Verification::Verified => check_decagram()
                            .tint(Accent)
                            .size(14.0, 14.0)
                            .a11y_label("Verified")
                            .anyview(),
                        Verification::Scam => alert_decagram()
                            .tint(Error)
                            .size(14.0, 14.0)
                            .a11y_label("Scam")
                            .anyview(),
                        Verification::None => spacer().width(0.0).anyview(),
                    },
                    if row.muted {
                        bell_off()
                            .tint(MutedForeground)
                            .size(13.0, 13.0)
                            .a11y_hidden(true)
                            .anyview()
                    } else {
                        spacer().width(0.0).anyview()
                    },
                    if row.auto_delete > 0 {
                        clock_outline()
                            .tint(MutedForeground)
                            .size(13.0, 13.0)
                            .a11y_label(format!(
                                "Auto-delete: {}",
                                auto_delete_label(row.auto_delete)
                            ))
                            .anyview()
                    } else {
                        spacer().width(0.0).anyview()
                    },
                    spacer(),
                    when(row.online, || text("●").caption().foreground(Accent)),
                    // Desktop hides the timestamp while the hover ⋮ covers it.
                    {
                        let t = row.time.clone();
                        when(hov_off, move || {
                            let t = t.clone();
                            text(t).caption().muted()
                        })
                    },
                ))
                .spacing(4.0),
                hstack((preview_line, spacer(), reaction_badge, mention_badge, badge)).spacing(4.0),
            ))
            .spacing(2.0)
            .leading(),
        ))
        .spacing(10.0)
        .padding_with((6.0, 10.0)),
        when(hov_show, move || {
            Menu::new(
                label("Chat actions").icon(dots_vertical()).icon_only(),
                chat_row_menu(&row_for_menu),
            )
        }),
    ))
    .alignment(TopTrailing)
    .on_hover_enter(|State(h): State<Binding<bool>>| h.set(true))
    .on_hover_exit(|State(h): State<Binding<bool>>| h.set(false))
    .state(&hov)
    .context_menu(chat_row_menu(&row))
}

/// Chat-row action menu — shared between the right-click context menu
/// and the hover ⋮ button (Desktop shows the same actions on both).
fn chat_row_menu(row: &ChatRow) -> impl MenuView {
    let id = row.id;
    (
        // Ctrl+R — Desktop's mark-read accelerator; the hint registers
        // while the row's ⋮ menu is mounted (hover) and the command
        // itself runs from the context menu too.
        "Mark read"
            .action(move |store: Store| store.mark_read(id))
            .shortcut(Shortcut::new('r').control()),
        if row.marked_unread {
            "Mark as read (clear flag)"
        } else {
            "Mark as unread"
        }
        .action(move |store: Store| store.toggle_mark_unread(id)),
        if row.pinned { "Unpin" } else { "Pin" }.action(move |store: Store| store.toggle_pin(id)),
        if row.in_archive {
            "Unarchive"
        } else {
            "Archive"
        }
        .action(move |store: Store| store.toggle_archive(id)),
        row.muted
            .then(|| "Unmute".action(move |store: Store| store.toggle_mute(id))),
        // Telegram Desktop: "Mute for…" submenu with fixed durations.
        Menu::new(
            "Mute for…",
            (
                "1 hour".action(move |store: Store| store.set_mute(id, 3_600)),
                "8 hours".action(move |store: Store| store.set_mute(id, 28_800)),
                "2 days".action(move |store: Store| store.set_mute(id, 172_800)),
                "Forever".action(move |store: Store| store.set_mute(id, i32::MAX)),
            ),
        ),
        // Auto-delete timer submenu (Desktop's "Auto-delete messages").
        Menu::new(
            "Auto-delete",
            (
                "Off".action(move |store: Store| store.set_auto_delete(id, 0)),
                "1 day".action(move |store: Store| store.set_auto_delete(id, 86_400)),
                "1 week".action(move |store: Store| store.set_auto_delete(id, 604_800)),
                "1 month".action(move |store: Store| store.set_auto_delete(id, 2_592_000)),
            ),
        ),
        "Join chat".action(move |store: Store| store.join(id)),
        // Desktop's "Report" on the chat-row menu (spam flag — same call
        // the chat action bar's Report button makes).
        "Report".action(move |store: Store| store.report_chat(id)),
        "Clear history".action(move |store: Store| store.clear_history(id)),
        "Leave chat".action(move |store: Store| store.leave(id)),
    )
}

// ---------------------------------------------------------------------------
// Chat detail
// ---------------------------------------------------------------------------

pub(crate) fn chat_column(store: Store) -> impl View {
    let composer_b = store.composer.clone();
    let has_more = store.no_more_history.not();
    let reply_open = store.reply_to.is_some();
    let edit_open = store.editing.is_some();
    let reply_label = store.reply_label.clone();
    let messages = SignalCollection::new(store.messages.clone());
    let has_attach = store.attach.map(|v| !v.is_empty()).distinct();
    let voice_rec = store.recording_voice.clone();
    let has_capture_error = store.capture_error.map(|s: Str| !s.is_empty()).distinct();
    let capture_error_text = store.capture_error.clone();
    let composer_empty = store.composer.str_is_empty().distinct();
    let store_bar = store.clone();
    let bg_color = store.chat_bg_color();
    // The open chat's `chatActionBar*` hides the composer row entirely.
    let bar_present = store
        .chats
        .zip(&store.selected)
        .map(|(rows, sel)| {
            sel.and_then(|id| {
                rows.iter()
                    .find(|r| r.id == id)
                    .map(|r| !r.action_bar.is_empty())
            })
            .unwrap_or(false)
        })
        .distinct();
    let video_note_open = store.video_note_open.clone();
    let voice_elapsed = store.voice_elapsed.clone();
    let store_for_sheet = store.clone();
    let scroller = store.scroll.clone();
    let has_pinned = store.pinned_label.map(|s: Str| !s.is_empty()).distinct();
    let pinned_label = store.pinned_label.clone();
    // Banner counter "·2" when several messages are pinned (Desktop shows
    // a progress tick on the pin icon; the count reads the same info).
    let pinned_count = store
        .pinned_msgs
        .map(|v: Vec<PinnedRow>| {
            if v.len() > 1 {
                Str::from(format!(" · {}", v.len()))
            } else {
                Str::from("")
            }
        })
        .distinct();
    let pinned_rows = SignalCollection::new(store.pinned_msgs.clone());
    let search_open = store.chat_search_open.clone();
    let search_b = store.chat_search.clone();
    let search_results_b = store.chat_search_results.clone();
    // Desktop's "n/N" match counter beside the in-chat search field.
    let match_label = store
        .chat_search_pos
        .zip(&store.chat_match_ids)
        .map(|(p, ids)| {
            if ids.is_empty() {
                Str::from("")
            } else {
                Str::from(format!("{}/{}", p + 1, ids.len()))
            }
        });
    let stickers_open = store.stickers_open.clone();
    let sticker_items = store.sticker_items.clone();
    let store_cells = store.clone();
    let inner = store.clone();
    let sel_active = store
        .selected_msgs
        .map(|v: Vec<i64>| !v.is_empty())
        .distinct();
    let sel_active_rows = sel_active.clone();
    let sel_msgs_rows = store.selected_msgs.clone();
    let sel_count = store.selected_msgs.map(|v: Vec<i64>| v.len()).distinct();
    let scheduled_open = store.scheduled_open.clone();
    let scheduled_b = store.scheduled.clone();
    let emoji_tab = store.panel_tab.is_zero().distinct();
    // @mention completion: trailing @token in the composer filters the
    // loaded member list; the popup sits above the composer row.
    let mention_sig = store.composer.zip(&store.members).map(|(q, ms)| {
        let tok = Store::mention_token(&q);
        match tok {
            None => Vec::new(),
            Some(t) => {
                let t = t.to_lowercase();
                ms.iter()
                    .filter(|m| {
                        !m.username.is_empty()
                            && (m.username.to_lowercase().starts_with(&t)
                                || m.name.to_lowercase().starts_with(&t)
                                || t.is_empty())
                    })
                    .take(6)
                    .cloned()
                    .collect::<Vec<MemberRow>>()
            }
        }
    });
    let mention_rows = SignalCollection::new(mention_sig.clone());
    // `completion_off` is the Escape-dismissed latch — the popup stays
    // hidden until the composer text changes again.
    let mention_show = mention_sig
        .zip(&store.completion_off)
        .map(|(v, off)| !v.is_empty() && !off)
        .distinct();
    // `:shortcode` emoji completion: same trailing-token surface as
    // @mention, sourced from the static shortcode table.
    let emoji_sig = store.composer.map(|q: Str| Store::emoji_suggest(&q));
    let emoji_rows = SignalCollection::new(emoji_sig.clone());
    let emoji_show = emoji_sig
        .zip(&store.completion_off)
        .map(|(v, off)| !v.is_empty() && !off)
        .distinct();
    // `/` bot-command completion: a leading `/token` filters the selected
    // chat's command table (Demo: `bot_cmds`; real path: `getCommands`).
    let store_cmds = store.clone();
    let botcmd_sig = store.composer.zip(&store.selected).map(move |(q, sel)| {
        sel.map(|id| store_cmds.botcmd_suggest(id, &q))
            .unwrap_or_default()
    });
    let botcmd_rows = SignalCollection::new(botcmd_sig.clone());
    let botcmd_show = botcmd_sig
        .zip(&store.completion_off)
        .map(|(v, off)| !v.is_empty() && !off)
        .distinct();
    let poll_open = store.poll_open.clone();
    let store_for_poll = store.clone();
    // Modal Escape scopes, cloned before `store` moves into the `when`
    // closures below; the last-built active scope wins the key.
    let esc_members = modal_escape(store.clone(), |s| s.members_open.set(false));
    let esc_scheduled = modal_escape(store.clone(), |s| s.scheduled_open.set(false));
    let esc_search = modal_escape(store.clone(), |s| {
        s.chat_search_open.set(false);
        s.run_chat_search(Str::from(""));
    });
    let esc_stickers = modal_escape(store.clone(), |s| s.stickers_open.set(false));
    vstack((
        vstack((
            when(has_pinned, move || {
                hstack((
                    pin().tint(Accent).size(14.0, 14.0).a11y_hidden(true),
                    text!("{pinned_label}{pinned_count}")
                        .caption()
                        .line_limit(ONE)
                        .foreground(Accent),
                    spacer(),
                ))
                .padding_with((6.0, 12.0))
                .on_tap(|store: Store| store.pinned_tap())
                .background(Surface)
            }),
            // Multi-pin list (Desktop's "View all pinned" popup): each row
            // jumps to that message and closes the popup. Kept as a sibling
            // `when`, not nested in the banner's payload — a `when` inside
            // another `when`'s payload never materializes live (r36-2).
            when(store.pinned_popup.clone(), move || {
                VStack::for_each(pinned_rows.clone(), |row: PinnedRow| {
                    let id = row.id;
                    let label = row.label.clone();
                    hstack((
                        pin()
                            .tint(MutedForeground)
                            .size(12.0, 12.0)
                            .a11y_hidden(true),
                        text(label.clone()).caption().line_limit(ONE),
                        spacer(),
                    ))
                    .spacing(6.0)
                    .padding_with((4.0, 12.0))
                    .on_tap(move |store: Store| store.pinned_jump(id))
                    .a11y_label(Str::from(format!("Jump to pinned: {label}")))
                })
                .background(SurfaceVariant)
            }),
            // Jump-to-date popup (calendar toolbar item): sibling `when` per
            // the r36-2 nested-`when` defect — same pattern as pinned_popup.
            when(store.jump_date_open.clone(), {
                let days = SignalCollection::new(store.jump_days.clone());
                move || {
                    VStack::for_each(days.clone(), |d: DayRow| {
                        let day = d.day;
                        let label = d.label.clone();
                        hstack((
                            calendar()
                                .tint(MutedForeground)
                                .size(12.0, 12.0)
                                .a11y_hidden(true),
                            text(label.clone()).caption(),
                            spacer(),
                            when(d.count > 0, move || {
                                text(d.count.to_string()).caption().muted()
                            }),
                        ))
                        .spacing(6.0)
                        .padding_with((4.0, 12.0))
                        .on_tap(move |store: Store| store.jump_to_day(day))
                        .a11y_label(Str::from(format!("Jump to {label}")))
                    })
                    .background(SurfaceVariant)
                }
            }),
        )),
        // Multi-select action bar (Desktop's "N selected" header state).
        when(sel_active, move || {
            hstack((
                text!("{n} selected", n = sel_count.clone())
                    .caption()
                    .bold(),
                spacer(),
                icon_button(content_copy(), "Copy selected", |store: Store| {
                    store.copy_selected()
                }),
                icon_button(share_variant(), "Forward selected", |store: Store| {
                    store.forward_selected()
                }),
                icon_button(delete_sweep(), "Delete selected", |store: Store| {
                    store.delete_selected()
                }),
                icon_button(close(), "Clear selection", |store: Store| {
                    store.clear_selection()
                }),
            ))
            .spacing(6.0)
            .padding_with((6.0, 12.0))
            .background(Surface)
        }),
        vstack((
            when(has_more, || {
                button("Load earlier messages…")
                    .label_style(LabelDisplayMode::TitleOnly)
                    .action(|store: Store| store.load_older())
            }),
            List::for_each(messages, move |row: MessageRow| {
                let rid = row.id;
                // Desktop packs a same-sender run nearly flush (~1pt) while runs
                // stay separated (~4pt): the outer edge of a run keeps the full
                // inset, inner edges collapse.
                let pad_top = if row.group_first { 2.0 } else { 0.5 };
                let pad_bottom = if row.group_last { 2.0 } else { 0.5 };
                let is_service = row.is_service;
                let check = sel_msgs_rows
                    .map(move |v: Vec<i64>| v.contains(&rid))
                    .distinct();
                ListItem::new(
                    hstack((
                        when(sel_active_rows.clone(), move || {
                            let check = check.clone();
                            when(!is_service, move || {
                                when(check.clone(), || text("☑").body().foreground(Accent))
                                    .otherwise(|| text("☐").body().muted())
                                    .padding_with((0.0, 4.0))
                            })
                        }),
                        message_bubble(inner.clone(), row),
                    ))
                    .spacing(4.0)
                    .on_tap(move |store: Store| {
                        // Only multi-select mode: context-menu "Select"
                        // seeds the set; further taps toggle membership.
                        if !is_service && !store.selected_msgs.snapshot().is_empty() {
                            store.toggle_select(rid);
                        }
                    })
                    .padding_with([pad_top, pad_bottom, 12.0, 12.0]),
                )
                // waterui#1252: a service row is sized to its content plus
                // these insets (`.list_min_row_height(0.0)` below), so the
                // pill keeps Desktop's compact spacing. Bubble rows keep the
                // theme's row insets.
                .insets(if is_service {
                    EdgeInsets::symmetric(2.0, 0.0)
                } else {
                    EdgeInsets::all(0.0)
                })
            })
            .scroll_controller(&scroller)
            // 0.0 sizes each row to its content plus its insets (waterui#1252)
            // — this lifts the theme's one-line row-height floor that padded
            // service pills to 56 pt (r31-1).
            .list_min_row_height(0.0),
        )),
        when(store.members_open.clone(), move || {
            vstack((
                hstack((
                    text!(
                        "{members_count}",
                        members_count = store.members_count.clone()
                    )
                    .caption()
                    .muted(),
                    spacer(),
                    icon_button(close(), "Close members", |store: Store| {
                        store.members_open.set(false)
                    }),
                ))
                .padding_with((6.0, 10.0)),
                hstack((
                    field("Group title", &store.admin_title).hide_label(),
                    button("Rename").action(|store: Store| store.rename_chat()),
                ))
                .spacing(6.0)
                .padding_with((0.0, 10.0)),
                hstack((
                    field("Group description", &store.admin_desc).hide_label(),
                    button("Set").action(|store: Store| store.set_chat_desc()),
                ))
                .spacing(6.0)
                .padding_with((0.0, 10.0)),
                hstack((
                    FilePicker::open(
                        label("Photo").icon(image_outline()),
                        &store.chat_avatar_pick,
                    ),
                    button("Set photo").action(|store: Store| store.set_chat_avatar()),
                ))
                .spacing(6.0)
                .padding_with((0.0, 10.0)),
                hstack((
                    link_variant().tint(MutedForeground).size(14.0, 14.0),
                    text!("{invite_link}", invite_link = store.invite_link.clone())
                        .caption()
                        .line_limit(ONE)
                        .muted(),
                    spacer(),
                    button("New link").action(|store: Store| store.create_invite()),
                ))
                .spacing(6.0)
                .padding_with((4.0, 10.0)),
                scroll(VStack::for_each(
                    SignalCollection::new(store.members.clone()),
                    move |row: MemberRow| {
                        let r_kick = row.clone();
                        let is_user = matches!(row.sender, enums::MessageSender::User(_));
                        let uid = match &row.sender {
                            enums::MessageSender::User(u) => u.user_id,
                            _ => 0,
                        };
                        hstack((
                            text(row.name.clone()).caption(),
                            spacer(),
                            text(row.status.clone()).caption().muted(),
                        ))
                        .spacing(6.0)
                        .padding_with((4.0, 10.0))
                        .on_tap(move |store: Store| {
                            if is_user {
                                store.open_profile(uid)
                            }
                        })
                        .a11y_label(Str::from(format!("Open profile of {}", row.name)))
                        .a11y_role(AccessibilityRole::Button)
                        .context_menu((
                            "Kick".action(move |store: Store| store.kick_member(&r_kick)),
                            "Block".action(move |store: Store| store.toggle_block(&row, true)),
                        ))
                    },
                ))
                .max_height(200.0),
            ))
            .background(Surface)
            .with(esc_members.clone())
        }),
        // Scheduled messages panel (nav-toolbar clock icon).
        when(scheduled_open, move || {
            vstack((
                hstack((
                    text("Scheduled messages").caption().muted(),
                    spacer(),
                    icon_button(close(), "Close scheduled", |store: Store| {
                        store.scheduled_open.set(false)
                    }),
                ))
                .padding_with((4.0, 10.0)),
                scroll(VStack::for_each(
                    SignalCollection::new(scheduled_b.clone()),
                    |row: MessageRow| {
                        let mid = row.id;
                        let mut label = row.text.clone();
                        if label.is_empty() {
                            label = row.media_label.clone();
                        }
                        hstack((
                            clock_outline().tint(MutedForeground).size(12.0, 12.0),
                            text(label).caption().line_limit(ONE),
                            spacer(),
                            text(row.time.clone()).caption().muted(),
                        ))
                        .spacing(6.0)
                        .padding_with((4.0, 10.0))
                        .context_menu((
                            "Send now".action(move |store: Store| store.scheduled_send_now(mid)),
                        ))
                    },
                ))
                .max_height(150.0),
            ))
            .background(Surface)
            .with(esc_scheduled.clone())
        }),
        when(search_open, move || {
            vstack((
                hstack((
                    field("Search in chat", &search_b)
                        .prompt("Search in this chat")
                        .hide_label()
                        // Enter jumps to the next match (Telegram Desktop).
                        .on_submit(|store: Store| store.chat_search_next()),
                    text!("{m}", m = match_label.clone()).caption().muted(),
                    icon_button(chevron_up(), "Previous match", |store: Store| {
                        store.chat_search_prev()
                    }),
                    icon_button(chevron_down(), "Next match", |store: Store| {
                        store.chat_search_next()
                    }),
                    icon_button(close(), "Close search", |store: Store| {
                        store.chat_search_open.set(false);
                        store.run_chat_search(Str::from(""));
                    }),
                ))
                .spacing(6.0)
                .padding_with((6.0, 10.0)),
                scroll(VStack::for_each(
                    SignalCollection::new(search_results_b.clone()),
                    move |row: MessageRow| {
                        hstack((
                            text(row.time.clone()).caption().muted(),
                            text(row.text.clone()).caption().line_limit(ONE),
                            spacer(),
                        ))
                        .spacing(6.0)
                        .padding_with((4.0, 10.0))
                        .on_tap(move |store: Store| store.jump_to_message(row.id))
                    },
                ))
                .max_height(160.0),
            ))
            .background(Surface)
            .with(esc_search.clone())
        }),
        when(reply_open, move || {
            banner("Reply to", reply_label.clone(), |store: Store| {
                store.reply_to.set(None)
            })
        }),
        when(edit_open, || {
            banner("Editing message", "", |store: Store| {
                store.editing.set(None);
                store.composer.set_from("");
            })
        }),
        when(voice_rec, move || {
            hstack((
                text!("Recording… {voice_elapsed}")
                    .caption()
                    .foreground(Accent),
                spacer(),
                icon_button(close(), "Cancel recording", |store: Store| {
                    store.cancel_voice_record()
                }),
                icon_button(send(), "Send voice note", |store: Store| {
                    store.toggle_voice_record()
                }),
            ))
            .padding_with((6.0, 12.0))
            .background(Surface)
        }),
        when(has_capture_error, move || {
            banner("Capture", text!("{capture_error_text}"), |store: Store| {
                store.capture_error.set_from("")
            })
        }),
        when(video_note_open, move || {
            video_note_sheet(store_for_sheet.clone())
        }),
        when(has_attach, {
            let first_url = store.attach.map(|v: Vec<Url>| {
                v.first()
                    .map(|u| Url::from_file_path_str(Str::from(u.path().to_string())))
                    .unwrap_or_else(|| Url::from_file_path_str(Str::from("")))
            });
            let is_img = store.attach.map(|v: Vec<Url>| {
                v.first()
                    .map(|u| {
                        let p = u.path().to_string().to_lowercase();
                        [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
                            .iter()
                            .any(|e| p.ends_with(e))
                    })
                    .unwrap_or(false)
            });
            let fname = store.attach.map(|v: Vec<Url>| {
                v.first()
                    .map(|u| {
                        u.path()
                            .to_string()
                            .rsplit('/')
                            .next()
                            .unwrap_or("file")
                            .to_string()
                    })
                    .unwrap_or_default()
            });
            let cap_b = store.attach_caption.clone();
            move || {
                let is_img = is_img.clone();
                let first_url = first_url.clone();
                let fname = fname.clone();
                let cap_b = cap_b.clone();
                hstack((
                    when(is_img, {
                        let u = first_url.clone();
                        move || {
                            Photo::new(u.clone())
                                .width(40.0)
                                .height(40.0)
                                .clip(RoundedRectangle::new(0.15))
                        }
                    })
                    .otherwise(|| paperclip().tint(Accent).size(24.0, 24.0).a11y_hidden(true)),
                    vstack((
                        text!("{f}", f = fname.clone())
                            .caption()
                            .line_limit(ONE)
                            .foreground(Accent),
                        field("Caption", &cap_b)
                            .prompt("Add a caption…")
                            .hide_label(),
                    ))
                    .spacing(2.0)
                    .leading(),
                    spacer(),
                    icon_button(close(), "Remove", |store: Store| {
                        store.attach.set(Vec::new());
                        store.attach_caption.set_from("")
                    }),
                ))
                .spacing(8.0)
                .padding_with((6.0, 12.0))
                .background(Surface)
            }
        }),
        when(stickers_open, move || {
            let cells = store_cells.clone();
            let emoji_tab2 = emoji_tab.clone();
            let sticker_q = store.sticker_query.clone();
            // Bound outside the `when` payload: disjoint-field capture would
            // otherwise move `store.emoji_query` and make the payload FnOnce.
            let emoji_q = store.emoji_query.clone();
            // The inner `when` payload move-captures `eq2`; cloning here keeps
            // the outer `Fn` closure borrow-only.
            let eq2 = emoji_q.clone();
            let packs_b = store.sticker_packs.clone();
            let items_b = sticker_items.clone();
            let recents = SignalCollection::new(store.recent_emojis.clone());
            vstack((
                // Picker tabs: Emoji grid is local; Stickers/GIFs query TDLib.
                hstack((
                    panel_tab_chip("Emoji", 0),
                    panel_tab_chip("Stickers", 1),
                    panel_tab_chip("GIFs", 2),
                    spacer(),
                ))
                .spacing(4.0)
                .padding_with((2.0, 8.0)),
                when(emoji_tab2, move || {
                    // Panel search field like Desktop's — substring match over
                    // the shortcode table; `watch` swaps the whole grid region
                    // so the filtered list never rides the late-mounted `when`
                    // payload hazard (r36-2/#251).
                    let recents = recents.clone();
                    vstack((
                        field("Emoji search", &eq2).prompt("😀 emoji").hide_label(),
                        // Desktop's "Frequently used" strip — most-recently
                        // inserted first, tapped cells insert like grid cells.
                        vstack((
                            text("Frequently used").caption().muted(),
                            scroll_horizontal(HStack::for_each(recents, |cell: RecentEmoji| {
                                let emo = cell.emoji.clone();
                                let label = cell.emoji.clone();
                                text(emo.to_string())
                                    .headline()
                                    .padding_with((4.0, 4.0))
                                    .on_tap(move |store: Store| store.insert_emoji(&emo))
                                    .a11y_label(label)
                                    .a11y_role(AccessibilityRole::Button)
                            })),
                        )),
                        watch(eq2.clone(), move |q: Str| {
                            let hits = Store::emoji_search(&q);
                            if hits.is_empty() {
                                emoji_grid().anyview()
                            } else {
                                scroll(vstack(
                                    hits.iter()
                                        .map(|s| {
                                            let emo = s.emoji.to_string();
                                            hstack((
                                                text(s.emoji.clone()).body(),
                                                text!(":{n}:", n = s.name.clone())
                                                    .caption()
                                                    .muted(),
                                                spacer(),
                                            ))
                                            .spacing(8.0)
                                            .padding_with((4.0, 8.0))
                                            .on_tap(move |store: Store| store.insert_emoji(&emo))
                                            .a11y_label(Str::from(format!("Insert :{}:", s.name)))
                                            .a11y_role(AccessibilityRole::Button)
                                            .anyview()
                                        })
                                        .collect::<Vec<_>>(),
                                ))
                                .max_height(140.0)
                                .anyview()
                            }
                        }),
                    ))
                    .spacing(4.0)
                })
                .otherwise(move || {
                    let cells = cells.clone();
                    vstack((
                        hstack((
                            field("Emoji search", &sticker_q)
                                .prompt("😀 emoji")
                                .hide_label(),
                            icon_button(magnify(), "Search stickers", |store: Store| {
                                store.search_stickers_by()
                            }),
                            {
                                let q = sticker_q.clone();
                                icon_button(file_gif_box(), "Search GIFs", move |store: Store| {
                                    store.search_gifs_by(q.snapshot().to_string())
                                })
                            },
                        ))
                        .spacing(6.0)
                        .padding_with((2.0, 8.0)),
                        scroll_horizontal(HStack::for_each(
                            SignalCollection::new(packs_b.clone()),
                            move |pack: PackRow| {
                                let id = pack.id;
                                text(pack.title.clone())
                                    .caption()
                                    .padding_with((2.0, 8.0))
                                    .background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
                                    .on_tap(move |store: Store| store.open_sticker_pack(id))
                            },
                        ))
                        .max_height(28.0),
                        scroll_horizontal(HStack::for_each(
                            SignalCollection::new(items_b.clone()),
                            move |item: StickerItem| {
                                let cell_store = cells.clone();
                                let thumb = item.thumb;
                                let mut fallback: Str = item.emoji.clone();
                                if item.gif {
                                    fallback = "GIF".into();
                                }
                                let it = item.clone();
                                let has = cell_store
                                    .file_signal(thumb)
                                    .map(|p: Str| !p.is_empty())
                                    .distinct();
                                let no_file =
                                    cell_store.file_signal(thumb).str_is_empty().distinct();
                                let url =
                                    cell_store.file_signal(thumb).map(Url::from_file_path_str);
                                let cell = zstack((
                                    when(has, move || {
                                        Photo::new(url.clone()).max_width(72.0).max_height(72.0)
                                    }),
                                    when(no_file, move || text(fallback.clone()).headline()),
                                ))
                                .anyview();
                                cell.padding_with((6.0, 6.0))
                                    .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
                                    .on_tap(move |store: Store| store.send_sticker(it.clone()))
                                    .a11y_label("Send sticker")
                                    .a11y_role(AccessibilityRole::Button)
                            },
                        ))
                        .max_height(96.0),
                    ))
                    .spacing(4.0)
                }),
            ))
            .with(esc_stickers.clone())
        }),
        vstack((
            when(mention_show, move || {
                VStack::for_each(mention_rows.clone(), move |m: MemberRow| {
                    let uname = m.username.clone();
                    hstack((
                        text!("@{u}", u = uname.clone())
                            .caption()
                            .bold()
                            .foreground(Accent),
                        text(m.name.clone()).caption().muted(),
                        spacer(),
                    ))
                    .spacing(8.0)
                    .padding_with((6.0, 12.0))
                    .on_tap(move |store: Store| store.apply_mention(&uname))
                    .a11y_label(Str::from(format!("Mention @{u}", u = m.username)))
                    .a11y_role(AccessibilityRole::Button)
                })
                .spacing(0.0)
                .padding_with((4.0, 0.0))
                .background(Surface)
                .clip(RoundedRectangle::new(0.12))
            }),
            when(emoji_show, move || {
                VStack::for_each(emoji_rows.clone(), move |s: EmojiSug| {
                    let emo = s.emoji.to_string();
                    hstack((
                        text(s.emoji.clone()).body(),
                        text!(":{n}:", n = s.name.clone()).caption().muted(),
                        spacer(),
                    ))
                    .spacing(8.0)
                    .padding_with((6.0, 12.0))
                    .on_tap(move |store: Store| store.apply_emoji(&emo))
                    .a11y_label(Str::from(format!("Insert :{}:", s.name)))
                    .a11y_role(AccessibilityRole::Button)
                })
                .spacing(0.0)
                .padding_with((4.0, 0.0))
                .background(Surface)
                .clip(RoundedRectangle::new(0.12))
            }),
            when(botcmd_show, move || {
                VStack::for_each(botcmd_rows.clone(), move |c: BotCmd| {
                    let cmd = c.cmd.to_string();
                    hstack((
                        text(Str::from(format!("/{}", c.cmd))).body(),
                        text(c.desc.clone()).caption().muted(),
                        spacer(),
                    ))
                    .spacing(8.0)
                    .padding_with((6.0, 12.0))
                    .on_tap(move |store: Store| store.apply_botcmd(&cmd))
                    .a11y_label(Str::from(format!("Run /{}", c.cmd)))
                    .a11y_role(AccessibilityRole::Button)
                })
                .spacing(0.0)
                .padding_with((4.0, 0.0))
                .background(Surface)
                .clip(RoundedRectangle::new(0.12))
            }),
            when(poll_open, {
                let st = store_for_poll.clone();
                move || poll_creator(st.clone())
            }),
        ))
        .spacing(0.0),
        // `chatActionBar*` — Telegram Desktop replaces the composer with the
        // bar's buttons (Report spam / Add contact / Share phone / …) plus a
        // trailing ✕ that hides it (`removeChatActionBar`).
        when(bar_present.not(), {
            // `when` builders are `Fn` — clone the signal handles inside
            // instead of consuming the captures.
            let composer_b = composer_b.clone();
            let composer_empty = composer_empty.clone();
            move || {
                hstack((
                    FilePicker::open(
                        label("Attach file").icon(paperclip()).icon_only(),
                        &store.attach,
                    )
                    .context_menu((
                        "Create poll".action(|store: Store| store.toggle_poll_creator()),
                    )),
                    icon_button(emoticon(), "Stickers & GIFs", |store: Store| {
                        store.toggle_stickers()
                    }),
                    field("Message", &composer_b)
                        .prompt("Message")
                        .hide_label()
                        // waterui#1265: Return submits (send) via the field's own
                        // on_submit instead of an ancestor key handler.
                        .on_submit(|store: Store| store.submit_composer()),
                    // Search/members/scheduled live in the navigation toolbar
                    // (Telegram Desktop parity). A `Spacer` before the mic/send slot
                    // keeps it trailing-aligned.
                    spacer(),
                    when(composer_empty.clone(), || {
                        icon_button(microphone(), "Record voice note", |store: Store| {
                            store.toggle_voice_record()
                        })
                    })
                    .otherwise(|| {
                        icon_button(send(), "Send", |store: Store| store.send()).context_menu((
                            "Send silently".action(|store: Store| store.send_silent()),
                            "Send in 1 hour".action(|store: Store| store.send_later(3600)),
                        ))
                    }),
                    icon_button(camera(), "Video note", |store: Store| {
                        store.open_video_note()
                    }),
                ))
                .spacing(4.0)
                .padding_with((4.0, 6.0))
                .background(Surface)
                // Keys bubble up from the focused field (waterui#1265): Escape
                // dismisses completion / cancels edit or reply, ArrowUp in an empty
                // composer edits the last outgoing message (Telegram Desktop).
                .on_key_press(|Use(press): Use<KeyPress>, store: Store| match press.key {
                    Key::Named(NamedKey::Escape) => {
                        if store.composer_escape() {
                            KeyHandling::Handled
                        } else {
                            KeyHandling::Ignored
                        }
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        if store.edit_last_own() {
                            KeyHandling::Handled
                        } else {
                            KeyHandling::Ignored
                        }
                    }
                    _ => KeyHandling::Ignored,
                })
            }
        })
        .otherwise(move || action_bar_view(store_bar.clone())),
    ))
    // Appearance → chat-background preset paints the whole conversation
    // pane behind the bubbles (Telegram Desktop); `signal_color` keeps the
    // fill a live signal rather than remounting the column.
    .background(signal_color(bg_color))
}

/// Telegram Desktop's `chatActionBar` strip — full-width label buttons plus a
/// trailing ✕ that hides the bar (`removeChatActionBar`). The labels come
/// from the bar kind (`action_bar_parts`); taps dispatch per kind via
/// `Store::action_bar_run`.
fn action_bar_view(store: Store) -> impl View {
    let (kind, title, _peer) = store.open_action_bar().unwrap_or_default();
    let labels: Vec<Str> = match kind.as_str() {
        "report_spam" => vec!["Report spam and leave".into()],
        "report_add_block" => vec!["Report".into(), "Add to blocklist".into()],
        "add_contact" => vec!["Add contact".into()],
        "share_phone" => vec!["Share my phone number".into()],
        "invite_members" => vec!["Invite members".into()],
        "join_request" => vec![format!("Apply to join \"{title}\"").into()],
        _ => vec!["Continue".into()],
    };
    let mut btns: Vec<AnyView> = labels
        .iter()
        .enumerate()
        .map(|(n, label)| {
            let n = n as i32;
            let label = label.clone();
            hstack((
                spacer(),
                text(label.clone()).body().bold().foreground(Accent),
                spacer(),
            ))
            .padding_with((10.0, 4.0))
            .on_tap(move |store: Store| store.action_bar_run(n))
            .a11y_role(AccessibilityRole::Button)
            .a11y_label(label.clone())
            .max_width(f32::INFINITY)
            .anyview()
        })
        .collect();
    btns.push(icon_button(close(), "Hide", |store: Store| store.action_bar_dismiss()).anyview());
    vstack((Divider, hstack(btns).spacing(0.0))).spacing(0.0)
}

/// "Show message info" card — sent/read time, view count and the seen-by
/// list (`getMessageReadDate` + `getMessageViewers` on the real path).
fn msg_info_card(store: Store) -> impl View {
    let info = store.msg_info.clone();
    let sent = info.map(|i| i.as_ref().map(|i| i.sent.clone()).unwrap_or_default());
    let read = info.map(|i| i.as_ref().map(|i| i.read.clone()).unwrap_or_default());
    let views = info.map(|i| i.as_ref().map(|i| i.views.clone()).unwrap_or_default());
    let from = info.map(|i| i.as_ref().map(|i| i.from.clone()).unwrap_or_default());
    let seen = info.map(|i| {
        i.as_ref()
            .map(|i| {
                if i.seen.is_empty() {
                    Str::from("")
                } else {
                    let names: Vec<Str> = i.seen.clone();
                    Str::from(format!(
                        "Seen by {}",
                        names
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            })
            .unwrap_or_default()
    });
    let esc = modal_escape(store.clone(), |s| s.dismiss_msg_info());
    zstack((
        // Scrim — same treatment as the delete-confirm card.
        Rectangle
            .fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.45))
            .on_tap(|store: Store| store.dismiss_msg_info())
            .a11y_label("Dismiss message info")
            .a11y_role(AccessibilityRole::Button),
        vstack((
            text("Message info").body().bold(),
            text!("{from}").caption().muted(),
            Divider,
            text!("{sent}").caption(),
            text!("{read}").caption(),
            text!("{views}").caption(),
            text!("{seen}").caption().muted(),
            hstack((
                spacer(),
                text("Close")
                    .body()
                    .bold()
                    .foreground(Accent)
                    .padding_with((6.0, 12.0))
                    .on_tap(|store: Store| store.dismiss_msg_info())
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_label("Close message info"),
            )),
        ))
        .spacing(8.0)
        .padding_with(16.0)
        .background(Surface)
        .clip(RoundedRectangle::new(0.08))
        .max_width(300.0),
    ))
    .with(esc)
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn chat_detail(store: Store, chat_id: i64) -> NavigationView {
    let title = store
        .chats
        .map(move |rows| {
            rows.iter()
                .find(|r| r.id == chat_id)
                .map(|r| {
                    // The nav title slot is text-only (IntoText) — the
                    // verification mark travels as a glyph suffix, same
                    // position Desktop draws its badge.
                    match r.badge {
                        Verification::Verified => Str::from(format!("{} ✓", r.title)),
                        Verification::Scam => Str::from(format!("{} ⚠", r.title)),
                        Verification::None => r.title.clone(),
                    }
                })
                .unwrap_or_default()
        })
        .distinct();
    let subtitle = store.chat_subtitle(chat_id);

    let composer_b = store.composer.clone();
    let search_open2 = store.chat_search_open.clone();
    let search_live = store.chat_search.debounce(Duration::from_millis(400));
    let viewer_open = store.viewer.is_some().distinct();
    let store_for_viewer = store.clone();
    let store_for_info = store.clone();
    let store_info_card = store.clone();
    let msg_info_open = store.msg_info.is_some().distinct();
    let info_docked = store
        .info_open
        .zip(&store.win_frame)
        .map(|(open, f)| open && f.width() >= 1120.0)
        .distinct();
    let info_overlay = store
        .info_open
        .zip(&store.win_frame)
        .map(|(open, f)| open && f.width() < 1120.0)
        .distinct();
    // r35: the open chat's unread-mention count drives the floating `@`
    // jump button (Telegram Desktop's bottom-right quick-jump).
    let open_mentions = store
        .chats
        .zip(&store.selected)
        .map(|(rows, open)| {
            open.and_then(|id| rows.iter().find(|r| r.id == id).map(|r| r.unread_mentions))
                .unwrap_or(0)
        })
        .distinct();
    let has_mentions = open_mentions.is_positive().distinct();
    // Open chat's unread-reaction count drives the floating ❤ button.
    let open_reactions = store
        .chats
        .zip(&store.selected)
        .map(|(rows, open)| {
            open.and_then(|id| rows.iter().find(|r| r.id == id).map(|r| r.unread_reactions))
                .unwrap_or(0)
        })
        .distinct();
    let has_reactions = open_reactions.is_positive().distinct();
    // Open chat's unread count drives the floating catch-up chip.
    let open_unread = store
        .chats
        .zip(&store.selected)
        .map(|(rows, open)| {
            open.and_then(|id| rows.iter().find(|r| r.id == id).map(|r| r.unread))
                .unwrap_or(0)
        })
        .distinct();
    let del_open = store.confirm_delete.is_some().distinct();
    let store_del = store.clone();
    let com_open = store.comments_open.is_some().distinct();
    let store_com = store.clone();
    let store_for_info_overlay = store.clone();
    // Honest reproduction: `when(a).otherwise(b)` (WhenComplete) panics at
    // mount on the shipped renderers — nami#23. Kept in this
    // form until the upstream fix lands; no workarounds.
    zstack((
        when(info_docked, {
            let st = store.clone();
            let s2 = store_for_info.clone();
            move || {
                hstack((
                    chat_column(st.clone()),
                    Color::from(BorderColor).width(1.0),
                    info_panel(s2.clone()),
                ))
            }
        })
        .otherwise({
            let st = store.clone();
            let ov = info_overlay.clone();
            let s3 = store_for_info_overlay.clone();
            move || {
                let ov2 = ov.clone();
                let s4 = s3.clone();
                zstack((
                    chat_column(st.clone()),
                    when(ov2, move || info_overlay_chunk(s4.clone())),
                ))
            }
        }),
        when(has_mentions, move || {
            vstack((
                spacer(),
                hstack((
                    spacer(),
                    at().tint(AccentForeground)
                        .size(20.0, 20.0)
                        .padding_with(10.0)
                        .background(Circle.fill(Accent))
                        // hydrolysis#221 workaround: hide the leaf so the
                        // gesture emits the single named Button node.
                        .a11y_hidden(true)
                        .on_tap(|store: Store| store.mention_jump())
                        .a11y_label("Jump to the unread mention")
                        .a11y_role(AccessibilityRole::Button),
                )),
            ))
            // Stacks above the ❤ button (Desktop: @ over ❤ over ↓).
            .padding_with([0.0, 188.0, 0.0, 16.0])
        }),
        // Floating ❤ button — jumps to the first message with unread
        // reactions (Desktop parity, same slot family as the @ button).
        when(has_reactions, move || {
            vstack((
                spacer(),
                hstack((
                    spacer(),
                    heart()
                        .tint(AccentForeground)
                        .size(20.0, 20.0)
                        .padding_with(10.0)
                        .background(Circle.fill(Accent))
                        // hydrolysis#221 workaround: hide the leaf so the
                        // gesture emits the single named Button node.
                        .a11y_hidden(true)
                        .on_tap(|store: Store| store.reaction_jump())
                        .a11y_label("Jump to the unread reaction")
                        .a11y_role(AccessibilityRole::Button),
                )),
            ))
            .padding_with([0.0, 136.0, 0.0, 16.0])
        }),
        // Floating "N unread ↓" catch-up chip — Desktop's bottom-right
        // button while the open chat carries unread. Without a scroll
        // readback (waterui#1259) it shows while unread > 0 and clears on
        // tap; real clients will hide it at the tail once #1259 lands.
        when(open_unread.is_positive().distinct(), move || {
            let n = open_unread.clone();
            vstack((
                spacer(),
                hstack((
                    spacer(),
                    hstack((
                        chevron_down()
                            .tint(AccentForeground)
                            .size(14.0, 14.0)
                            .a11y_hidden(true),
                        text!("{n}").caption().bold().foreground(AccentForeground),
                    ))
                    .spacing(2.0)
                    .padding_with((7.0, 12.0))
                    .background(RoundedRectangle::new(0.5).fill(Accent))
                    .a11y_hidden(true)
                    .on_tap(|store: Store| store.catch_up())
                    .a11y_label("Mark all read and jump to latest")
                    .a11y_role(AccessibilityRole::Button),
                )),
            ))
            .padding_with([0.0, 84.0, 0.0, 16.0])
        }),
        // Delete-confirm card (Desktop's "Delete N messages?" dialog with
        // the "Also delete for <peer>" checkbox in private chats).
        when(del_open, move || delete_confirm_card(store_del.clone())),
        when(com_open, move || comments_card(store_com.clone())),
        // "Show message info" card (Desktop's message-info dialog).
        when(msg_info_open, move || {
            msg_info_card(store_info_card.clone())
        }),
        when(viewer_open, move || viewer_layer(store_for_viewer.clone())),
    ))
    .on_change(&search_live, |q: Str, store: Store| {
        store.run_chat_search(q)
    })
    .on_change(&composer_b, |_: Str, store: Store| {
        // A new keystroke re-arms the completion popup after Escape.
        store.completion_off.set(false);
        store.typing_ping();
        store.maybe_load_members();
    })
    .title(text!("{title}"))
    .navigation_subtitle(text!("{subtitle}"))
    .navigation_toolbar(
        // `::new` keeps the 40dp icon chip; `::action` draws label+icon and
        // measured ~170dp, which crowds the title in a 460dp pane.
        NavigationToolbar::default()
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                icon_button(magnify(), "Search in chat", move |store: Store| {
                    let v = !search_open2.snapshot();
                    store.chat_search_open.set(v);
                }),
            ))
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                icon_button(calendar(), "Jump to date", move |store: Store| {
                    store.toggle_jump_date()
                }),
            ))
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                icon_button(account_group(), "Members", move |store: Store| {
                    let open = !store.members_open.snapshot();
                    store.members_open.set(open);
                    if open {
                        store.load_members();
                    }
                }),
            ))
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                icon_button(clock_outline(), "Scheduled messages", |store: Store| {
                    store.toggle_scheduled()
                }),
            ))
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                icon_button(information(), "Chat info", |store: Store| {
                    store.toggle_info()
                }),
            ))
            .item(NavigationToolbarItem::new(
                NavigationToolbarPlacement::TopBarTrailing,
                // The overflow ⋮ menu stays mounted while the chat is
                // open, so its `shortcut` chords dispatch window-wide
                // (hydrolysis#261): Ctrl+F search, Ctrl+R mark as read —
                // Telegram Desktop's chat-level accelerators.
                Menu::new(
                    label("Chat actions").icon(dots_vertical()).icon_only(),
                    (
                        "Search in chat"
                            .action(move |store: Store| store.chat_search_open.set(true))
                            .shortcut(Shortcut::new('f').control()),
                        "Mark as read"
                            .action(move |store: Store| store.mark_read(chat_id))
                            .shortcut(Shortcut::new('r').control()),
                        "Clear history".action(move |store: Store| store.clear_history(chat_id)),
                        Divider,
                        "Leave chat"
                            .command()
                            .action(move |store: Store| store.leave(chat_id))
                            .destructive(),
                    ),
                ),
            )),
    )
}

pub(crate) fn banner(
    kind: &'static str,
    label_text: impl IntoText + 'static,
    on_close: impl Fn(Store) + 'static,
) -> impl View {
    hstack((
        text(kind).caption().bold().foreground(Accent),
        text(label_text).caption().line_limit(ONE).muted(),
        spacer(),
        icon_button(close(), "Dismiss", move |store: Store| on_close(store)),
    ))
    .padding_with((6.0, 12.0))
    .background(Surface)
}

/// Telegram Desktop renders a message that contains only emoji at a larger
/// size with no bubble background. Returns the emoji count, 0 if the text
/// contains anything else (whitespace, ZWJ, variation selectors, keycap and
/// tag markers do not count as content).
pub(crate) fn emoji_count(s: &str) -> usize {
    let mut n = 0usize;
    for c in s.chars() {
        match c as u32 {
            // Whitespace, ZWJ, variation selectors, keycap/tag markers, and
            // skin-tone modifiers don't count as separate emoji.
            0x20 | 0x200D | 0xFE0E | 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F => {}
            0x2300..=0x27BF | 0x2B00..=0x2BFF | 0x1F000..=0x1FAFF => n += 1,
            _ => return 0,
        }
    }
    n
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
/// Surface a reaction pill is drawn on — the unchosen tint is derived
/// from it (r25-1): the same component on the page background, inside an
/// incoming bubble, and inside an outgoing accent bubble.
#[derive(Clone, Copy)]
enum ChipSurface {
    /// Emoji-only row — the pill sits on the page background.
    Page,
    /// Inside an incoming or highlighted bubble.
    Incoming,
    /// Inside an outgoing (accent) bubble.
    Outgoing,
}

/// One reaction pill: tap toggles it, the chosen state is accented in
/// every placement (r25-1 — on the accent bubble the container tint
/// keeps it readable where an accent-on-accent pill would vanish).
fn reaction_chip(row: &MessageRow, c: &ReactionChip, surface: ChipSurface) -> AnyView {
    let emoji = c.emoji.clone();
    let e2 = emoji.to_string();
    let label = format!("{} {}", emoji, c.count);
    let r = row.clone();
    // Right-click a chip → who reacted, like Desktop's reaction-details
    // list: inert name rows (+ "+N more" for counts beyond the recent
    // window), and "Remove your reaction" when the chip is ours.
    let mut menu_items: Vec<Command> = c
        .reactors
        .iter()
        .map(|name| {
            Command::builder(text!(
                "{name} reacted with {emoji}",
                name = name.clone(),
                emoji = emoji.clone()
            ))
            .action(|| {})
            .disabled(true)
        })
        .collect();
    let extra = c.count - c.reactors.len() as i32;
    if extra > 0 && !menu_items.is_empty() {
        menu_items.push(
            Command::builder(text!("+{#extra} more", extra = extra))
                .action(|| {})
                .disabled(true),
        );
    }
    if c.chosen {
        let r3 = row.clone();
        let e3 = emoji.to_string();
        menu_items.push(
            "Remove your reaction"
                .action(move |store: Store| store.toggle_reaction(&r3, &e3))
                .destructive(),
        );
    }
    let pill = text(label)
        .caption()
        .padding_with((1.0, 6.0))
        .on_tap(move |store: Store| store.toggle_reaction(&r, &e2))
        .a11y_role(AccessibilityRole::Button)
        .a11y_label(format!(
            "React with {}, {} {}",
            emoji,
            c.count,
            if c.count == 1 {
                "reaction"
            } else {
                "reactions"
            }
        ));
    let pill = if menu_items.is_empty() {
        pill.anyview()
    } else {
        pill.context_menu(menu_items).anyview()
    };
    match (surface, c.chosen) {
        (ChipSurface::Outgoing, true) => pill
            .foreground(Accent)
            .background(RoundedRectangle::new(0.5).fill(AccentContainer))
            .anyview(),
        (ChipSurface::Outgoing, false) => pill
            .foreground(AccentForeground)
            .background(RoundedRectangle::new(0.5).fill(WithOpacity::new(AccentForeground, 0.18)))
            .anyview(),
        (_, true) => pill
            .foreground(AccentForeground)
            .background(RoundedRectangle::new(0.5).fill(Accent))
            .anyview(),
        (ChipSurface::Page, false) => pill
            .background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
            .anyview(),
        (ChipSurface::Incoming, false) => pill
            .background(RoundedRectangle::new(0.5).fill(Surface))
            .anyview(),
    }
}

/// Bubble tail: a small hook growing out of a run's last bubble at the
/// bottom corner on the sender's side (Telegram Desktop). Drawn as a
/// layer of the bubble's own background — same fill, so it reads as part
/// of the outline — and `.offset` pushes the wedge outward past the edge,
/// so no layout space is reserved (r26-2). The path's flat side lands a
/// few points inside the edge so no antialiased seam shows; the joined
/// corner is drawn square via `UnevenRoundedRectangle` (r26-1).
fn bubble_tail(outgoing: bool, fill: Color) -> AnyView {
    const W: f32 = 10.0;
    const H: f32 = 13.0;
    // PathCommand coordinates are normalized to the view's bounds — the
    // W×H frame pins the wedge's absolute size.
    let path = if outgoing {
        Path::new()
            .move_to(0.0, 0.0)
            .quad_to(0.18, 0.88, 0.9, 1.0)
            .line_to(0.0, 1.0)
            .close()
    } else {
        Path::new()
            .move_to(1.0, 0.0)
            .quad_to(0.82, 0.88, 0.1, 1.0)
            .line_to(1.0, 1.0)
            .close()
    };
    path.fill(fill)
        .size(W, H)
        .offset(if outgoing { 7.0 } else { -7.0 }, 0.0)
        .anyview()
}

/// Positions the wedge at the bubble's bottom corner on the sender's
/// side: trailing for outgoing, leading for incoming.
fn bubble_tail_layer(outgoing: bool, fill: Color) -> AnyView {
    let wedge = bubble_tail(outgoing, fill);
    let row: AnyView = if outgoing {
        hstack((spacer(), wedge)).spacing(0.0).anyview()
    } else {
        hstack((wedge, spacer())).spacing(0.0).anyview()
    };
    vstack((spacer(), row)).spacing(0.0).anyview()
}

/// One Desktop service line ("X pinned a message", "joined the
/// group"): a centered muted caption on a subtle pill at ~the gap of a
/// bubble run — no bubble, no meta.
fn service_pill(line: Str) -> impl View {
    hstack((
        spacer(),
        text(line)
            .caption()
            .muted()
            .padding_with((2.0, 8.0))
            .background(RoundedRectangle::new(0.5).fill(SurfaceVariant)),
        spacer(),
    ))
    .padding_with((4.0, 0.0))
}

#[cfg(test)]
pub(crate) fn service_pill_for_test() -> AnyView {
    service_pill(Str::from("Alice pinned a message")).anyview()
}

#[cfg(test)]
pub(crate) fn reaction_strip_for_test(row: &MessageRow) -> AnyView {
    reaction_strip(row)
}

/// Standalone service row — every service event is its own List row.
#[allow(needless_anyview)] // AnyView is the concrete type: `message_bubble`'s
// tail returns AnyView, so `-> impl View` would not unify the two sites.
fn service_line(row: &MessageRow) -> AnyView {
    service_pill(row.text.clone()).anyview()
}

/// The bubble's visual content. Extracted from `message_bubble` so the
/// context menu can mount a second copy as its lifted `preview`
/// (waterui#1245 — `AnyView` isn't `Clone`).
fn bubble_view(store: &Store, row: &MessageRow) -> AnyView {
    let reply_excerpt = row.reply_excerpt.clone();
    let has_reply = !reply_excerpt.is_empty();
    let body_text = row.text.clone();
    let body_styled = row.styled.clone();
    let has_styled = !body_styled.is_empty();
    let link_site = row.link_site.clone();
    let link_title = row.link_title.clone();
    let link_desc = row.link_desc.clone();
    let has_link = !link_site.is_empty() || !link_title.is_empty();
    let fwd = row.forwarded_from.clone();
    let has_fwd = !fwd.is_empty();
    let has_text = !body_text.is_empty();
    let chips = row.reaction_chips.clone();
    let has_reactions = !chips.is_empty();
    let time = row.time.clone();
    // Signal-derived cap — Telegram Desktop's ~72%-of-pane rule with its
    // absolute 480dp ceiling. Honest repro: `Frame::max_width` samples a
    // signal only at mount (waterui#1214), so the cap is
    // whatever the mount-time pane width produces; no wrap inside the cap
    // on hydrolysis yet (hydrolysis#130). No workaround applied.
    let bubble_cap = store
        .win_frame
        .map(|f| ((f.width() - 340.0) * 0.72).clamp(220.0, 480.0));
    let media = media_slot(store, row, bubble_cap.clone());
    let sender = row.sender.clone();
    let r_view = row.clone();

    // Secondary text inside the bubble: `MutedForeground` on a plain
    // background, a translucent on-accent color inside an outgoing bubble —
    // the fixed muted token is unreadable on the accent fill.
    let row_outgoing = row.outgoing;
    let muted_parts = move |v: AnyView| -> AnyView {
        if row_outgoing {
            v.foreground(WithOpacity::new(AccentForeground, 0.78))
                .anyview()
        } else {
            v.muted().anyview()
        }
    };
    let has_media = row.media_file != 0
        || row.play_file != 0
        || !row.media_label.is_empty()
        || !row.album_files.is_empty();
    // Emoji-only messages render large on no bubble fill (Telegram Desktop).
    let emoji_n = if has_media || has_link || has_styled || row.poll.is_some() {
        0
    } else {
        emoji_count(&body_text)
    };

    let mut parts: Vec<AnyView> = Vec::new();
    if has_fwd {
        // Desktop: the "Forwarded from X" badge opens the source
        // user/channel profile on tap (no-op for hidden senders).
        let (fu, fc) = (row.fwd_user, row.fwd_chat);
        let fwd_name = fwd
            .as_str()
            .strip_prefix("Forwarded from ")
            .unwrap_or(fwd.as_str())
            .to_string();
        parts.push(
            muted_parts(
                text(fwd.clone())
                    .italic(true)
                    .caption()
                    .line_limit(ONE)
                    .anyview(),
            )
            // hydrolysis#221 workaround: hide the leaf so the gesture, not the
            // leaf, emits the single named Button node (remove with the fix).
            .a11y_hidden(true)
            .on_tap(move |store: Store| store.open_peer(fu, fc, Str::from(fwd_name.clone())))
            .a11y_label(Str::from(format!(
                "Open profile of {}",
                fwd.as_str()
                    .strip_prefix("Forwarded from ")
                    .unwrap_or(fwd.as_str())
            )))
            .a11y_role(AccessibilityRole::Button)
            .anyview(),
        );
    }
    // Desktop orders the sender's name above the quoted reply inside a
    // reply bubble (r33-5). Tapping it opens the sender profile.
    if row.group_first && !row.outgoing && !sender.is_empty() {
        let (su, sc) = (row.sender_user, row.sender_chat);
        let sender_name = sender.to_string();
        parts.push(
            text(sender.clone())
                .caption()
                .bold()
                .foreground(peer_color(row.sender_accent, sender.as_str()))
                // hydrolysis#221 workaround: hide the leaf so the gesture,
                // not the leaf, emits the single named Button node.
                .a11y_hidden(true)
                .on_tap(move |store: Store| store.open_peer(su, sc, Str::from(sender_name.clone())))
                .a11y_label(Str::from(format!("Open profile of {sender}")))
                .a11y_role(AccessibilityRole::Button)
                .anyview(),
        );
    }
    if has_reply {
        // Desktop's quote block: accent bar + excerpt; tapping it jumps to
        // the replied message (scroll + flash-highlight via jump_to_message).
        let rid = row.reply_to_id;
        let bar_color = if row.outgoing {
            Color::from(AccentForeground)
        } else {
            Color::from(Accent)
        };
        parts.push(
            hstack((
                bar_color.width(2.0),
                muted_parts(
                    text(reply_excerpt.clone())
                        .caption()
                        .line_limit(TWO)
                        .anyview(),
                ),
            ))
            .spacing(6.0)
            .on_tap(move |store: Store| {
                if rid != 0 {
                    store.jump_to_message(rid);
                }
            })
            .a11y_role(AccessibilityRole::Button)
            .a11y_label("Jump to replied message")
            .anyview(),
        );
    }
    if has_media {
        parts.push(
            media
                .on_tap(move |store: Store| store.open_viewer(&r_view))
                .anyview(),
        );
    }
    if let Some(poll) = &row.poll {
        // Secondary poll text takes the same outgoing/incoming muted color
        // as every other bubble part — `MutedForeground` on the Accent
        // (lavender) fill is unreadable (r51 D7).
        parts.push(poll_block(row.id, poll, muted_parts).anyview());
    }
    if has_text {
        // `text_part` builds inside the `otherwise` closure — `AnyView` is
        // not `Clone`, and a `when`/`otherwise` builder is `Fn() -> V`.
        let make_text_part = {
            let store = store.clone();
            let row = row.clone();
            let body_text = body_text.clone();
            let body_styled = body_styled.clone();
            move || -> AnyView {
                if emoji_n > 0 {
                    let size = if emoji_n <= 3 {
                        44.0
                    } else if emoji_n <= 8 {
                        34.0
                    } else {
                        26.0
                    };
                    text(body_text.clone()).size(size).anyview()
                } else if row.search_hit && !row.search_styled.is_empty() {
                    // In-chat search match: the body renders with every
                    // occurrence highlighted via span `background`
                    // (Desktop parity; r35).
                    text(row.search_styled.clone()).body().anyview()
                } else if has_styled {
                    if row.has_spoiler {
                        // Desktop masks spoiler spans until tapped. The mask
                        // is a span `TextStyle.background` in the text color —
                        // hydrolysis drops per-span backgrounds until #207
                        // lands, so the spoiler text shows unmasked for now
                        // (r32-2).
                        let masked = store.spoiler_masked(row.id);
                        let masked_styled = body_styled.clone();
                        let open_styled = row.styled_open.clone();
                        let rid = row.id;
                        // Both variants stay mounted and flip `.visible` —
                        // under a `when()`'s Dynamic mount the mask branch
                        // disappears on reveal, taking its gesture region
                        // with it (r32-3: regions also sit ~15px below paint
                        // on virtualized rows).
                        let open = masked.not();
                        zstack((
                            text(masked_styled.clone()).body().visible(masked.clone()),
                            text(open_styled.clone()).body().visible(open.clone()),
                        ))
                        .on_tap(move |store: Store| store.reveal_spoiler(rid))
                        .a11y_role(AccessibilityRole::Button)
                        .a11y_label("Hidden text — tap to reveal")
                        .a11y_state_signal(
                            open.map(|shown| AccessibilityState::new().hidden(shown)),
                        )
                        .anyview()
                    } else {
                        text(body_styled.clone()).body().anyview()
                    }
                } else {
                    text(body_text.clone()).body().anyview()
                }
            }
        };
        // r39 translate: while `translated` holds an entry the body swaps
        // to it. Only the active branch is mounted — a `.visible`-flipped
        // zstack sibling keeps its measured width and the hidden branch
        // pushes the visible text off the sender line at narrow widths
        // (r51 D4). The caption restores the original.
        let rid = row.id;
        let tr_map = store.translated.clone();
        let has_tr = tr_map
            .map(move |m: std::collections::BTreeMap<i64, Str>| m.contains_key(&rid))
            .distinct();
        let tr_text = tr_map
            .map(move |m| m.get(&rid).cloned().unwrap_or_default())
            .computed();
        parts.push(
            when(has_tr.clone(), move || {
                vstack((
                    text(tr_text.clone()).body(),
                    text("Translated to English — show original")
                        .caption()
                        .foreground(Accent)
                        .on_tap(move |store: Store| store.untranslate(rid))
                        .a11y_role(AccessibilityRole::Button)
                        .a11y_label("Show original message"),
                ))
                .spacing(3.0)
                .leading()
            })
            .otherwise(make_text_part)
            .anyview(),
        );
    }
    for cb in &row.code_blocks {
        // Desktop: every `Pre`/`PreCode` block renders as a surface card
        // with the language in the header and a Copy affordance — the
        // framework's `Code` view ships exactly that shape; `on_copied`
        // surfaces our toast/clipboard binding.
        let store_c = store.clone();
        let text_c = cb.text.clone();
        let lang = Language::try_from(cb.lang.as_str()).unwrap_or(Language::Plaintext);
        let card = code(lang, cb.text.clone());
        let card = if cb.lang.is_empty() {
            card
        } else {
            card.info(cb.lang.clone())
        };
        parts.push(
            card.on_copied(move |_env| store_c.copy_field(text_c.as_str()))
                .anyview(),
        );
    }
    if has_link {
        // Desktop's link-preview card: accent bar + site name, title and
        // description, inside the bubble under the message text.
        let card_color = if row.outgoing {
            Color::from(AccentForeground)
        } else {
            Color::from(Accent)
        };
        let mut card: Vec<AnyView> = Vec::new();
        if !link_site.is_empty() {
            card.push(
                text(link_site.clone())
                    .caption()
                    .foreground(card_color.clone())
                    .anyview(),
            );
        }
        if !link_title.is_empty() {
            card.push(
                text(link_title.clone())
                    .body()
                    .bold()
                    .line_limit(TWO)
                    .anyview(),
            );
        }
        if !link_desc.is_empty() {
            card.push(muted_parts(
                text(link_desc.clone()).caption().line_limit(TWO).anyview(),
            ));
        }
        let card_row = hstack((card_color.width(2.0), vstack(card).spacing(1.0).leading()))
            .spacing(6.0)
            .anyview();
        // Desktop: the whole preview card is the link — tapping it opens
        // the URL in the system browser.
        if row.link_url.is_empty() {
            parts.push(card_row);
        } else {
            let url = row.link_url.clone();
            let site = link_site.clone();
            parts.push(
                card_row
                    .on_tap(move |store: Store| store.open_link(url.clone()))
                    .a11y_label(Str::from(format!("Open link {site}")))
                    .a11y_role(AccessibilityRole::Button)
                    .anyview(),
            );
        }
    }
    // Bot inline keyboard (`replyMarkupInlineKeyboard`): one pill row per
    // keyboard row under the content, inside the bubble like Desktop.
    if !row.kb_rows.is_empty() {
        let msg_id = row.id;
        let (fg, pill): (Color, Color) = if row.outgoing {
            (
                Color::from(AccentForeground),
                Color::from(WithOpacity::new(AccentForeground, 0.18)),
            )
        } else {
            (
                Color::from(Accent),
                Color::from(WithOpacity::new(Accent, 0.10)),
            )
        };
        let kb_stack: Vec<AnyView> = row
            .kb_rows
            .iter()
            .map(|cells| {
                let views: Vec<AnyView> = cells
                    .iter()
                    .map(|b| {
                        let kind = b.kind.clone();
                        hstack((
                            spacer(),
                            text(b.text.clone()).caption().bold().foreground(fg.clone()),
                            spacer(),
                        ))
                        .padding_with((5.0, 6.0))
                        .background(RoundedRectangle::new(0.45).fill(pill.clone()))
                        .on_tap(move |store: Store| store.inline_tap(msg_id, &kind))
                        .a11y_role(AccessibilityRole::Button)
                        .a11y_label(Str::from(format!("Button {}", b.text)))
                        .max_width(f32::INFINITY)
                        .anyview()
                    })
                    .collect();
                hstack(views).spacing(4.0).anyview()
            })
            .collect();
        parts.push(vstack(kb_stack).spacing(4.0).anyview());
    }
    // Reactions and the meta row overlay the bubble's bottom inset on two
    // separate lines: chips sit on the zone right under the content
    // (leading), the meta row on the band below (trailing) — the "time drops
    // below the reactions" arrangement, so they can never overlap at any
    // chip count (r24-1). A shared-line layout needs a Spacer in the band,
    // and a flexible member claims the whole main-axis offer in
    // `measure_stack`, pinning the bubble at its cap — that greedy layout is
    // what the overlays avoid (the bubble hugs max(content, chips, meta)).
    let meta_band = 20.0;
    let chips_band = if has_reactions { 26.0 } else { 0.0 };
    let content = vstack(parts).spacing(4.0).leading().padding_with([
        10.0,
        10.0 + meta_band + chips_band,
        10.0,
        10.0,
    ]);

    // Reaction strip: one pill component everywhere (r25-1). The
    // unchosen tint derives from the surface under the chip exactly as
    // Desktop derives it from the bubble colour; the chosen pill is
    // accented in every placement (the accent-bubble variant uses the
    // container tint so it stays readable on the accent fill).
    let chip_surface = if row.outgoing && !(emoji_n > 0 && !row.highlighted) {
        ChipSurface::Outgoing
    } else if emoji_n > 0 && !row.highlighted {
        ChipSurface::Page
    } else {
        ChipSurface::Incoming
    };
    let chip_views: Vec<AnyView> = chips
        .iter()
        .map(|c| reaction_chip(row, c, chip_surface))
        .collect();
    // Bottom-anchored with the meta band reserved beneath: the chips occupy
    // the zone between the content and the meta line.
    let reactions_overlay: AnyView = if has_reactions {
        hstack(chip_views)
            .spacing(4.0)
            .padding_with([0.0, 10.0 + meta_band, 10.0, 10.0])
            .anyview()
    } else {
        AnyView::default()
    };

    // Channel post footer: "👁 1.2K · signature" before the clock
    // (Telegram Desktop meta order).
    let footer = row.post_footer();
    let footer2 = footer.clone();
    let n_comments = row.comments;
    let post_id = row.id;
    let meta_overlay = hstack((
        // Channel post "💬 N comments" → the comments thread popup
        // (Desktop draws it as a separate footer line under the post).
        when(n_comments > 0, move || {
            text!("💬 {#n_comments} comments", n_comments = n_comments)
                .caption()
                .on_tap(move |store: Store| store.open_comments(post_id))
                .a11y_label("View comments")
                .a11y_role(AccessibilityRole::Button)
        }),
        when(!footer.is_empty(), move || text(footer2.clone()).caption()),
        when(row.edited, || text("edited").caption()),
        text(time).caption(),
        if row.pending {
            clock_outline().size(11.0, 11.0).anyview()
        } else if row.failed {
            let rid = row.id;
            alert_circle()
                .tint(Error)
                .size(11.0, 11.0)
                .on_tap(move |store: Store| store.resend_failed(rid))
                .a11y_label("Resend failed message")
                .a11y_role(AccessibilityRole::Button)
                .anyview()
        } else if row.outgoing && row.read_out {
            text("✓✓").caption().anyview()
        } else if row.outgoing {
            text("✓").caption().anyview()
        } else {
            AnyView::default()
        },
    ))
    .spacing(3.0)
    .padding_with([0.0, 8.0, 0.0, 8.0]);

    // Finite zstack layers: `ZStackLayout` reports the envelope of its
    // children, so the bubble hugs max(content, chips, meta) — BottomLeading
    // anchors the chips under the content, BottomTrailing the meta row.
    let bubble_inner = zstack((
        zstack((content, reactions_overlay)).alignment(BottomLeading),
        meta_overlay,
    ))
    .alignment(BottomTrailing);
    // The tail hooks a run's last bubble toward the sender's avatar /
    // the screen edge (Desktop); bubble-less emoji-only rows get none.
    // It is a layer of the bubble's background offset past the edge —
    // nothing is reserved in layout — and the joined corner is square
    // (r26-1, r26-2).
    let tail_on = row.group_last && !(emoji_n > 0 && !row.highlighted);
    let bubble_fill = if row.highlighted {
        Color::from(AccentContainer)
    } else if row.outgoing {
        Color::from(Accent)
    } else {
        Color::from(SurfaceVariant)
    };
    let bubble: AnyView = if emoji_n > 0 && !row.highlighted {
        // No fill: the emoji itself is the content (Telegram Desktop).
        Frame::new(bubble_inner)
            .max_width(bubble_cap.clone())
            .foreground(Foreground)
            .anyview()
    } else {
        let bg: AnyView = if tail_on {
            // The sender-side bottom corner is square where the tail
            // joins so fill and wedge read as one outline.
            let (bl, br) = if row.outgoing {
                (0.18, 0.0)
            } else {
                (0.0, 0.18)
            };
            zstack((
                UnevenRoundedRectangle::new(0.18, 0.18, bl, br).fill(bubble_fill.clone()),
                bubble_tail_layer(row.outgoing, bubble_fill),
            ))
            .anyview()
        } else {
            RoundedRectangle::new(0.18).fill(bubble_fill).anyview()
        };
        Frame::new(bubble_inner)
            .max_width(bubble_cap.clone())
            .background(bg)
            .foreground(if row.outgoing {
                Color::from(AccentForeground)
            } else {
                Color::from(Foreground)
            })
            .anyview()
    };

    bubble
}

/// The context menu's accessory (waterui#1245): Desktop's quick-reaction
/// strip — five emoji in a pill floating above the lifted bubble. A tap
/// applies the reaction and dismisses the menu through the
/// `DismissContextMenu` the accessory's environment carries. The chosen
/// reaction reads highlighted.
/// The bubble's context-menu items in Telegram Desktop order (r33-3):
/// Reply / Edit (own messages) / Copy text / Pin·Unpin / Forward /
/// Select / Delete — the last with `CommandRole::Destructive`. The
/// quick-reaction strip rides as the menu's `accessory` instead
/// (waterui#1245).
pub(crate) fn bubble_menu_items(row: &MessageRow, pinned: bool, linkable: bool) -> impl MenuView {
    let r1 = row.clone();
    let r2 = row.clone();
    let r3 = row.clone();
    let r4 = row.clone();
    let r5 = row.id;
    let rid = row.id;
    let r11 = row.id;
    let r_tr = row.clone();
    let r_link = row.clone();
    let r_img = row.clone();
    let r_info = row.clone();
    let r_saved = row.clone();
    let r_quote = row.clone();
    let r_tag = row.clone();
    let r_poll = row.id;
    let pin_label: &'static str = if pinned {
        "Unpin message"
    } else {
        "Pin message"
    };
    // Photo bubbles get Desktop's "Copy Image"; groups/channels get
    // "Copy Link"; incoming text gets "Translate".
    let is_photo = row.media_file != 0
        && row.play_file == 0
        && row.media_label.to_lowercase().starts_with("photo");
    let mut items: Vec<Command> = vec!["Reply".action(move |store: Store| store.start_reply(&r1))];
    // Desktop's "Quote": the message text lands in the composer as a `> `
    // blockquote draft (`parse_markdown` re-entities it at send).
    if !row.text.is_empty() {
        items.push("Quote".action(move |store: Store| store.quote_message(&r_quote)));
    }
    if row.outgoing {
        items.push("Edit".action(move |store: Store| store.start_edit(&r4)));
    }
    items.push(
        "Copy text"
            .action(move |store: Store| store.copy_message(&r3))
            // Ctrl+C — Desktop's copy accelerator.
            .shortcut(Shortcut::new('c').control()),
    );
    if !row.outgoing && !row.text.is_empty() {
        items.push("Translate".action(move |store: Store| store.translate_message(&r_tr)));
    }
    if linkable {
        items.push("Copy link".action(move |store: Store| store.copy_link(&r_link)));
    }
    if is_photo {
        items.push("Copy image".action(move |store: Store| store.copy_image(&r_img)));
    }
    items.push(pin_label.action(move |store: Store| {
        if pinned {
            store.unpin_message(rid)
        } else {
            store.pin_message(rid)
        }
    }));
    items.push("Forward".action(move |store: Store| store.start_forward(&r2)));
    // Desktop's message-menu quick action — forwards into the user's
    // own Saved Messages chat without leaving this one.
    items.push("Save to Saved Messages".action(move |store: Store| store.save_to_saved(&r_saved)));
    // Poll creator only — closing is the last action an open poll takes.
    if row.poll.as_ref().map(|p| !p.closed).unwrap_or(false) && row.outgoing {
        items.push("Stop poll".action(move |store: Store| store.stop_poll(r_poll)));
    }
    items.push("Select".action(move |store: Store| store.toggle_select(r11)));
    // Desktop's "Info" on outgoing messages: read time, views and the
    // seen-by list (getMessageReadDate / getMessageViewers).
    if row.outgoing {
        items.push("Info".action(move |store: Store| store.open_msg_info(&r_info)));
    }
    // Desktop scopes `#tag` to the in-chat search. Inline-entity taps are
    // a framework gap (water-rs/waterui#1352) — the menu is the honest route.
    if !row.hashtag.is_empty() {
        items.push(
            text!("Search {hashtag}", hashtag = row.hashtag)
                .action(move |store: Store| store.search_hashtag(&r_tag)),
        );
    }
    (
        items,
        Divider,
        "Delete"
            .command()
            .action(move |store: Store| store.delete_message(r5))
            .destructive(),
    )
}

fn reaction_strip(row: &MessageRow) -> AnyView {
    let strip: Vec<AnyView> = ["👍", "❤️", "🔥", "😂", "😮"]
        .iter()
        .map(|e| {
            let r = row.clone();
            let e = *e;
            let tint = if row.my_reaction == e {
                Color::from(AccentContainer)
            } else {
                Color::from(WithOpacity::new(SurfaceVariant, 0.0))
            };
            text(e)
                .size(18.0)
                .padding_with((3.0, 6.0))
                .background(Circle.fill(tint))
                .on_tap(move |store: Store, Use(dismiss): Use<DismissContextMenu>| {
                    store.toggle_reaction(&r, e);
                    dismiss.dismiss();
                })
                .a11y_role(AccessibilityRole::Button)
                .a11y_label(Str::from(format!("React {e}")))
                .anyview()
        })
        .collect();
    hstack(strip)
        .spacing(2.0)
        .padding_with((3.0, 4.0))
        .background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
        .anyview()
}

#[allow(needless_anyview)] // AnyView is the concrete type: the tail's
// `vstack(column)` must be AnyView to unify with `service_line` above.
pub(crate) fn message_bubble(store: Store, row: MessageRow) -> impl View {
    if row.is_service {
        return service_line(&row);
    }
    let pinned = store.pinned_id.get() == row.id;
    // waterui#1245: the menu lifts a second copy of the bubble as `preview`
    // and floats the quick-reaction strip above it as `accessory`; a tap
    // there applies the reaction and dismisses via `DismissContextMenu`.
    // `Delete` carries `CommandRole::Destructive`. On Linux the lifted
    // presentation is still in progress (hydrolysis#200), so the backend
    // shows a plain popup for now.
    // r35: hover reveals Desktop's quick-reply button beside the bubble,
    // and a double-tap on the bubble applies the quick ❤️ reaction.
    let hov = Binding::bool(false);
    let r_quick = row.clone();
    let r_dbl = row.clone();
    let r_sel = row.clone();
    let quick_reply = when(hov.clone(), move || {
        let rr = r_quick.clone();
        reply()
            .tint(MutedForeground)
            .size(18.0, 18.0)
            .padding_with(6.0)
            .background(Circle.fill(SurfaceVariant))
            // hydrolysis#221 workaround: hide the leaf so the gesture, not
            // the leaf, emits the single named Button node.
            .a11y_hidden(true)
            .on_tap(move |store: Store| store.start_reply(&rr))
            .a11y_label("Quick reply")
            .a11y_role(AccessibilityRole::Button)
            .anyview()
    });

    // "Copy Link" is group/channel-only (private chats have no t.me link).
    let linkable = store
        .chats
        .snapshot()
        .iter()
        .find(|c| c.id == store.open_chat.get())
        .map(|c| matches!(c.kind_icon.as_str(), "group" | "channel"))
        .unwrap_or(false);
    let bubble = bubble_view(&store, &row)
        .context_menu(
            ContextMenu::new(bubble_menu_items(&row, pinned, linkable))
                .preview(bubble_view(&store, &row))
                .accessory(reaction_strip(&row)),
        )
        .on_tap_gesture_count(2, move |store: Store| store.quick_react(&r_dbl))
        // A tap landing on the bubble is owned exclusively by the deepest
        // gesture group — the row-level select toggle never sees it
        // (hydrolysis gesture groups: one group owns a point; chained
        // gestures on this view share the bubble's group), so multi-select
        // needs its own tap here. It is inert until selection is active.
        .on_tap(move |store: Store| {
            if !r_sel.is_service && !store.selected_msgs.snapshot().is_empty() {
                store.toggle_select(r_sel.id);
            }
        })
        .anyview();

    let placed = if row.outgoing {
        // The button appears in the spacer zone — the bubble stays put.
        hstack((spacer(), quick_reply, bubble))
            .spacing(4.0)
            .anyview()
    } else if row.avatar_col {
        // Groups/channels reserve a leading avatar column on incoming rows
        // so a run's bubbles stay aligned; the avatar itself shows only on
        // the run's last row, bottom-aligned (Telegram Desktop).
        let slot: AnyView = if row.show_avatar {
            // Desktop: tapping a group sender's avatar opens their profile.
            let (su, sc) = (row.sender_user, row.sender_chat);
            let sender_name = row.sender.to_string();
            avatar(
                store.clone(),
                row.sender_photo,
                row.sender.as_str(),
                32.0,
                row.sender_accent,
            )
            .on_tap(move |store: Store| store.open_peer(su, sc, Str::from(sender_name.clone())))
            .a11y_label(Str::from(format!("Open profile of {}", row.sender)))
            .a11y_role(AccessibilityRole::Button)
            .anyview()
        } else {
            Color::from(Surface).size(32.0, 32.0).anyview()
        };
        hstack((
            vstack((spacer(), slot)).spacing(0.0),
            bubble,
            quick_reply,
            spacer(),
        ))
        .spacing(4.0)
        .anyview()
    } else {
        hstack((bubble, quick_reply, spacer()))
            .spacing(4.0)
            .anyview()
    };
    let placed = placed
        .on_hover_enter(|State(h): State<Binding<bool>>| h.set(true))
        .on_hover_exit(|State(h): State<Binding<bool>>| h.set(false))
        .state(&hov)
        .anyview();
    // Desktop's vertical order inside a row: day divider, unread
    // divider, then the bubble itself.
    let mut column: Vec<AnyView> = Vec::new();
    if row.day_header {
        column.push(
            zstack((
                Color::from(BorderColor).height(1.0),
                hstack((
                    spacer(),
                    text(row.day_label.clone())
                        .caption()
                        .muted()
                        .padding_with((0.0, 6.0))
                        .background(Surface),
                    spacer(),
                )),
            ))
            .padding_with((2.0, 4.0))
            .anyview(),
        );
    }
    if row.unread_divider {
        column.push(
            zstack((
                Color::from(BorderColor).height(1.0),
                hstack((
                    spacer(),
                    text(store.tr("UnreadMessages", 0, "Unread messages"))
                        .caption()
                        .bold()
                        .foreground(Accent)
                        .padding_with((0.0, 6.0))
                        .background(Surface),
                    spacer(),
                )),
            ))
            .padding_with((2.0, 4.0))
            .anyview(),
        );
    }
    column.push(placed);
    vstack(column).spacing(0.0).anyview()
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
#[allow(signal_get_in_view)] // the `has` gate rebuilds the view when the
// file lands; get() then reads the resolved path at rebuild time
pub(crate) fn media_slot(
    store: &Store,
    row: &MessageRow,
    bubble_cap: impl IntoComputed<f32>,
) -> impl View {
    let bubble_cap = bubble_cap.into_computed();
    if row.play_file != 0 {
        // Playable payload (video / voice / audio / animation). Shows the
        // inline player once the file is downloaded; before that, a
        // labelled progress row (thumbnails still render via the photo
        // branch below for kinds that carry one).
        let pfid = row.play_file;
        let tfid = row.media_file;
        let path_a = store.file_signal(pfid);
        let path_b = store.file_signal(pfid);
        let has = path_a.map(|p: Str| !p.is_empty()).distinct();
        let url = path_b.map(Url::from_file_path_str);
        let audio_only = matches!(row.media_label.as_str(), "Voice" | "Audio");
        let label_text = row.media_label.clone();
        let pct = store.file_progress_signal(pfid);
        let thumb = store.file_signal(tfid);
        let secs = row.media_secs;
        let media_col = vstack((when(has, move || {
            if audio_only {
                video_player(url.snapshot())
                    .max_height(56.0)
                    .max_width(320.0)
                    .anyview()
            } else {
                video_player(url.snapshot())
                    .max_height(240.0)
                    .max_width(320.0)
                    .clip(RoundedRectangle::new(0.12))
                    .anyview()
            }
        })
        .otherwise(move || {
            hstack((
                // `Url::from_file_path_str` panics on an empty string, so
                // the thumbnail Photo must not exist until the file path
                // resolves — gate it on the resolved path, not on `tfid`.
                when(thumb.map(|p: Str| !p.is_empty()).distinct(), {
                    let thumb_url = thumb.map(Url::from_file_path_str);
                    move || {
                        Photo::new(thumb_url.clone())
                            .max_width(96.0)
                            .max_height(96.0)
                            .clip(RoundedRectangle::new(0.12))
                    }
                })
                .otherwise(|| image_outline().tint(MutedForeground).size(14.0, 14.0)),
                text!(
                    "{label_text}{suffix}",
                    suffix = pct.map(|p: i32| {
                        if p > 0 && p < 100 {
                            Str::from(format!(" — {p}%"))
                        } else {
                            Str::from("")
                        }
                    })
                )
                .caption()
                .muted(),
            ))
            .padding_with((4.0, 8.0))
        }),));
        // m:ss duration badge on the media corner (Telegram Desktop shows
        // it over the video thumbnail, bottom-right).
        if secs > 0 {
            let badge = format!("{}:{:02}", secs / 60, secs % 60);
            zstack((
                media_col,
                text(badge)
                    .caption()
                    .bold()
                    .foreground(Srgb::from_hex("#FFFFFF"))
                    .padding_with((2.0, 6.0))
                    .background(
                        RoundedRectangle::new(0.5)
                            .fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.5)),
                    )
                    .padding_with((6.0, 6.0))
                    .a11y_hidden(true),
            ))
            .alignment(BottomTrailing)
            .anyview()
        } else {
            media_col.anyview()
        }
    } else if row.album_files.len() > 1 {
        // Incoming media album: the merged row renders every member's
        // media as a two-column grid (Desktop's album bubble).
        let mut grid: Vec<AnyView> = Vec::new();
        for pair in row.album_files.chunks(2) {
            let cells: Vec<AnyView> = pair
                .iter()
                .map(|&fid| {
                    // `Url::from_file_path_str` panics on an empty path, so
                    // the Photo only exists once the file resolves.
                    let path = store.file_signal(fid);
                    let has = path.map(|p: Str| !p.is_empty()).distinct();
                    let url = store.file_signal(fid).map(Url::from_file_path_str);
                    when(has, move || {
                        Photo::new(url.clone())
                            .max_width(240.0)
                            .max_height(180.0)
                            .clip(RoundedRectangle::new(0.08))
                    })
                    .otherwise(|| {
                        vstack((
                            spacer(),
                            hstack((
                                spacer(),
                                image_outline().tint(MutedForeground).size(24.0, 24.0),
                                spacer(),
                            )),
                            spacer(),
                        ))
                        .width(240.0)
                        .height(180.0)
                    })
                    .anyview()
                })
                .collect();
            grid.push(hstack(cells).spacing(2.0).anyview());
        }
        vstack(grid).spacing(2.0).anyview()
    } else if row.media_file != 0 {
        let fid = row.media_file;
        let path_a = store.file_signal(fid);
        let path_b = store.file_signal(fid);
        let has = path_a.map(|p: Str| !p.is_empty()).distinct();
        let url = path_b.map(Url::from_file_path_str);
        let label_text = row.media_label.clone();
        let pct = store.file_progress_signal(fid);
        let cap = bubble_cap.clone();
        let nat_w = row.media_w.max(1) as f32;
        let nat_h = row.media_h.max(1) as f32;
        when(has, move || {
            // `ReactiveImage` is non-resizable by default — without
            // `.resizable()` the photo paints at its natural pixel size
            // (r51 D5). The photo is `resizable + Fit` inside a frame sized
            // to the bubble cap from the payload's own pixels (like
            // Desktop's media layout), and `clip` corners it off.
            // `aspect_ratio` sizes against the proposed width rather than a
            // cap, so the capped width and matching height come from signals;
            // signal-driven `width/height` needs `Frame` (`View` takes f32
            // only).
            let disp_w = cap.map(move |c| nat_w.min(c));
            let disp_h = cap.map(move |c| nat_h * nat_w.min(c) / nat_w);
            Frame::new(
                Photo::new(url.clone())
                    .resizable()
                    .content_mode(ContentMode::Fit),
            )
            .width(disp_w)
            .height(disp_h)
            .clip(RoundedRectangle::new(0.12))
        })
        .otherwise(move || {
            hstack((
                image_outline().tint(MutedForeground).size(14.0, 14.0),
                text!(
                    "{label_text}{suffix}",
                    suffix = pct.map(|p: i32| {
                        if p > 0 && p < 100 {
                            Str::from(format!(" — {p}%"))
                        } else {
                            Str::from("")
                        }
                    })
                )
                .caption()
                .muted(),
            ))
            .padding_with((4.0, 8.0))
            .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
        })
        .anyview()
    } else if !row.media_label.is_empty() {
        hstack((
            file().tint(MutedForeground).size(14.0, 14.0),
            text(row.media_label.clone()).caption().muted(),
        ))
        .padding_with((4.0, 8.0))
        .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
        .anyview()
    } else {
        spacer().width(0.0).anyview()
    }
}

// ---------------------------------------------------------------------------
// Pushed routes
// ---------------------------------------------------------------------------

// LangRow fields are immutable row data — the active flag updates by
// replacing the pack list, not by mutating the row in place.
#[allow(collection_item_snapshot)]
pub(crate) fn settings_view(store: Store) -> NavigationView {
    store.start_notification_watchers();
    let me = store.me.clone();
    let dark = store.dark.clone();
    let note = store.profile_note.clone();

    let content = scroll(
        vstack((
            text("Account").caption().muted(),
            hstack((
                text("Name"),
                spacer(),
                text!("{name}", name = me.map(|m| m.name)).muted(),
            )),
            hstack((
                text("Username"),
                spacer(),
                text!("{username}", username = me.map(|m| m.username)).muted(),
            )),
            hstack((
                text("Phone"),
                spacer(),
                text!("+{phone}", phone = me.map(|m| m.phone)).muted(),
            )),
            hstack((
                text("User ID"),
                spacer(),
                text!("{uid}", uid = me.map(|m| m.id)).muted(),
            )),
            vstack((
                text("Edit profile").caption().muted(),
                // The M3 text field keeps Compose's 280 dp floor, so two fields
                // cannot share a 340 dp sidebar row — stack them vertically.
                field("First name", &store.edit_first),
                field("Last name", &store.edit_last),
                field("Bio", &store.edit_bio).prompt("a few words about you"),
                field("Username", &store.edit_username).prompt("username (no @)"),
                hstack((
                    when(note.map(|s| !s.as_str().is_empty()), move || {
                        text!("{profile_note}", profile_note = note.clone())
                            .caption()
                            .muted()
                    }),
                    spacer(),
                    button("Save").action(|store: Store| store.save_profile()),
                )),
                hstack((
                    FilePicker::open(
                        label("Choose photo").icon(image_outline()).icon_only(),
                        &store.avatar_pick,
                    ),
                    button("Set avatar").action(|store: Store| store.set_avatar()),
                ))
                .spacing(8.0),
            ))
            .spacing(10.0)
            .leading(),
            vstack((
                text(store.tr("PrivacySettings", 0, "Privacy"))
                    .caption()
                    .muted(),
                hstack((
                    text("Two-step verification"),
                    spacer(),
                    text!("{twofa}", twofa = store.twofa.clone()).muted(),
                ))
                .on_tap(|store: Store| store.open_twofa())
                .a11y_label("Two-step verification")
                .a11y_role(AccessibilityRole::Button),
                scroll(VStack::for_each(
                    SignalCollection::new(store.privacy_rows.clone()),
                    move |row: PrivacyRow| {
                        let k = row.key.clone();
                        let k2 = row.key.clone();
                        let k3 = row.key.clone();
                        let k4 = row.key.clone();
                        let k5 = row.key.clone();
                        hstack((
                            text(row.setting.clone()),
                            spacer(),
                            text(row.audience.clone()).muted(),
                        ))
                        .context_menu((
                            "Everyone".action(move |store: Store| {
                                store.set_privacy_audience(k.clone(), "Everyone")
                            }),
                            "My contacts".action(move |store: Store| {
                                store.set_privacy_audience(k2.clone(), "My contacts")
                            }),
                            "Nobody".action(move |store: Store| {
                                store.set_privacy_audience(k3.clone(), "Nobody")
                            }),
                            "Always allow a person…".action(move |store: Store| {
                                store.open_privacy_exception(k4.clone(), true)
                            }),
                            "Never allow a person…".action(move |store: Store| {
                                store.open_privacy_exception(k5.clone(), false)
                            }),
                        ))
                    },
                )),
                vstack((
                    text(store.tr("Notifications", 0, "Notifications"))
                        .caption()
                        .muted(),
                    toggle("Private chats", &store.notif_private),
                    toggle("Groups", &store.notif_groups),
                    toggle("Channels", &store.notif_channels),
                ))
                .spacing(4.0)
                .leading(),
                hstack((
                    text(store.tr("SessionsTitle", 0, "Active sessions"))
                        .caption()
                        .muted(),
                    spacer(),
                    button(Label::new("Terminate other sessions", || {
                        text("Terminate other sessions").caption().foreground(Error)
                    }))
                    .style(ButtonStyle::Plain)
                    .action(|store: Store| store.terminate_all_sessions()),
                )),
                scroll(VStack::for_each(
                    SignalCollection::new(store.sessions.clone()),
                    move |row: SessionRow| {
                        let is_current = row.current;
                        hstack((
                            vstack((
                                text(row.title.clone()).bold(),
                                text(row.subtitle.clone()).muted(),
                            ))
                            .spacing(2.0)
                            .leading(),
                            spacer(),
                            when(is_current, || text("current").foreground(Accent)),
                        ))
                        .padding_with((4.0, 0.0))
                        .context_menu(("Terminate"
                            .action(move |store: Store| store.terminate_session_by_id(row.id)),))
                    },
                )),
                vstack((
                    text(store.tr("BlockedUsers", 0, "Blocked users"))
                        .caption()
                        .muted(),
                    scroll(VStack::for_each(
                        SignalCollection::new(store.blocked.clone()),
                        |row: MemberRow| {
                            hstack((
                                text(row.name.clone()),
                                spacer(),
                                text("Unblock").foreground(Accent),
                            ))
                            .padding_with((4.0, 0.0))
                            .on_tap({
                                let row = row.clone();
                                move |store: Store| store.unblock_sender(&row)
                            })
                            .a11y_label(Str::from(format!("Unblock {}", row.name)))
                            .a11y_role(AccessibilityRole::Button)
                        },
                    )),
                ))
                .spacing(4.0)
                .leading(),
            ))
            .spacing(10.0)
            .leading(),
            vstack((
                text(store.tr("StorageUsage", 0, "Storage"))
                    .caption()
                    .muted(),
                hstack((
                    text!("{storage}", storage = store.storage_summary.clone()).muted(),
                    spacer(),
                    button("Refresh").action(|store: Store| store.load_storage()),
                )),
                hstack((
                    delete_sweep()
                        .tint(Error)
                        .size(14.0, 14.0)
                        .a11y_hidden(true),
                    button(Label::new("Clear cached media", || {
                        text("Clear cached media").caption().foreground(Error)
                    }))
                    .style(ButtonStyle::Plain)
                    .action(|store: Store| store.clear_storage()),
                    spacer(),
                )),
            ))
            .spacing(8.0)
            .leading(),
            // Language: packs from getLocalizationTargetInfo; tapping applies
            // setOption language_pack_id and refetches the pack's strings.
            vstack((
                text(store.tr("Language", 0, "Language")).caption().muted(),
                scroll(VStack::for_each(
                    SignalCollection::new(store.lang_packs.clone()),
                    |row: LangRow| {
                        let id = row.id.clone();
                        hstack((
                            text(row.name.clone()),
                            when(row.beta, || text("beta").caption().muted()),
                            spacer(),
                            when(row.active, || {
                                check().tint(Accent).size(16.0, 16.0).a11y_hidden(true)
                            }),
                        ))
                        .padding_with((4.0, 0.0))
                        .on_tap(move |store: Store| store.apply_language(&id))
                        .a11y_label(Str::from(format!("Use {}", row.name)))
                        .a11y_role(AccessibilityRole::Button)
                    },
                )),
            ))
            .spacing(4.0)
            .leading(),
            text(store.tr("Appearance", 0, "Appearance"))
                .caption()
                .muted(),
            toggle("Dark mode", &dark),
            // Telegram Desktop's wallpaper picker — a row of named presets the
            // chat pane paints behind the bubbles (`chat_bg_color`).
            {
                let presets: [(&'static str, &'static str); 5] = [
                    ("", "Default"),
                    ("sky", "Sky"),
                    ("sand", "Sand"),
                    ("dusk", "Dusk"),
                    ("night", "Night"),
                ];
                hstack(
                    presets
                        .iter()
                        .map(|(key, label)| {
                            let key = *key;
                            let active = store.chat_bg.map(move |k: Str| k == key).distinct();
                            vstack((
                                RoundedRectangle::new(0.25)
                                    .fill(match key {
                                        "sky" => Color::srgb_hex("#9CC3E8"),
                                        "sand" => Color::srgb_hex("#D8C8A4"),
                                        "dusk" => Color::srgb_hex("#44506E"),
                                        "night" => Color::srgb_hex("#1B2634"),
                                        _ => Color::from(SurfaceVariant),
                                    })
                                    .size(44.0, 30.0),
                                hstack((
                                    text(*label).caption(),
                                    when(active, || {
                                        // hydrolysis#221: decorative glyph —
                                        // the parent carries the a11y label.
                                        check().tint(Accent).size(12.0, 12.0).a11y_hidden(true)
                                    }),
                                ))
                                .spacing(2.0),
                            ))
                            .spacing(3.0)
                            .on_tap(move |store: Store| store.chat_bg.set_from(key))
                            .a11y_label(Str::from(format!("{label} chat background")))
                            .a11y_role(AccessibilityRole::Button)
                        })
                        .collect::<Vec<_>>(),
                )
                .spacing(10.0)
            },
            text(store.tr("SessionsTitle", 0, "Session"))
                .caption()
                .muted(),
            button(store.tr("LogOut", 0, "Log out")).action(|store: Store| store.logout()),
        ))
        .spacing(10.0)
        .leading()
        .padding_with((12.0, 16.0)),
    )
    .task({
        let store = store.clone();
        async move {
            store.load_sessions();
            store.load_twofa();
            store.load_storage();
            store.load_privacy();
            store.load_blocked();
            store.load_language_packs();
        }
    });
    let store_for_twofa = store.clone();
    let store_for_picker = store.clone();
    let content = vstack((
        content,
        when(store.twofa_open.clone(), move || {
            twofa_sheet(store_for_twofa.clone())
        }),
        when(store.privacy_picker_open.clone(), move || {
            privacy_picker_view(store_for_picker.clone())
        }),
    ));
    NavigationView::new("Settings", content)
}

/// Two-step verification sheet: current+new password, hint, recovery email.
fn twofa_sheet(store: Store) -> impl View {
    let note = store.twofa_note.clone();
    vstack((
        hstack((
            text("Two-step verification").headline(),
            spacer(),
            icon_button(close(), "Close", |store: Store| store.twofa_open.set(false)),
        )),
        SecureField::new("Current password", &store.twofa_old).hide_label(),
        SecureField::new("New password (empty disables)", &store.twofa_new).hide_label(),
        field("Password hint", &store.twofa_hint_in)
            .prompt("Hint")
            .hide_label(),
        field("Recovery email", &store.twofa_email)
            .prompt("Recovery email (optional)")
            .hide_label(),
        when(note.map(|s: Str| !s.is_empty()).distinct(), move || {
            text(note.clone()).caption().muted()
        }),
        hstack((
            button("Save").action(|store: Store| store.save_twofa()),
            spacer(),
        )),
    ))
    .spacing(8.0)
    .padding_with((10.0, 16.0))
    .background(Surface)
}

/// Contact picker for a per-user privacy exception.
fn privacy_picker_view(store: Store) -> impl View {
    vstack((
        hstack((
            text("Pick a contact").headline(),
            spacer(),
            icon_button(close(), "Close", |store: Store| {
                store.privacy_picker_open.set(false)
            }),
        )),
        scroll(VStack::for_each(
            SignalCollection::new(store.contacts.clone()),
            |row: MemberRow| {
                let uid = row.key;
                hstack((text(row.name.clone()).caption(), spacer()))
                    .padding_with((4.0, 0.0))
                    .on_tap(move |store: Store| store.pick_privacy_exception(uid))
            },
        )),
    ))
    .spacing(6.0)
    .padding_with((10.0, 16.0))
    .background(Surface)
}

pub(crate) fn profile_view(store: Store) -> NavigationView {
    let card = store.profile.clone();
    let name = card.map(|c| c.as_ref().map(|c| c.name.clone()).unwrap_or_default());
    let username = card.map(|c| c.as_ref().map(|c| c.username.clone()).unwrap_or_default());
    let phone = card.map(|c| c.as_ref().map(|c| c.phone.clone()).unwrap_or_default());
    let bio = card.map(|c| c.as_ref().map(|c| c.bio.clone()).unwrap_or_default());
    let online = card.map(|c| c.as_ref().map(|c| c.online).unwrap_or(false));
    let uid = card.map(|c| c.as_ref().map(|c| c.user_id).unwrap_or(0));
    let not_contact = card.map(|c| c.as_ref().map(|c| !c.is_contact).unwrap_or(false));

    // Desktop parity: username/phone/bio fields are tap-to-copy.
    let u_copy = username.clone();
    let p_copy = phone.clone();
    let b_copy = bio.clone();
    let content = scroll(
        vstack((
            text!("{name}", name = name.clone()).headline(),
            text!("{username}", username = username.clone())
                .caption()
                .muted()
                .on_tap(move |store: Store| {
                    let v = u_copy.snapshot().to_string();
                    if !v.is_empty() {
                        store.copy_field(&v);
                    }
                })
                .a11y_role(AccessibilityRole::Button),
            when(online.distinct(), || {
                text("online").caption().foreground(Accent)
            }),
            hstack((
                text("Phone"),
                spacer(),
                text!("{phone}", phone = phone.clone())
                    .muted()
                    .on_tap(move |store: Store| {
                        let v = p_copy.snapshot().to_string();
                        if !v.is_empty() {
                            store.copy_field(&v);
                        }
                    })
                    .a11y_role(AccessibilityRole::Button),
            )),
            text!("{bio}", bio = bio.clone())
                .body()
                .muted()
                .on_tap(move |store: Store| {
                    let v = b_copy.snapshot().to_string();
                    if !v.is_empty() {
                        store.copy_field(&v);
                    }
                })
                .a11y_role(AccessibilityRole::Button),
            hstack((
                spacer(),
                when(not_contact.distinct(), || {
                    button("Add to contacts").action(|store: Store| store.profile_add_contact())
                }),
                button("Message").action(move |store: Store| {
                    let id = uid.snapshot();
                    if id != 0 {
                        store.nav.pop();
                        store.start_chat_with(id);
                    }
                }),
            ))
            .spacing(8.0),
        ))
        .spacing(10.0)
        .padding_with((12.0, 16.0)),
    );
    NavigationView::new("Profile", content)
}

pub(crate) fn new_chat_view(store: Store) -> NavigationView {
    let kind = store.new_chat_kind.clone();
    let input = store.new_chat_input.clone();
    let store_nc = store.clone();
    let items: Vec<PickerItem<i32>> = vec![
        text("Private chat").tag(0),
        text("Group").tag(1),
        text("Channel").tag(2),
    ];
    let hint = kind.map(|k| {
        Str::from(match k {
            1 => "Group name",
            2 => "Channel name",
            _ => "@username or numeric user id",
        })
    });

    let content = scroll(
        vstack((
            text("Type").caption().muted(),
            Picker::new("Chat type", items.clone(), &kind).hide_label(),
            text("Details").caption().muted(),
            field("Name", &input).prompt("Name").hide_label(),
            hstack((
                text!("{hint}").caption().muted(),
                spacer(),
                button("Create").action(|store: Store| store.create_chat()),
            )),
            hstack((
                text("Contacts").caption().muted(),
                spacer(),
                icon_button(account_plus(), "Add contact", |store: Store| {
                    store.add_contact_open.toggle()
                }),
            )),
            when(store.add_contact_open.clone(), move || {
                vstack((
                    field("Phone", &store_nc.nc_phone)
                        .prompt("+1234567890")
                        .hide_label(),
                    hstack((
                        field("First name", &store_nc.nc_first).hide_label(),
                        field("Last name", &store_nc.nc_last).hide_label(),
                    ))
                    .spacing(8.0),
                    hstack((
                        spacer(),
                        button("Add contact").action(|store: Store| store.add_contact()),
                    )),
                ))
                .spacing(8.0)
                .leading()
            }),
            scroll(VStack::for_each(
                SignalCollection::new(store.contacts.clone()),
                move |row: MemberRow| {
                    hstack((
                        text(row.name.clone()).caption(),
                        spacer(),
                        text(row.status.clone()).caption().muted(),
                    ))
                    .spacing(6.0)
                    .padding_with((4.0, 0.0))
                    .on_tap(move |store: Store| store.start_chat_with(row.key))
                    .context_menu((
                        "New secret chat"
                            .action(move |store: Store| store.new_secret_chat(row.key)),
                        "Remove contact".action(move |store: Store| store.remove_contact(row.key)),
                    ))
                },
            )),
        ))
        .spacing(10.0)
        .padding_with((12.0, 16.0)),
    )
    .task({
        let store = store.clone();
        async move {
            store.load_contacts();
        }
    });
    NavigationView::new("New chat", content)
}

/// Video-note sheet: GPU camera preview + record/stop/send controls.
fn video_note_sheet(store: Store) -> impl View {
    let shared = store
        .video_shared
        .borrow()
        .clone()
        .expect("video-note sheet mounted without shared state — open_video_note installs it before video_note_open");
    let (status, inner, event_rx) = {
        let mut sh = shared.borrow_mut();
        let (inner, event_rx) = sh.take_gpu_session();
        (sh.status.clone(), inner, event_rx)
    };
    let rec_flag = store.video_recording.clone();
    let rec_label = store.video_elapsed.clone();
    vstack((
        hstack((
            text("Video note").headline(),
            spacer(),
            icon_button(close(), "Close", |store: Store| store.close_video_note()),
        )),
        {
            let status_drain = status.clone();
            GpuContentView::new(VideoNoteGpu::new(inner))
                .on_frame(move || {
                    while let Ok(crate::capture::VideoNoteEvent::Status(text)) = event_rx.try_recv()
                    {
                        status_drain.set_from(Str::from(text));
                    }
                })
                .size(240.0, 240.0)
                .background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
        },
        when(status.map(|s: Str| !s.is_empty()).distinct(), move || {
            text(status.clone()).caption().muted()
        }),
        when(rec_flag, move || {
            text!("Recording {rec_label}").caption().foreground(Accent)
        }),
        hstack((
            icon_button(camera(), "Start recording", |store: Store| {
                store.start_video_record()
            }),
            icon_button(send(), "Send video note", |store: Store| {
                store.finish_video_record()
            }),
        ))
        .spacing(12.0),
    ))
    .padding_with((10.0, 12.0))
    .background(Surface)
    .with(modal_escape(store.clone(), |s| s.close_video_note()))
}

/// Composer poll creator (attach menu → Poll, Telegram Desktop parity):
/// question, 2–`POLL_MAX_OPTIONS` option slots, anonymous / multiple /
/// quiz toggles; quiz mode adds a per-row correct-answer marker.
pub(crate) fn poll_creator(store: Store) -> impl View {
    let question_b = store.poll_question.clone();
    let anon_b = store.poll_anonymous.clone();
    let multi_b = store.poll_multiple.clone();
    let quiz_b = store.poll_quiz.clone();
    let regular = store.poll_quiz.not().distinct();
    let mut option_rows: Vec<AnyView> = Vec::new();
    for i in 0..Store::POLL_MAX_OPTIONS {
        let shown = store
            .poll_option_count
            .map(move |n: usize| i < n)
            .distinct();
        let quiz_correct = store
            .poll_correct
            .zip(&store.poll_quiz)
            .map(move |(c, q)| q && c == i)
            .distinct();
        let quiz_empty = store
            .poll_correct
            .zip(&store.poll_quiz)
            .map(move |(c, q)| q && c != i)
            .distinct();
        let can_remove = store.poll_option_count.gt(2).distinct();
        let field_b = store.poll_option_fields[i].clone();
        option_rows.push(
            when(shown, move || {
                let qc = quiz_correct.clone();
                let qe = quiz_empty.clone();
                let rm = can_remove.clone();
                let fb = field_b.clone();
                hstack((
                    when(qc, move || {
                        text("●")
                            .caption()
                            .foreground(Accent)
                            .on_tap(move |s: Store| s.poll_correct.set(i))
                            .a11y_label(Str::from(format!("Mark option {} correct", i + 1)))
                            .a11y_role(AccessibilityRole::Button)
                    }),
                    when(qe, move || {
                        text("○")
                            .caption()
                            .muted()
                            .on_tap(move |s: Store| s.poll_correct.set(i))
                            .a11y_label(Str::from(format!("Mark option {} correct", i + 1)))
                            .a11y_role(AccessibilityRole::Button)
                    }),
                    field(text!("Option {#index}", index = i + 1), &fb)
                        .prompt("Option")
                        .hide_label(),
                    when(rm, move || {
                        icon_button(close(), "Remove option", move |s: Store| {
                            s.remove_poll_option(i)
                        })
                    }),
                ))
                .spacing(6.0)
            })
            .anyview(),
        );
    }
    let option_stack: VStack<(Vec<AnyView>,)> = option_rows.into_iter().collect();
    vstack((
        hstack((
            poll().tint(Accent).size(16.0, 16.0).a11y_hidden(true),
            text("Create poll").caption().bold(),
            spacer(),
            icon_button(close(), "Close poll", |store: Store| {
                store.poll_open.set(false)
            }),
        ))
        .padding_with((4.0, 10.0)),
        field("Question", &question_b).prompt("Ask a question"),
        option_stack.spacing(4.0),
        hstack((
            {
                let add_label = store.tr("AddAnOption", 0, "+ Add an option");
                button(Label::new("Add an option", move || {
                    text(add_label.clone()).caption().foreground(Accent)
                }))
                .style(ButtonStyle::Plain)
                .action(|store: Store| store.add_poll_option())
            },
            spacer(),
        ))
        .padding_with((0.0, 12.0)),
        toggle(store.tr("PollAnonymous", 0, "Anonymous votes"), &anon_b).padding_with((0.0, 12.0)),
        when(regular, {
            let multi_label = store.tr("PollMultiple", 0, "Multiple answers");
            move || toggle(multi_label.clone(), &multi_b).padding_with((0.0, 12.0))
        }),
        toggle(store.tr("QuizMode", 0, "Quiz mode"), &quiz_b).padding_with((0.0, 12.0)),
        // Telegram actions: text buttons on the trailing edge, primary last.
        hstack((
            spacer(),
            button(store.tr("Cancel", 0, "Cancel"))
                .style(ButtonStyle::Plain)
                .action(|store: Store| store.poll_open.set(false)),
            button(store.tr("Create", 0, "Create")).action(|store: Store| store.send_poll()),
        ))
        .padding_with((0.0, 12.0)),
    ))
    .spacing(6.0)
    .padding_with((6.0, 0.0))
    .background(Surface)
    .with(modal_escape(store.clone(), |s| s.poll_open.set(false)))
}

/// Info panel below the dock threshold — Telegram Desktop's narrow-window
/// treatment: a full-height elevated surface pinned to the pane's trailing
/// edge, over a dimmed tap-to-dismiss scrim. Full height keeps the divider
/// reading as the panel's edge (it crosses the composer, as on Desktop) and
/// gives the shared-media scroll real room — a content-high panel leaves the
/// lazy grid with no visible rows.
pub(crate) fn info_overlay_chunk(store: Store) -> impl View {
    let esc = modal_escape(store.clone(), |s| s.info_open.set(false));
    Frame::new(
        zstack((
            Color::from(Foreground)
                .opacity(0.32)
                .on_tap(|store: Store| store.info_open.set(false)),
            Frame::new(zstack((
                Color::from(Surface),
                hstack((Color::from(BorderColor).width(1.0), info_panel(store))),
            )))
            .width(281.0)
            .max_height(f32::INFINITY)
            .shadow(Shadow::new(
                Color::srgb_hex("#000000").with_opacity(0.30),
                Vector::new(-2.0, 0.0),
                8.0,
                Rectangle,
            )),
        ))
        .alignment(TopTrailing),
    )
    .max_width(f32::INFINITY)
    .max_height(f32::INFINITY)
    .with(esc)
}

/// Right-side info panel (Desktop's wide-layout details panel): chat
/// profile header, members shortcut, shared-media grid.
pub(crate) fn info_panel(store: Store) -> impl View {
    let media_chunks = SignalCollection::new(store.shared_media.map(|v: Vec<SharedMediaRow>| {
        v.chunks(3)
            .enumerate()
            .map(|(ix, c)| MediaChunkRow {
                ix,
                cells: c.to_vec(),
            })
            .collect::<Vec<MediaChunkRow>>()
    }));
    let shared_tab_store = store.clone();
    let chat_id = store.selected.unwrap_or(0);
    let kind = store
        .chats
        .zip(&chat_id)
        .map(|(rows, id)| {
            rows.iter()
                .find(|r| r.id == id)
                .map(|r| r.kind_icon.clone())
                .unwrap_or_default()
        })
        .distinct();
    let title = store
        .chats
        .zip(&chat_id)
        .map(|(rows, id)| {
            rows.iter()
                .find(|r| r.id == id)
                .map(|r| r.title.clone())
                .unwrap_or_default()
        })
        .distinct();
    // Telegram Desktop: the member list appears only for groups/channels —
    // a private chat's panel shows no Members section.
    let is_group = kind
        .map(|k: Str| matches!(k.as_str(), "group" | "channel"))
        .distinct();
    let tr_store = store.clone();
    let members_label = kind.map(move |k: Str| {
        let key = if k.as_str() == "channel" {
            "Subscribers"
        } else {
            "Members"
        };
        tr_store.tr(key, 0, key)
    });
    let member_rows = store.clone();
    vstack((
        hstack((
            text!("{t}", t = title.clone()).headline().line_limit(ONE),
            spacer(),
            icon_button(close(), "Close info", |store: Store| {
                store.info_open.set(false)
            }),
        ))
        .padding_with((8.0, 14.0)),
        when(is_group, move || {
            vstack((
                hstack((
                    text!("{l}", l = members_label).caption().muted(),
                    spacer(),
                    text!("{n}", n = member_rows.members_count.clone())
                        .caption()
                        .muted(),
                ))
                .padding_with((4.0, 14.0)),
                {
                    let member_av = member_rows.clone();
                    scroll(VStack::for_each(
                        SignalCollection::new(member_rows.members.clone()),
                        move |row: MemberRow| {
                            let label_text = format!("Member {}", row.name);
                            let av_store = member_av.clone();
                            hstack((
                                button(Label::new(Str::from(label_text), move || {
                                    hstack((
                                        avatar(
                                            av_store.clone(),
                                            row.photo,
                                            &row.name,
                                            32.0,
                                            row.accent,
                                        ),
                                        vstack((
                                            text(row.name.clone()).caption(),
                                            text(row.status.clone())
                                                .caption()
                                                .line_limit(ONE)
                                                .muted(),
                                        ))
                                        .spacing(0.0)
                                        .leading(),
                                    ))
                                    .spacing(10.0)
                                    .padding_with((4.0, 14.0))
                                }))
                                .style(ButtonStyle::Plain)
                                .action(|store: Store| {
                                    store.members_open.set(true);
                                    store.load_members();
                                }),
                                spacer(),
                            ))
                        },
                    ))
                },
            ))
            .spacing(0.0)
        }),
        text("Shared").caption().muted().padding_with((8.0, 14.0)),
        // Media / Files / Links tabs (Desktop's shared-content section).
        {
            let tab = store.shared_tab.clone();
            picker(
                "Shared content",
                vec![
                    PickerItem::new(0usize, text("Media")),
                    PickerItem::new(1usize, text("Files")),
                    PickerItem::new(2usize, text("Links")),
                ],
                &tab,
            )
            .segmented()
            .padding_with((0.0, 14.0))
            .on_change(&tab, |t: usize, store: Store| store.load_shared_tab(t))
        },
        watch(store.shared_tab.clone(), move |tab: usize| {
            let st = shared_tab_store.clone();
            match tab {
                1 => scroll(VStack::for_each(
                    SignalCollection::new(st.shared_files.clone()),
                    |row: SharedLinkRow| {
                        hstack((
                            file()
                                .tint(MutedForeground)
                                .size(14.0, 14.0)
                                .a11y_hidden(true),
                            vstack((
                                text(row.title.clone()).caption().line_limit(ONE),
                                text(row.detail.clone()).caption().muted(),
                            ))
                            .spacing(0.0)
                            .leading(),
                            spacer(),
                        ))
                        .spacing(8.0)
                        .padding_with((4.0, 14.0))
                    },
                ))
                .anyview(),
                2 => scroll(VStack::for_each(
                    SignalCollection::new(st.shared_links.clone()),
                    |row: SharedLinkRow| {
                        hstack((
                            link_variant()
                                .tint(Accent)
                                .size(14.0, 14.0)
                                .a11y_hidden(true),
                            vstack((
                                text(row.title.clone()).caption().line_limit(ONE),
                                text(row.detail.clone()).caption().line_limit(ONE).muted(),
                            ))
                            .spacing(0.0)
                            .leading(),
                            spacer(),
                        ))
                        .spacing(8.0)
                        .padding_with((4.0, 14.0))
                    },
                ))
                .anyview(),
                _ => scroll(VStack::for_each(media_chunks.clone(), {
                    let cells = st.clone();
                    move |chunk: MediaChunkRow| {
                        let mut cells_v: Vec<AnyView> = Vec::new();
                        for cell in chunk.cells.iter().take(3) {
                            cells_v.push(media_cell(cells.clone(), cell.clone()).anyview());
                        }
                        hstack(cells_v).spacing(4.0)
                    }
                }))
                .anyview(),
            }
        }),
    ))
    .spacing(0.0)
    .leading()
    .width(280.0)
    .background(Surface)
}

/// One shared-media grid cell: the downloaded photo thumb, else the
/// emoji/kind fallback while `want_file_id` fetches it.
fn media_cell(store: Store, row: SharedMediaRow) -> impl View {
    let path_a = store.file_signal(row.file);
    let path_b = store.file_signal(row.file);
    let has = path_a.map(|p: Str| !p.is_empty()).distinct();
    let url = path_b.map(Url::from_file_path_str);
    when(has, move || {
        Photo::new(url.clone())
            .width(80.0)
            .height(80.0)
            .clip(RoundedRectangle::new(0.08))
    })
    .otherwise({
        let label = row.label.clone();
        move || {
            text(label.clone())
                .title()
                .width(80.0)
                .height(80.0)
                .background(SurfaceVariant)
        }
    })
}

/// Poll block inside a bubble: question, tappable options showing vote
/// share, and a totals footer. Tapping an option calls `setPollAnswer`.
#[allow(if_else_view)] // color pick, not a view
fn poll_block(
    message_id: i64,
    poll: &PollRow,
    muted_parts: impl Fn(AnyView) -> AnyView,
) -> impl View {
    let mut opts: Vec<AnyView> = Vec::new();
    for o in &poll.options {
        let ix = o.ix;
        let mark = if o.chosen { " ✓" } else { "" };
        let chosen = o.chosen;
        let bar_w = (o.pct.max(2) as f32) * 2.0;
        opts.push(
            vstack((
                hstack((
                    text(format!("{}{}", o.text, mark)).caption(),
                    spacer(),
                    muted_parts(text(format!("{}%", o.pct)).caption().anyview()),
                ))
                .spacing(6.0),
                RoundedRectangle::new(0.5)
                    .fill(if chosen {
                        Color::from(Accent)
                    } else {
                        Color::from(SurfaceVariant)
                    })
                    .size(bar_w, 4.0),
            ))
            .spacing(2.0)
            .leading()
            .on_tap(move |store: Store| store.vote_poll(message_id, ix))
            .a11y_label(Str::from(format!("Vote for {}", o.text)))
            .a11y_role(AccessibilityRole::Button)
            .anyview(),
        );
    }
    vstack((
        text(poll.question.clone()).body().bold(),
        vstack(opts).spacing(6.0),
        muted_parts(
            text(if poll.closed {
                format!("{} votes · closed", poll.voters)
            } else {
                format!("{} votes", poll.voters)
            })
            .caption()
            .anyview(),
        ),
    ))
    .spacing(6.0)
    .leading()
}

/// Desktop's delete-confirmation card: dimmed scrim + centered card with
/// the count, an "Also delete for <peer>" revoke checkbox on private
/// chats, and Cancel / Delete (destructive red).
fn delete_confirm_card(store: Store) -> impl View {
    let ask = store.confirm_delete.snapshot().unwrap_or(DeleteAsk {
        ids: Vec::new(),
        peer: "".into(),
    });
    let n = ask.ids.len();
    let has_peer = !ask.peer.is_empty();
    let peer_label = format!("Also delete for {}", ask.peer);
    let revoke = store.delete_revoke.clone();
    let esc = modal_escape(store.clone(), |s| s.dismiss_delete());
    zstack((
        // Scrim: taps outside the card dismiss it (Desktop parity).
        Rectangle
            .fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.45))
            .on_tap(|store: Store| store.dismiss_delete()),
        vstack((
            hstack((
                text(if n == 1 {
                    "Delete message?".into()
                } else {
                    format!("Delete {} messages?", n)
                })
                .body()
                .bold(),
                spacer(),
            )),
            if has_peer {
                toggle(peer_label, &revoke).anyview()
            } else {
                spacer().height(0.0).anyview()
            },
            hstack((
                spacer(),
                button("Cancel")
                    .action(|store: Store| store.dismiss_delete())
                    .a11y_label("Cancel"),
                button("Delete")
                    .action(|store: Store| store.confirm_delete_now())
                    .foreground(Error)
                    .a11y_label("Delete"),
            ))
            .spacing(8.0),
        ))
        .spacing(14.0)
        .padding_with(20.0)
        .background(Surface)
        .clip(RoundedRectangle::new(0.08))
        .max_width(300.0),
    ))
    .with(esc)
}

/// Channel comments thread popup — Desktop's "N Comments" footer opens
/// the linked discussion thread; ours lists `getMessageThreadHistory`
/// rows (demo corpus when offline).
fn comments_card(store: Store) -> impl View {
    let esc = modal_escape(store.clone(), |s| s.comments_open.set(None));
    let rows = SignalCollection::new(store.comments_list.clone());
    zstack((
        Rectangle
            .fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.45))
            .on_tap(|store: Store| store.comments_open.set(None)),
        vstack((
            hstack((
                text("Comments").body().bold(),
                spacer(),
                text("✕")
                    .body()
                    .padding_with((4.0, 8.0))
                    .on_tap(|store: Store| store.comments_open.set(None))
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_label("Close comments"),
            )),
            scroll(VStack::for_each(rows, |c: CommentRow| {
                vstack((text(c.sender).caption().bold(), text(c.text).body()))
                    .spacing(2.0)
                    .leading()
            }))
            .max_height(240.0),
        ))
        .spacing(10.0)
        .padding_with(16.0)
        .background(Surface)
        .clip(RoundedRectangle::new(0.08))
        .max_width(360.0),
    ))
    .with(esc)
}

/// In-pane media viewer (Desktop's viewer is fullscreen; ours covers the
/// detail pane): photo or playable payload with sender, caption, close.
#[allow(signal_get_in_view)] // `video_player` takes a constant MediaItem, so
// the video branch snapshots the resolved path each time the `watch` rebuilds.
fn viewer_layer(store: Store) -> impl View {
    // Desktop's media viewer: an opaque dark overlay carrying light text —
    // light header row (sender + white controls) and a light caption under
    // the media, nothing of the chat bleeds through.
    let viewer_fg = Color::srgb_hex("#FFFFFF");
    let viewer_scrim = Color::srgb_hex("#101010");
    // Row-driven text stays fine-grained: stepping between media only
    // re-evaluates these signals, not the overlay.
    let from_text = store
        .viewer
        .map(|v| v.map(|r| r.from).unwrap_or_default())
        .computed();
    let caption_text = store
        .viewer
        .map(|v| v.map(|r| r.caption).unwrap_or_default())
        .computed();
    // The media node itself is a different identity per message (a different
    // photo source / MediaItem), so the subtree is rebuilt per msg_id — the
    // scoped `watch` case, not a per-value one.
    let store_m = store.clone();
    let media = watch(
        store.viewer.map(|v| {
            v.map(|r| (r.msg_id, r.file, r.video))
                .unwrap_or((0, 0, false))
        }),
        move |(msg_id, file, video)| {
            let _ = msg_id;
            let has = store_m
                .file_signal(file)
                .map(|p: Str| !p.is_empty())
                .distinct();
            let url = store_m.file_signal(file).map(Url::from_file_path_str);
            let url_v = url.clone();
            when(has, move || {
                let url_p = url.clone();
                let url_v2 = url_v.clone();
                when(video, move || {
                    video_player(url_v2.snapshot()).max_height(420.0)
                })
                .otherwise(move || {
                    Photo::new(url_p.clone())
                        .max_width(430.0)
                        .max_height(430.0)
                        .clip(RoundedRectangle::new(0.05))
                })
            })
            .otherwise(|| {
                text("Downloading…")
                    .caption()
                    .foreground(Srgb::from_hex("#B0B0B0"))
            })
        },
    );
    // ‹ › step through the chat's media messages (Desktop's edge buttons;
    // ←/→ keys are handled by the `on_key_press` below). They sit as a
    // sibling zstack layer over the content column — inside the column as a
    // nested `when` they never armed (water-rs/hydrolysis#251). The
    // `when`-mounted layer occludes hits beneath it since
    // water-rs/hydrolysis#269.
    let nav_buttons = hstack((
        icon_button(
            chevron_left().tint(viewer_fg.clone()),
            "Previous media",
            |store: Store| {
                store.viewer_step(-1);
            },
        )
        .background(Circle.fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.45))),
        spacer(),
        icon_button(
            chevron_right().tint(viewer_fg.clone()),
            "Next media",
            |store: Store| {
                store.viewer_step(1);
            },
        )
        .background(Circle.fill(WithOpacity::new(Srgb::from_hex("#000000"), 0.45))),
    ))
    .padding_with((12.0, 12.0));
    zstack((
        vstack((
            hstack((
                text(from_text).body().bold().foreground(viewer_fg.clone()),
                spacer(),
                icon_button(
                    close().tint(viewer_fg.clone()),
                    "Close viewer",
                    |store: Store| store.close_viewer(),
                ),
            ))
            .spacing(8.0)
            .padding_with((8.0, 12.0))
            .background(viewer_scrim.clone()),
            spacer(),
            media,
            spacer(),
            text(caption_text)
                .caption()
                .line_limit(THREE)
                .foreground(viewer_fg.clone())
                .padding_with((8.0, 12.0)),
        ))
        .background(viewer_scrim),
        nav_buttons,
    ))
    .alignment(Center)
    // ←/→ step media like the ‹ › buttons. This sits on the overlay root so
    // keys bubble to it whether the focused node is in the content column or
    // the sibling nav-buttons layer.
    .on_key_press(|Use(press): Use<KeyPress>, store: Store| match press.key {
        Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowRight) => {
            if store.viewer_step(if press.key == Key::Named(NamedKey::ArrowLeft) {
                -1
            } else {
                1
            }) {
                KeyHandling::Handled
            } else {
                KeyHandling::Ignored
            }
        }
        _ => KeyHandling::Ignored,
    })
    .with(modal_escape(store.clone(), |s| s.close_viewer()))
}

/// Picker tab chip (Emoji / Stickers / GIFs) with the same metrics as the
/// folder chips so the row stays at chip height.
fn panel_tab_chip(label: &'static str, tab: i32) -> impl View {
    // A real `button`: gesture targets never reach `bind_interaction_target`,
    // so `text().on_tap` chips would not register the modal Escape scope or
    // keyboard focus for this panel.
    button(text(label).caption())
        .style(ButtonStyle::Plain)
        .action(move |store: Store| store.pick_panel_tab(tab))
        .a11y_label(label)
}

/// Emoji grid — the picker panel's first tab. Local data only (Desktop's
/// emoji tab is also local); tapping inserts into the composer.
fn emoji_grid() -> impl View {
    let mut rows: Vec<AnyView> = Vec::new();
    for chunk in EMOJI_SET.chunks(8) {
        let mut cells: Vec<AnyView> = Vec::new();
        for e in chunk {
            let s = (*e).to_string();
            let s_label = s.clone();
            cells.push(
                text(s.clone())
                    .headline()
                    .padding_with((4.0, 4.0))
                    .on_tap(move |store: Store| store.insert_emoji(&s))
                    .a11y_label(s_label)
                    .a11y_role(AccessibilityRole::Button)
                    .anyview(),
            );
        }
        rows.push(hstack(cells).spacing(2.0).anyview());
    }
    scroll(vstack(rows).spacing(2.0).leading().padding_with((4.0, 8.0))).max_height(160.0)
}

/// Common emoji, one grapheme each (VS16 sequences kept whole so chunking
/// can't split a variation selector from its base).
const EMOJI_SET: &[&str] = &[
    "😀", "😁", "😂", "🤣", "😃", "😄", "😅", "😆", "😉", "😊", "😋", "😎", "😍", "😘", "🥰", "😗",
    "😙", "😚", "🙂", "🤗", "🤔", "😐", "😑", "😶", "🙄", "😏", "😣", "😥", "😮", "🤐", "😯", "😪",
    "😫", "🥱", "😴", "😌", "😛", "😜", "😝", "🤤", "😒", "😓", "😔", "😕", "🙃", "🤑", "😲", "🙁",
    "😖", "😞", "😟", "😤", "😢", "😭", "😦", "😧", "😨", "😩", "🤯", "😬", "😰", "😱", "🥵", "🥶",
    "😳", "🤪", "😵", "🥴", "😠", "😡", "🤬", "😷", "🤒", "🤕", "🤢", "🤮", "😇", "🥳", "🥺", "🤠",
    "🤡", "🤥", "🤫", "🤭", "🧐", "🤓", "😈", "👿", "👹", "👺", "💀", "👻", "👽", "🤖", "💩", "😺",
    "😸", "😹", "😻", "😼", "😽", "🙀", "😿", "😾", "👍", "👎", "👏", "🙌", "👐", "🤝", "🙏", "✌️",
    "🤞", "🤟", "🤘", "🤙", "👌", "🤌", "🤏", "👈", "👉", "👆", "👇", "☝️", "✋", "🤚", "🖐", "🖖",
    "👋", "💪", "🖕", "✍️", "🤳", "💅", "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "🤎", "💔",
    "❣️", "💕", "💞", "💓", "💗", "💖", "💘", "💝", "🔥", "✨", "🌟", "⭐", "💯", "✅", "❌", "⚠️",
    "🎉", "🎊", "🎁", "🎈", "🎂", "🏆", "⚽", "🏀", "🎯", "🎮", "🎲", "🧩", "🚗", "✈️", "🚀", "🏠",
    "📱", "💻", "⌚", "📷", "🔒", "🔑", "💡", "📌", "✏️", "📝", "📖", "🔍", "💰", "☕", "🍕", "🍔",
    "🍎", "🍉", "🍺", "🥂", "⏰", "📅", "🌍", "🌙", "☀️", "🌈", "☔", "❄️", "⚡", "🐱", "🐶", "🐭",
    "🐹", "🐰", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🐦", "🦆",
    "🦅", "🦉", "🦇", "🐺", "🐗", "🐴", "🦄", "🐝", "🐛", "🦋", "🐌", "🐞", "🐜", "🐢", "🐍", "🦎",
    "🐙", "🦑", "🦐", "🦀", "🐡", "🐠", "🐟", "🐬", "🐳", "🐋", "🦈", "🐊", "🐅", "🐆", "🦓", "🦍",
    "🐘", "🦛", "🦏", "🐪", "🐫", "🦒", "🦘", "🐎", "🐖", "🐏", "🐑", "🦙", "🐐", "🦌", "🐕", "🐈",
];
