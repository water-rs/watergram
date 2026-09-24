//! View layer for Watergram.
//!
//! All views are pure declarations over [`Store`]; every handler takes
//! `store: Store` as a `#[state]` extractor injected once at the root.

use std::num::NonZeroUsize;
use std::time::Duration;


use waterui_backend_core::widget::ModalInteraction;
use waterui::component::list::{List, ListItem};
use waterui::layout::frame::Frame;
use waterui::graphics::GpuSurface;
use crate::capture::VideoNoteGpu;
use waterui::media::Photo;
use waterui::video::video_player;
use waterui::accessibility::AccessibilityRole;
use waterui::navigation::{
    ColumnWidth, NavigationSplitView, NavigationToolbar, NavigationToolbarItem,
    NavigationToolbarPlacement, NavigationView,
};
use waterui::prelude::*;
use waterui::reactive::collection::SignalCollection;
use waterui::text::IntoText;
use waterui::form::picker::file::FilePicker;
use waterui::shape::{Circle, RoundedRectangle, ShapeExt};
use waterui::graphics::color::{BorderColor, Srgb, WithOpacity};
use waterui::theme::color::{
    Accent, AccentContainer, AccentForeground, Error, Foreground,
    MutedForeground, Surface, SurfaceVariant,
};
use waterui::handler::SharedAction;
use waterui::widget::condition::when;

/// Escape-key dispatch for overlay layers: `ModalInteraction` marks a
/// subtree as a modal scope, so the backend sends Escape to the topmost
/// open overlay before any other handler (Telegram Desktop parity).
fn modal_escape(store: Store, close: fn(&Store)) -> ModalInteraction {
    ModalInteraction::new(
        true,
        SharedAction::new(move |_: Environment| close(&store)),
    )
}
use waterui_barcode::Barcode;
use tdlib_rs::enums;
use waterui_icons_material_icon as mdi;

use crate::state::{AccountRow, ChatRow, FolderRow, LangRow, MediaChunkRow, PackRow, MemberRow, MessageRow, PollRow, PrivacyRow, Route, Screen, SessionRow, SharedMediaRow, StickerItem, Store, ViewerRow};
use mdi::folder_plus;
use mdi::account_group;
use mdi::alert_circle;
use mdi::clock_outline;
use mdi::camera;
use mdi::check;
use mdi::microphone;
use mdi::file_gif_box;
use mdi::close;
use mdi::delete_sweep;
use mdi::account_plus;
use mdi::emoticon;
use mdi::link_variant;
use mdi::file;
use mdi::image_outline;
use mdi::information;
use mdi::lock;
use mdi::magnify;
use mdi::menu;
use mdi::paperclip;
use mdi::pin;
use mdi::poll;
use mdi::plus;
use mdi::send;
use mdi::share_variant;


const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const TWO: NonZeroUsize = NonZeroUsize::new(2).unwrap();
const THREE: NonZeroUsize = NonZeroUsize::new(3).unwrap();

// ---------------------------------------------------------------------------
// Root: switch on the top-level screen
// ---------------------------------------------------------------------------

pub fn root(store: Store) -> impl View {
    let inner = store.clone();
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
    .state(&store)
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
        text!("Code sent to {phone}").headline().foreground(Foreground),
        text("On the test servers the code equals the digits after 99966, e.g. 99966XXXXX → XXXXX.")
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
        button("Log in by phone instead")
            .action(|store: Store| store.screen.set(Screen::Phone)),
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
                        ({
                            let s = store.clone();
                            store.tr("NewChat", 0, "New chat").action(move || {
                                s.nav.push(Route::NewChat)
                            })
                        }, {
                            let s = store.clone();
                            store.tr("ArchivedChats", 0, "Archive").action(move || {
                                s.toggle_archive_view()
                            })
                        }, {
                            let s = store.clone();
                            store.tr("Accounts", 0, "Accounts").action(move || {
                                s.toggle_accounts()
                            })
                        }, {
                            let s = store.clone();
                            store
                                .tr("Settings", 0, "Settings")
                                .action(move || s.nav.push(Route::Settings))
                        }),
                    ),
                )),
            ),
    )
    .destination(
        move |route| match route {
            Route::Settings => settings_view(inner.clone()),
            Route::NewChat => new_chat_view(inner.clone()),
            Route::Profile => profile_view(inner.clone()),
        },
    )
}

#[allow(if_else_view)] // tab styling picks between two text looks, not reactive content
pub(crate) fn sidebar_view(store: Store) -> impl View {
    let conn = store.connection.clone();
    // Single-message forward (context menu) sets `forward_message`; the
    // multi-select bar sets `forward_ids`. The banner shows for either.
    let forward_mode = store
        .forward_message
        .is_some()
        .or(&store.forward_ids.map(|ids| !ids.is_empty()))
        .distinct();
    let filtered: SignalCollection<_> = SignalCollection::new(
        store
            .chats
            .zip(&store.search)
            .map(|(rows, q)| {
                let q = q.to_lowercase();
                if q.is_empty() {
                    rows
                } else {
                    rows.into_iter()
                        .filter(|r| {
                            r.title.to_lowercase().contains(&q)
                                || r.preview.to_lowercase().contains(&q)
                        })
                        .collect()
                }
            }),
    );
    let server_results = SignalCollection::new(store.server_results.clone());
    let show_results = store.server_results.map(|v| !v.is_empty()).distinct();
    let debounced = store.search.debounce(Duration::from_millis(400));
    let rows_store = store.clone();
    let res_store = store.clone();
    let folder_tabs = store
        .folders
        .zip(&store.active_folder)
        .map(|(fs, active)| {
            let mut tabs: Vec<FolderRow> = vec![FolderRow {
                id: 0,
                title: "All".into(),
                active: active == 0,
            }];
            tabs.extend(fs.iter().map(|f| FolderRow {
                id: f.id,
                title: f.title.clone(),
                active: f.id == active,
            }));
            tabs.push(FolderRow {
                id: -1,
                title: "Archive".into(),
                active: active == -1,
            });
            tabs
        });
    let has_folders = store.folders.map(|f| !f.is_empty()).distinct();

    vstack((
        // Connection state: Desktop only surfaces it while reconnecting —
        // the binding carries a label only for non-Ready states.
        when(conn.map(|c: Str| !c.as_str().is_empty()), move || {
            text!("{conn}")
                .caption()
                .muted()
                .padding_with((6.0, 12.0))
        }),
        // Collapsed `when` children would each still eat a vstack spacing
        // slot; keep the optional sections in one zero-spacing stack so a
        // hidden panel costs no gap.
        vstack((
        when(store.accounts_open.clone(), move || {
            let rows = store.accounts.clone();
            vstack((
                VStack::for_each(
                    SignalCollection::new(rows.clone()),
                    move |acc: AccountRow| {
                        let id = acc.id;
                        hstack((text(acc.label.clone()).caption(), spacer()))
                            .padding_with((4.0, 12.0))
                            .on_tap(move |store: Store| store.switch_account(id))
                    },
                ),
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
            hstack((
                share_variant().tint(Accent).size(16.0, 16.0),
                text("Select a chat to forward to")
                    .caption()
                    .foreground(Accent),
                field("Comment", &comment_b)
                    .prompt("Add a comment…")
                    .hide_label()
                    .width(150.0),
                spacer(),
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
                icon_button(close(), "Cancel", |store: Store| {
                    store.forward_message.set(None);
                    store.forward_ids.set(Vec::new());
                    store.forward_comment.set_from("");
                }),
            ))
            .padding_with((8.0, 12.0))
            .background(Surface)
        }),
        when(has_folders, move || {
            let folder_edit = store.folder_open.clone();
            // Chip metrics: caption line ~17dp + 2×3dp vertical chip padding.
            // ScrollView reports StretchAxis::Both unconditionally
            // (raw_view!, axis not consulted — see DOGFOOD), and View has no
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
                            let chip = if tab.active {
                                text(label)
                                    .caption()
                                    .bold()
                                    .foreground(Accent)
                                    .anyview()
                            } else {
                                text(label).caption().muted().anyview()
                            };
                            let chip = chip
                                .padding_with((chip_pad_v, 10.0_f32))
                                .background(if tab.active {
                                    RoundedRectangle::new(0.5)
                                        .fill(SurfaceVariant)
                                        .anyview()
                                } else {
                                    AnyView::default()
                                })
                                .on_tap(move |store: Store| store.set_list(id))
                                .a11y_label(a11y)
                                .a11y_role(AccessibilityRole::Button)
                                ;
                            if id > 0 {
                                chip.context_menu((
                                    "Edit folder".action(move |store: Store| {
                                        store.open_folder_editor(id)
                                    }),
                                    "Delete folder".action(
                                        move |store: Store| store.delete_folder(id),
                                    ),
                                ))
                                .anyview()
                            } else {
                                chip.anyview()
                            }
                        },
                    )),
                    // Same chip metrics as the tab chips so the row stays at
                    // chip height (the 40dp icon chip would stretch the row).
                    button(Label::new("New folder", move || {
                        folder_plus().foreground(Accent).size(14.0, 14.0)
                    }))
                    .style(ButtonStyle::Plain)
                    .action(|store: Store| store.open_folder_editor(0))
                    .padding_with((chip_pad_v, 8.0_f32)),
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
                        hstack((
                            spacer(),
                            button("Save folder").action(|store: Store| {
                                store.save_folder()
                            }),
                        )),
                    ))
                    .spacing(6.0)
                    .padding_with((4.0, 10.0))
                })
                }
            ))
        }),
        ))
        .spacing(0.0),
        // `List` reports StretchAxis::Both and fills the leftover region;
        // a `Lazy` stack inside `scroll(vstack)` reports None and is sized to
        // its realized rows, which clipped the list mid-pane (see DOGFOOD).
        {
            let filtered_else = filtered.clone();
            let rows_else = rows_store.clone();
            let list_sel = store.list_selection.clone();
            when(show_results, move || {
                let local_store = rows_store.clone();
                let remote_store = res_store.clone();
                vstack((
                    List::for_each(filtered.clone(), move |row: ChatRow| {
                        let id = row.id;
                        ListItem::new(
                            chat_row(local_store.clone(), row)
                                .on_tap(move |store: Store| store.select_chat(id)),
                        )
                    }),
                    Divider,
                    text("Global search results")
                        .caption()
                        .muted()
                        .padding_with((4.0, 12.0)),
                    List::for_each(server_results.clone(), move |row: ChatRow| {
                        let id = row.id;
                        ListItem::new(
                            chat_row(remote_store.clone(), row)
                                .on_tap(move |store: Store| store.select_chat(id)),
                        )
                    }),
                ))
                .leading()
            })
            .otherwise(move || {
                let rows_l = rows_else.clone();
                // Framework-owned selection (waterui#1233): pointer taps and
                // Up/Down write `list_selection`; `on_change` below routes it
                // through `select_chat`.
                List::for_each(filtered_else.clone(), move |row: ChatRow| {
                    ListItem::new(chat_row(rows_l.clone(), row))
                })
                .selection(&list_sel)
            })
        },
    ))
    .on_change(&debounced, |q: Str, store: Store| store.run_search(q))
    .on_change(&store.list_selection, |v: Option<i64>, store: Store| {
        if let Some(id) = v {
            store.select_chat(id)
        }
    })
}

// M3 icon button: an icon-only `Label` keeps `name` as the semantic identity
// (a11y) while rendering only the icon. Until water-rs/hydrolysis#115 gives
// icon-only buttons the 40dp icon-button box they keep the generic ~58-72dp
// minimum; measured widths are in DOGFOOD.
pub(crate) fn icon_button<F>(icon: impl View + Clone + 'static, name: &'static str, on_tap: F) -> impl View
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

pub(crate) fn avatar(store: Store, file_id: i32, title: &str, size: f32) -> impl View {
    let path_a = store.file_signal(file_id);
    let path_b = store.file_signal(file_id);
    let has = path_a.map(|p: Str| !p.is_empty()).distinct();
    let url = path_b.map(Url::from_file_path_str);
    let label = initials(title);
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
            .background(Circle.fill(Accent))
    })
    .a11y_hidden(true)
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn chat_row(store: Store, row: ChatRow) -> impl View {
    let id = row.id;
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
        _ => text(row.preview.clone())
            .caption()
            .line_limit(ONE)
            .muted()
            .anyview(),
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

    hstack((
        avatar(store, row.photo_file, &row.title, 44.0),
        vstack((
            hstack((
                kind_icon(&row.kind_icon),
                text(row.title.clone())
                    .body()
                    .line_limit(ONE)
                    .foreground(Foreground),
                spacer(),
                when(row.online, || text("●").caption().foreground(Accent)),
                text(row.time.clone()).caption().muted(),
            ))
            .spacing(4.0),
            hstack((
                preview_line,
                spacer(),
                mention_badge,
                badge,
            ))
            .spacing(4.0),
        ))
        .spacing(2.0)
        .leading(),
    ))
    .spacing(10.0)
    .padding_with((6.0, 10.0))
    .context_menu((
        "Mark read".action(move |store: Store| store.mark_read(id)),
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
        if row.muted { "Unmute" } else { "Mute" }.action(move |store: Store| store.toggle_mute(id)),
        "Join chat".action(move |store: Store| store.join(id)),
        "Clear history".action(move |store: Store| store.clear_history(id)),
        "Leave chat".action(move |store: Store| store.leave(id)),
    ))
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
    let has_capture_error = store
        .capture_error
        .map(|s: Str| !s.is_empty())
        .distinct();
    let capture_error_text = store.capture_error.clone();
    let composer_empty = store.composer.str_is_empty().distinct();
    let video_note_open = store.video_note_open.clone();
    let voice_elapsed = store.voice_elapsed.clone();
    let store_for_sheet = store.clone();
    let scroller = store.scroll.clone();
    let has_pinned = store.pinned_label.map(|s: Str| !s.is_empty()).distinct();
    let pinned_label = store.pinned_label.clone();
    let search_open = store.chat_search_open.clone();
    let search_b = store.chat_search.clone();
    let search_results_b = store.chat_search_results.clone();
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
    let sel_count = store
        .selected_msgs
        .map(|v: Vec<i64>| v.len())
        .distinct();
    let scheduled_open = store.scheduled_open.clone();
    let scheduled_b = store.scheduled.clone();
    let emoji_tab = store.panel_tab.is_zero().distinct();
    // @mention completion: trailing @token in the composer filters the
    // loaded member list; the popup sits above the composer row.
    let mention_sig = store
        .composer
        .zip(&store.members)
        .map(|(q, ms)| {
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
    let mention_show = mention_sig
        .map(|v: Vec<MemberRow>| !v.is_empty())
        .distinct();
    let poll_open = store.poll_open.clone();
    let store_for_poll = store.clone();
    // Modal Escape scopes, cloned before `store` moves into the `when`
    // closures below; the last-built active scope wins the key.
    let esc_members = modal_escape(store.clone(), |s| s.members_open.set(false));
    let esc_scheduled = modal_escape(store.clone(), |s| s.scheduled_open.set(false));
    let esc_search = modal_escape(store.clone(), |s| {
        s.chat_search_open.set(false);
        s.chat_search.set_from("");
        s.chat_search_results.set(Vec::new());
    });
    let esc_stickers = modal_escape(store.clone(), |s| s.stickers_open.set(false));
            vstack((
    when(has_pinned, move || {
        hstack((
            pin().tint(Accent).size(14.0, 14.0).a11y_hidden(true),
            text!("{pinned_label}")
                .caption()
                .line_limit(ONE)
                .foreground(Accent),
            spacer(),
        ))
        .padding_with((6.0, 12.0))
        .background(Surface)
        .on_tap(|store: Store| store.jump_to_message(store.pinned_id.get()))
    }),
    // Multi-select action bar (Desktop's "N selected" header state).
    when(sel_active, move || {
        hstack((
            text!("{n} selected", n = sel_count.clone())
                .caption()
                .bold(),
            spacer(),
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
            let check = sel_msgs_rows
                .map(move |v: Vec<i64>| v.contains(&rid))
                .distinct();
            ListItem::new(
                hstack((
                    when(sel_active_rows.clone(), move || {
                        let check = check.clone();
                        when(check, || text("☑").body().foreground(Accent))
                            .otherwise(|| text("☐").body().muted())
                            .padding_with((0.0, 4.0))
                    }),
                    message_bubble(inner.clone(), row),
                ))
                .spacing(4.0)
                .on_tap(move |store: Store| {
                    // Only multi-select mode: context-menu "Select"
                    // seeds the set; further taps toggle membership.
                    if !store.selected_msgs.snapshot().is_empty() {
                        store.toggle_select(rid);
                    }
                })
                .padding_with([pad_top, pad_bottom, 12.0, 12.0]),
            )
        })
        .scroll_controller(&scroller),
    )),
    when(store.members_open.clone(), move || {
        vstack((
            hstack((
                text!("{members_count}", members_count = store.members_count.clone())
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
                    let is_user =
                        matches!(row.sender, enums::MessageSender::User(_));
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
                        "Kick".action(move |store: Store| {
                            store.kick_member(&r_kick)
                        }),
                        "Block".action(move |store: Store| {
                            store.toggle_block(&row, true)
                        }),
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
                        clock_outline()
                            .tint(MutedForeground)
                            .size(12.0, 12.0),
                        text(label).caption().line_limit(ONE),
                        spacer(),
                        text(row.time.clone()).caption().muted(),
                    ))
                    .spacing(6.0)
                    .padding_with((4.0, 10.0))
                    .context_menu(("Send now".action(move |store: Store| {
                        store.scheduled_send_now(mid)
                    }),))
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
                    .hide_label(),
                icon_button(close(), "Close search", |store: Store| {
                    store.chat_search_open.set(false);
                    store.chat_search.set_from("");
                    store.chat_search_results.set(Vec::new());
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
        let first_url = store
            .attach
            .map(|v: Vec<Url>| {
                v.first()
                    .map(|u| Url::from_file_path_str(Str::from(u.path().to_string())))
                    .unwrap_or_else(|| Url::from_file_path_str(Str::from("")))
            })
;
        let is_img = store
            .attach
            .map(|v: Vec<Url>| {
                v.first()
                    .map(|u| {
                        let p = u.path().to_string().to_lowercase();
                        [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
                            .iter()
                            .any(|e| p.ends_with(e))
                    })
                    .unwrap_or(false)
            })
;
        let fname = store
            .attach
            .map(|v: Vec<Url>| {
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
            })
;
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
        let packs_b = store.sticker_packs.clone();
        let items_b = sticker_items.clone();
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
            when(emoji_tab2, emoji_grid).otherwise(move || {
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
                                .background(
                                    RoundedRectangle::new(0.5).fill(SurfaceVariant),
                                )
                                .on_tap(move |store: Store| {
                                    store.open_sticker_pack(id)
                                })
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
                            let no_file = cell_store
                                .file_signal(thumb)
                                .str_is_empty()
                                .distinct();
                            let url = cell_store.file_signal(thumb).map(Url::from_file_path_str);
                            let cell = zstack((
                                when(has, move || {
                                    Photo::new(url.clone()).max_width(72.0).max_height(72.0)
                                }),
                                when(no_file, move || text(fallback.clone()).headline()),
                            ))
                            .anyview();
                            cell.padding_with((6.0, 6.0))
                                .background(
                                    RoundedRectangle::new(0.2).fill(SurfaceVariant),
                                )
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
                    text!("@{u}", u = uname.clone()).caption().bold().foreground(Accent),
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
        when(poll_open, {
            let st = store_for_poll.clone();
            move || poll_creator(st.clone())
        }),
    ))
    .spacing(0.0),
    hstack((
        FilePicker::open(
            label("Attach file").icon(paperclip()).icon_only(),
            &store.attach,
        )
        .context_menu(("Create poll".action(|store: Store| {
            store.toggle_poll_creator()
        }),)),
        icon_button(emoticon(), "Stickers & GIFs", |store: Store| {
            store.toggle_stickers()
        }),
        field("Message", &composer_b)
            .prompt("Message")
            .hide_label(),
        // Search/members/scheduled live in the navigation toolbar
        // (Telegram Desktop parity). A `Spacer` before the mic/send slot
        // keeps it trailing-aligned.
        spacer(),
        when(composer_empty, || {
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
    .background(Surface),
    ))
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn chat_detail(store: Store, chat_id: i64) -> NavigationView {
    let title = store
        .chats
        .map(move |rows| {
            rows.iter()
                .find(|r| r.id == chat_id)
                .map(|r| r.title.clone())
                .unwrap_or_default()
        })
        .distinct();
    let subtitle = store.chat_subtitle(chat_id);

    let composer_b = store.composer.clone();
    let search_open2 = store.chat_search_open.clone();
    let search_debounced = store.chat_search.debounce(Duration::from_millis(400));
    let viewer_open = store.viewer.is_some().distinct();
    let store_for_viewer = store.clone();
    let store_for_info = store.clone();
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
    let store_for_info_overlay = store.clone();
    // Honest reproduction: `when(a).otherwise(b)` (WhenComplete) panics at
    // mount on the shipped renderers — nami#23, DOGFOOD r11-4a. Kept in this
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
        when(viewer_open, move || viewer_layer(store_for_viewer.clone())),
    ))
    .on_change(&search_debounced, |q: Str, store: Store| store.run_chat_search(q))
    .on_change(&composer_b, |_: Str, store: Store| {
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
                    store.chat_search_open.set(!search_open2.snapshot())
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
            0x20 | 0x200D | 0xFE0E | 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF
            | 0xE0020..=0xE007F => {}
            0x2300..=0x27BF | 0x2B00..=0x2BFF | 0x1F000..=0x1FAFF => n += 1,
            _ => return 0,
        }
    }
    n
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn message_bubble(store: Store, row: MessageRow) -> impl View {
    let reply_excerpt = row.reply_excerpt.clone();
    let has_reply = !reply_excerpt.is_empty();
    let body_text = row.text.clone();
    let body_styled = row.styled.clone();
    let has_styled = !body_styled.is_empty();
    let webpage = row.webpage.clone();
    let has_webpage = !webpage.is_empty();
    let fwd = row.forwarded_from.clone();
    let has_fwd = !fwd.is_empty();
    let has_text = !body_text.is_empty();
    let chips = row.reaction_chips.clone();
    let has_reactions = !chips.is_empty();
    let time = row.time.clone();
    let media = media_slot(&store, &row);
    let sender = row.sender.clone();
    let r1 = row.clone();
    let r2 = row.clone();
    let r3 = row.clone();
    let r4 = row.clone();
    let r5 = row.id;
    let r_view = row.clone();

    // Secondary text inside the bubble: `MutedForeground` on a plain
    // background, a translucent on-accent color inside an outgoing bubble —
    // the fixed muted token is unreadable on the accent fill.
    let row_outgoing = row.outgoing;
    let muted_parts = move |v: AnyView| -> AnyView {
        if row_outgoing {
            v.foreground(WithOpacity::new(AccentForeground, 0.78)).anyview()
        } else {
            v.muted().anyview()
        }
    };
    let has_media = row.media_file != 0 || row.play_file != 0 || !row.media_label.is_empty();
    // Emoji-only messages render large on no bubble fill (Telegram Desktop).
    let emoji_n = if has_media || has_webpage || has_styled || row.poll.is_some() {
        0
    } else {
        emoji_count(&body_text)
    };

    let mut parts: Vec<AnyView> = Vec::new();
    if has_fwd {
        parts.push(muted_parts(
            text(fwd.clone()).italic(true).caption().line_limit(ONE).anyview(),
        ));
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
    if row.group_first && !row.outgoing && !sender.is_empty() {
        parts.push(
            text(sender.clone())
                .caption()
                .bold()
                .foreground(Accent)
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
        parts.push(poll_block(row.id, poll).anyview());
    }
    if has_text {
        if emoji_n > 0 {
            let size = if emoji_n <= 3 {
                44.0
            } else if emoji_n <= 8 {
                34.0
            } else {
                26.0
            };
            parts.push(text(body_text.clone()).size(size).anyview());
        } else if has_styled {
            parts.push(text(body_styled.clone()).body().anyview());
        } else {
            parts.push(text(body_text.clone()).body().anyview());
        }
    }
    if has_webpage {
        parts.push(muted_parts(
            text(webpage.clone()).caption().line_limit(TWO).anyview(),
        ));
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
    let content = vstack(parts)
        .spacing(4.0)
        .leading()
        .padding_with([10.0, 10.0 + meta_band + chips_band, 10.0, 10.0]);

    // Reaction strip: one pill per emoji — tap toggles it (Desktop parity);
    // the chosen pill gets the accent fill.
    let chip_views: Vec<AnyView> = chips
        .iter()
        .map(|c| {
            let emoji = c.emoji.clone();
            let e2 = emoji.to_string();
            let label = format!("{} {}", emoji, c.count);
            let r = row.clone();
            let pill = text(label)
                .caption()
                .padding_with((1.0, 6.0))
                .on_tap(move |store: Store| store.toggle_reaction(&r, &e2))
                .a11y_role(AccessibilityRole::Button)
                .a11y_label(format!(
                    "React with {}, {} {}",
                    emoji,
                    c.count,
                    if c.count == 1 { "reaction" } else { "reactions" }
                ));
            if c.chosen {
                pill.foreground(AccentForeground)
                    .background(RoundedRectangle::new(0.5).fill(Accent))
                    .anyview()
            } else {
                pill.background(RoundedRectangle::new(0.5).fill(SurfaceVariant))
                    .anyview()
            }
        })
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

    let meta_overlay = hstack((
        when(row.edited, || text("edited").caption()),
        text(time).caption(),
        if row.pending {
            clock_outline().size(11.0, 11.0).anyview()
        } else if row.failed {
            alert_circle()
                .tint(Error)
                .size(11.0, 11.0)
                .on_tap(move |store: Store| store.resend_failed(row.id))
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

    // Signal-derived cap — Telegram Desktop's ~72%-of-pane rule with its
    // absolute 480dp ceiling. Honest repro: `Frame::max_width` samples a
    // signal only at mount (waterui#1214, DOGFOOD r11-2b), so the cap is
    // whatever the mount-time pane width produces; no wrap inside the cap
    // on hydrolysis yet (hydrolysis#130). No workaround applied.
    let bubble_cap = store
        .win_frame
        .map(|f| ((f.width() - 340.0) * 0.72).clamp(220.0, 480.0));
    // Finite zstack layers: `ZStackLayout` reports the envelope of its
    // children, so the bubble hugs max(content, chips, meta) — BottomLeading
    // anchors the chips under the content, BottomTrailing the meta row.
    let bubble_inner = zstack((
        zstack((content, reactions_overlay)).alignment(BottomLeading),
        meta_overlay,
    ))
    .alignment(BottomTrailing);
    let bubble: AnyView = if emoji_n > 0 && !row.highlighted {
        // No fill: the emoji itself is the content (Telegram Desktop).
        Frame::new(bubble_inner)
            .max_width(bubble_cap)
            .foreground(Foreground)
            .anyview()
    } else {
        Frame::new(bubble_inner)
            .max_width(bubble_cap)
            .background(if row.highlighted {
                RoundedRectangle::new(0.18).fill(AccentContainer)
            } else if row.outgoing {
                RoundedRectangle::new(0.18).fill(Accent)
            } else {
                RoundedRectangle::new(0.18).fill(SurfaceVariant)
            })
            .foreground(if row.outgoing {
                Color::from(AccentForeground)
            } else {
                Color::from(Foreground)
            })
            .anyview()
    };

    let react_label: &'static str = if row.my_reaction == "👍" {
        "Remove 👍"
    } else {
        "React 👍"
    };
    let pinned = store.pinned_id.get() == row.id;
    let pin_label: &'static str = if pinned {
        "Unpin message"
    } else {
        "Pin message"
    };
    let r6 = row.clone();
    let r7 = row.clone();
    let r8 = row.clone();
    let r9 = row.clone();
    let r10 = row.clone();
    let r11 = row.id;
    let bubble = bubble.context_menu((
        "Reply".action(move |store: Store| store.start_reply(&r1)),
        "Select".action(move |store: Store| store.toggle_select(r11)),
        react_label.action(move |store: Store| store.toggle_reaction(&r6, "👍")),
        "React ❤️".action(move |store: Store| store.toggle_reaction(&r7, "❤️")),
        "React 😂".action(move |store: Store| store.toggle_reaction(&r8, "😂")),
        "React 😮".action(move |store: Store| store.toggle_reaction(&r9, "😮")),
        "React 😢".action(move |store: Store| store.toggle_reaction(&r10, "😢")),
        "Forward".action(move |store: Store| store.start_forward(&r2)),
        pin_label.action(move |store: Store| {
            if pinned {
                store.unpin_message(row.id)
            } else {
                store.pin_message(row.id)
            }
        }),
        "Copy".action(move |store: Store| store.copy_message(&r3)),
        "Edit".action(move |store: Store| store.start_edit(&r4)),
        "Delete".action(move |store: Store| store.delete_message(r5)),
    ));

    let placed = if row.outgoing {
        hstack((spacer(), bubble)).anyview()
    } else if row.avatar_col {
        // Groups/channels reserve a leading avatar column on incoming rows
        // so a run's bubbles stay aligned; the avatar itself shows only on
        // the run's last row, bottom-aligned (Telegram Desktop).
        let slot: AnyView = if row.show_avatar {
            avatar(store.clone(), row.sender_photo, sender.as_str(), 32.0).anyview()
        } else {
            Color::from(Surface).size(32.0, 32.0).anyview()
        };
        hstack((vstack((spacer(), slot)).spacing(0.0), bubble, spacer()))
            .spacing(4.0)
            .anyview()
    } else {
        hstack((bubble, spacer())).anyview()
    };
    let placed = if row.unread_divider {
        vstack((
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
            .padding_with((2.0, 4.0)),
            placed,
        ))
        .spacing(0.0)
        .anyview()
    } else {
        placed
    };
    if row.day_header {
        vstack((
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
            .padding_with((2.0, 4.0)),
            placed,
        ))
        .spacing(0.0)
        .anyview()
    } else {
        placed
    }
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
#[allow(signal_get_in_view)] // the `has` gate rebuilds the view when the
// file lands; get() then reads the resolved path at rebuild time
pub(crate) fn media_slot(store: &Store, row: &MessageRow) -> impl View {
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
        vstack((
            when(has, move || {
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
                    if tfid != 0 {
                        Photo::new(thumb.map(Url::from_file_path_str))
                            .max_width(96.0)
                            .max_height(96.0)
                            .clip(RoundedRectangle::new(0.12))
                            .anyview()
                    } else {
                        image_outline()
                            .tint(MutedForeground)
                            .size(14.0, 14.0)
                            .anyview()
                    },
                    text!("{label_text}{suffix}", suffix = pct.map(|p: i32| {
                        if p > 0 && p < 100 {
                            Str::from(format!(" — {p}%"))
                        } else {
                            Str::from("")
                        }
                    }))
                    .caption()
                    .muted(),
                ))
                .padding_with((4.0, 8.0))
            }),
        ))
        .anyview()
    } else if row.media_file != 0 {
        let fid = row.media_file;
        let path_a = store.file_signal(fid);
        let path_b = store.file_signal(fid);
        let has = path_a.map(|p: Str| !p.is_empty()).distinct();
        let url = path_b.map(Url::from_file_path_str);
        let label_text = row.media_label.clone();
        let pct = store.file_progress_signal(fid);
        when(has, move || {
                Photo::new(url.clone()).max_width(320.0).clip(RoundedRectangle::new(0.12))
            })
            .otherwise(move || {
                hstack((
                    image_outline().tint(MutedForeground).size(14.0, 14.0),
                    text!("{label_text}{suffix}", suffix = pct.map(|p: i32| {
                        if p > 0 && p < 100 {
                            Str::from(format!(" — {p}%"))
                        } else {
                            Str::from("")
                        }
                    }))
                    .caption()
                    .muted(),
                ))
                .padding_with((4.0, 8.0))
                .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
            }).anyview()
    } else if !row.media_label.is_empty() {
        hstack((
                file().tint(MutedForeground).size(14.0, 14.0),
                text(row.media_label.clone()).caption().muted(),
            ))
            .padding_with((4.0, 8.0))
            .background(RoundedRectangle::new(0.2).fill(SurfaceVariant)).anyview()
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

    let content = scroll(vstack((
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
            .a11y_role(AccessibilityRole::Button)
            ,
            VStack::for_each(
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
            ),
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
                .action(|store: Store| store.terminate_all_sessions())
                ,
            )),
            VStack::for_each(
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
                    .context_menu(("Terminate".action(move |store: Store| {
                        store.terminate_session_by_id(row.id)
                    }),))
                },
            ),
            vstack((
                text(store.tr("BlockedUsers", 0, "Blocked users"))
                    .caption()
                    .muted(),
                VStack::for_each(
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
                ),
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
                text!("{storage}", storage = store.storage_summary.clone())
                    .muted(),
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
                .action(|store: Store| store.clear_storage())
                ,
                spacer(),
            )),
        ))
        .spacing(8.0)
        .leading(),
        // Language: packs from getLocalizationTargetInfo; tapping applies
        // setOption language_pack_id and refetches the pack's strings.
        vstack((
            text(store.tr("Language", 0, "Language"))
                .caption()
                .muted(),
            VStack::for_each(
                SignalCollection::new(store.lang_packs.clone()),
                |row: LangRow| {
                    let id = row.id.clone();
                    hstack((
                        text(row.name.clone()),
                        when(row.beta, || text("beta").caption().muted()),
                        spacer(),
                        when(row.active, || check().tint(Accent).size(16.0, 16.0)),
                    ))
                    .padding_with((4.0, 0.0))
                    .on_tap(move |store: Store| store.apply_language(&id))
                    .a11y_label(Str::from(format!("Use {}", row.name)))
                    .a11y_role(AccessibilityRole::Button)
                    
                },
            ),
        ))
        .spacing(4.0)
        .leading(),
        text(store.tr("Appearance", 0, "Appearance"))
            .caption()
            .muted(),
        toggle("Dark mode", &dark),
        text(store.tr("SessionsTitle", 0, "Session"))
            .caption()
            .muted(),
        button(store.tr("LogOut", 0, "Log out")).action(|store: Store| store.logout()),
    ))
    .spacing(10.0)
    .leading()
    .padding_with((12.0, 16.0)))
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
            icon_button(close(), "Close", |store: Store| {
                store.twofa_open.set(false)
            }),
        )),
        SecureField::new("Current password", &store.twofa_old).hide_label(),
        SecureField::new("New password (empty disables)", &store.twofa_new).hide_label(),
        field("Password hint", &store.twofa_hint_in)
            .prompt("Hint")
            .hide_label(),
        field("Recovery email", &store.twofa_email)
            .prompt("Recovery email (optional)")
            .hide_label(),
        when(
            note.map(|s: Str| !s.is_empty()).distinct(),
            move || text(note.clone()).caption().muted(),
        ),
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
                hstack((
                    text(row.name.clone()).caption(),
                    spacer(),
                ))
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
    let name = card.map(|c| {
        c.as_ref().map(|c| c.name.clone()).unwrap_or_default()
    });
    let username = card.map(|c| {
        c.as_ref().map(|c| c.username.clone()).unwrap_or_default()
    });
    let phone = card.map(|c| {
        c.as_ref().map(|c| c.phone.clone()).unwrap_or_default()
    });
    let bio = card.map(|c| {
        c.as_ref().map(|c| c.bio.clone()).unwrap_or_default()
    });
    let online = card.map(|c| c.as_ref().map(|c| c.online).unwrap_or(false));
    let uid = card.map(|c| c.as_ref().map(|c| c.user_id).unwrap_or(0));

    let content = scroll(vstack((
        text!("{name}", name = name.clone()).headline(),
        text!("{username}", username = username.clone())
            .caption()
            .muted(),
        when(online.distinct(), || {
            text("online").caption().foreground(Accent)
        }),
        hstack((text("Phone"), spacer(), text!("{phone}", phone = phone.clone()).muted())),
        text!("{bio}", bio = bio.clone()).body().muted(),
        hstack((
            spacer(),
            button("Message").action(move |store: Store| {
                let id = uid.snapshot();
                if id != 0 {
                    store.nav.pop();
                    store.start_chat_with(id);
                }
            }),
        )),
    ))
    .spacing(10.0)
    .padding_with((12.0, 16.0)));
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

    let content = scroll(vstack((
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
        VStack::for_each(
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
                    "New secret chat".action(move |store: Store| {
                        store.new_secret_chat(row.key)
                    }),
                    "Remove contact".action(move |store: Store| {
                        store.remove_contact(row.key)
                    }),
                ))
            },
        ),
    ))
    .spacing(10.0)
    .padding_with((12.0, 16.0)))
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
        .unwrap_or_else(crate::capture::new_video_note_shared);
    let status = shared.borrow().status.clone();
    let rec_flag = store.video_recording.clone();
    let rec_label = store.video_elapsed.clone();
    vstack((
        hstack((
            text("Video note").headline(),
            spacer(),
            icon_button(close(), "Close", |store: Store| {
                store.close_video_note()
            }),
        )),
        GpuSurface::new(VideoNoteGpu::new(shared))
            .size(240.0, 240.0)
            .background(RoundedRectangle::new(0.5).fill(SurfaceVariant)),
        when(
            status.map(|s: Str| !s.is_empty()).distinct(),
            move || text(status.clone()).caption().muted(),
        ),
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
        toggle(store.tr("PollAnonymous", 0, "Anonymous votes"), &anon_b)
            .padding_with((0.0, 12.0)),
        when(regular, {
            let multi_label = store.tr("PollMultiple", 0, "Multiple answers");
            move || {
                toggle(multi_label.clone(), &multi_b).padding_with((0.0, 12.0))
            }
        }),
        toggle(store.tr("QuizMode", 0, "Quiz mode"), &quiz_b)
            .padding_with((0.0, 12.0)),
        // Telegram actions: text buttons on the trailing edge, primary last.
        hstack((
            spacer(),
            button(store.tr("Cancel", 0, "Cancel"))
                .style(ButtonStyle::Plain)
                .action(|store: Store| store.poll_open.set(false)),
            button(store.tr("Create", 0, "Create"))
                .action(|store: Store| store.send_poll()),
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
                hstack((
                    Color::from(BorderColor).width(1.0),
                    info_panel(store),
                )),
            )))
            .width(281.0)
            .max_height(f32::INFINITY)
            .shadow(Shadow::new(
                Color::srgb_hex("#000000").with_opacity(0.30),
                Vector::new(-2.0, 0.0),
                8.0,
                0.0,
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
    let media_chunks = SignalCollection::new(
        store
            .shared_media
            .map(|v: Vec<SharedMediaRow>| {
                v.chunks(3)
                    .enumerate()
                    .map(|(ix, c)| MediaChunkRow {
                        ix,
                        cells: c.to_vec(),
                    })
                    .collect::<Vec<MediaChunkRow>>()
            }),
    );
    let cells = store.clone();
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
                    VStack::for_each(
                    SignalCollection::new(member_rows.members.clone()),
                    move |row: MemberRow| {
                        let label_text = format!("Member {}", row.name);
                        let av_store = member_av.clone();
                        hstack((
                            button(Label::new(Str::from(label_text), move || {
                                hstack((
                                    avatar(av_store.clone(), row.photo, &row.name, 32.0),
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
                )
                }
            ))
            .spacing(0.0)
        }),
        text("Shared media").caption().muted().padding_with((8.0, 14.0)),
        scroll(VStack::for_each(media_chunks, move |chunk: MediaChunkRow| {
            let mut cells_v: Vec<AnyView> = Vec::new();
            for cell in chunk.cells.iter().take(3) {
                cells_v.push(media_cell(cells.clone(), cell.clone()).anyview());
            }
            hstack(cells_v).spacing(4.0)
        })),
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
fn poll_block(message_id: i64, poll: &PollRow) -> impl View {
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
                    text(format!("{}%", o.pct)).caption().muted(),
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
        text(if poll.closed {
            format!("{} votes · closed", poll.voters)
        } else {
            format!("{} votes", poll.voters)
        })
        .caption()
        .muted(),
    ))
    .spacing(6.0)
    .leading()
}

/// In-pane media viewer (Desktop's viewer is fullscreen; ours covers the
/// detail pane): photo or playable payload with sender, caption, close.
#[allow(signal_get_in_view)] // the `when` gate rebuilds this view when the
// viewer opens; `url.snapshot()` then reads the resolved path at rebuild time.
fn viewer_layer(store: Store) -> impl View {
    // The `when(viewer_open)` gate on the caller side hides this layer until
    // a viewer row exists; file 0 simply resolves to the Downloading branch.
    let row = store.viewer.snapshot().unwrap_or(ViewerRow {
        file: 0,
        video: false,
        caption: "".into(),
        from: "".into(),
    });
    let file = row.file;
    let video = row.video;
    let has = store
        .file_signal(file)
        .map(|p: Str| !p.is_empty())
        .distinct();
    let url = store.file_signal(file).map(Url::from_file_path_str);
    let url_photo = url.clone();
    let media = when(has, move || {
        let url_v = url.clone();
        let url_p = url_photo.clone();
        when(video, move || video_player(url_v.snapshot()).max_height(420.0)).otherwise(
            move || {
                Photo::new(url_p.snapshot())
                    .max_width(430.0)
                    .max_height(430.0)
                    .clip(RoundedRectangle::new(0.05))
            },
        )
    })
    .otherwise(|| {
        text("Downloading…")
            .caption()
            .foreground(Srgb::from_hex("#B0B0B0"))
    });
    // Desktop's media viewer: an opaque dark overlay carrying light text —
    // light header row (sender + white controls) and a light caption under
    // the media, nothing of the chat bleeds through.
    let viewer_fg = Color::srgb_hex("#FFFFFF");
    let viewer_scrim = Color::srgb_hex("#101010");
    vstack((
        hstack((
            text(row.from.clone())
                .body()
                .bold()
                .foreground(viewer_fg.clone()),
            spacer(),
            icon_button(close().tint(viewer_fg.clone()), "Close viewer", |store: Store| {
                store.close_viewer()
            }),
        ))
        .spacing(8.0)
        .padding_with((8.0, 12.0))
        .background(viewer_scrim.clone()),
        spacer(),
        media,
        spacer(),
        text(row.caption.clone())
            .caption()
            .line_limit(THREE)
            .foreground(viewer_fg.clone())
            .padding_with((8.0, 12.0)),
    ))
    .background(viewer_scrim)
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
    scroll(vstack(rows).spacing(2.0).leading().padding_with((4.0, 8.0)))
        .max_height(160.0)
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
