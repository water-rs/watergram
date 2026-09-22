//! View layer for Watergram.
//!
//! All views are pure declarations over [`Store`]; every handler takes
//! `store: Store` as a `#[state]` extractor injected once at the root.

use std::num::NonZeroUsize;
use std::time::Duration;

use waterui::component::lazy::Lazy;
use waterui::media::Photo;
use waterui::video::video_player;
use waterui::navigation::{ColumnWidth, NavigationSplitView, NavigationView, Navigator};
use waterui::prelude::*;
use waterui::reactive::collection::SignalCollection;
use waterui::text::IntoText;
use waterui::form::picker::file::FilePicker;
use waterui::shape::{Circle, RoundedRectangle, ShapeExt};
use waterui::theme::color::{
    Accent, AccentForeground, Error, Foreground, MutedForeground, Surface, SurfaceVariant,
};
use waterui::views::ForEach;
use waterui::widget::condition::when;
use waterui_barcode::Barcode;
use tdlib_rs::enums;
use waterui_icons_material_icon as mdi;

use crate::state::{AccountRow, ChatRow, FolderRow, PackRow, MemberRow, MessageRow, PrivacyRow, Route, Screen, SessionRow, StickerItem, Store};
use mdi::account;
use mdi::folder_plus;
use mdi::account_group;
use mdi::alert_circle;
use mdi::archive;
use mdi::bookmark;
use mdi::bullhorn;
use mdi::clock_outline;
use mdi::close;
use mdi::delete_sweep;
use mdi::account_plus;
use mdi::emoticon;
use mdi::link_variant;
use mdi::cog;
use mdi::file;
use mdi::image_outline;
use mdi::lock;
use mdi::magnify;
use mdi::paperclip;
use mdi::pin;
use mdi::plus;
use mdi::send;
use mdi::share_variant;

const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const TWO: NonZeroUsize = NonZeroUsize::new(2).unwrap();

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
        vstack((
            text("Watergram").title().foreground(Foreground),
            text("Select a chat to start messaging").body().muted(),
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
        sidebar_view(store)
            .title("Watergram")
            .searchable(&search, "Search chats"),
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
    let forward_mode = store.forward_message.is_some();
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
        hstack((
            button(label("New chat").icon(plus()))
                .label_style(LabelDisplayMode::IconOnly)
                .plain()
                .action(|nav: Navigator<Route>| nav.push(Route::NewChat)),
            icon_button(archive(), "Archive", |store: Store| {
                store.toggle_archive_view()
            }),
            icon_button(account(), "Accounts", |store: Store| {
                store.accounts_open.toggle()
            }),
            spacer(),
            text!("{conn}").caption().muted(),
            spacer(),
            button(label("Settings").icon(cog()))
                .label_style(LabelDisplayMode::IconOnly)
                .plain()
                .action(|nav: Navigator<Route>| nav.push(Route::Settings)),
        ))
        .padding_with((12.0, 6.0)),
        when(store.accounts_open.clone(), move || {
            let rows = store.accounts.clone();
            vstack((
                Lazy::vstack(ForEach::new(
                    SignalCollection::new(rows.clone()),
                    move |acc: AccountRow| {
                        let id = acc.id;
                        hstack((text(acc.label.clone()).caption(), spacer()))
                            .padding_with((12.0, 4.0))
                            .on_tap(move |store: Store| store.switch_account(id))
                    },
                )),
                hstack((
                    plus().tint(Accent).size(14.0, 14.0),
                    text("Add account").caption().foreground(Accent),
                    spacer(),
                ))
                .padding_with((12.0, 4.0))
                .on_tap(|store: Store| store.add_account()),
            ))
            .background(Surface)
        }),
        when(has_folders, move || {
            let folder_edit = store.folder_open.clone();
            vstack((
                hstack((
                    scroll(Lazy::hstack(ForEach::new(
                        SignalCollection::new(folder_tabs.clone()),
                        move |tab: FolderRow| {
                            let label = tab.title.clone();
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
                                .padding_with((10.0, 3.0))
                                .background(if tab.active {
                                    RoundedRectangle::new(0.5)
                                        .fill(SurfaceVariant)
                                        .anyview()
                                } else {
                                    AnyView::default()
                                })
                                .on_tap(move |store: Store| store.set_list(id));
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
                    ))),
                    icon_button(folder_plus(), "New folder", |store: Store| {
                        store.open_folder_editor(0)
                    }),
                ))
                .padding_with((8.0, 4.0)),
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
                    .padding_with((10.0, 4.0))
                })
                }
            ))
        }),
        when(forward_mode, || {
            hstack((
                share_variant().tint(Accent).size(16.0, 16.0),
                text("Select a chat to forward to")
                    .caption()
                    .foreground(Accent),
                spacer(),
                icon_button(close(), "Cancel", |store: Store| {
                    store.forward_message.set(None)
                }),
            ))
            .padding_with((12.0, 8.0))
            .background(Surface)
        }),
        scroll(
            vstack((
                Lazy::vstack(ForEach::new(filtered, move |row: ChatRow| {
                    chat_row(rows_store.clone(), row)
                })),
                when(show_results, || {
                    vstack((
                        Divider,
                        text("Global search results")
                            .caption()
                            .muted()
                            .padding_with((12.0, 4.0)),
                    ))
                    .leading()
                }),
                Lazy::vstack(ForEach::new(server_results, move |row: ChatRow| {
                    chat_row(res_store.clone(), row)
                })),
            ))
            .leading(),
        ),
    ))
    .on_change(&debounced, |q: Str, store: Store| store.run_search(q))
}

pub(crate) fn icon_button<F>(icon: impl View + Clone + 'static, name: &'static str, on_tap: F) -> impl View
where
    F: Fn(Store) + 'static,
{
    button(label(name).icon(icon))
        .label_style(LabelDisplayMode::IconOnly)
        .plain()
        .action(move |store: Store| on_tap(store))
}

pub(crate) fn kind_icon(kind: &str) -> impl View {
    match kind {
        "group" => account_group().tint(MutedForeground).size(12.0, 12.0),
        "channel" => bullhorn().tint(MutedForeground).size(12.0, 12.0),
        "secret" => lock().tint(MutedForeground).size(12.0, 12.0),
        "saved" => bookmark().tint(MutedForeground).size(12.0, 12.0),
        _ => account().tint(MutedForeground).size(12.0, 12.0),
    }
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
}

#[allow(if_else_view)] // when() needs a signal; conditions here are plain bools
pub(crate) fn chat_row(store: Store, row: ChatRow) -> impl View {
    let id = row.id;
    let unread = row.unread;
    let preview: Str = if row.typing {
        "typing…".into()
    } else {
        row.preview.clone()
    };
    let badge: AnyView = if row.pinned {
        pin().tint(MutedForeground).size(14.0, 14.0).anyview()
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
                .padding_with((6.0, 2.0))
                .background(if row.muted {
                    Circle.fill(SurfaceVariant)
                } else {
                    Circle.fill(Accent)
                }).anyview()
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
                text(preview).caption().line_limit(ONE).muted(),
                spacer(),
                badge,
            ))
            .spacing(4.0),
        ))
        .spacing(2.0)
        .leading(),
    ))
    .spacing(10.0)
    .padding_with((10.0, 6.0))
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
        "Leave chat".action(move |store: Store| store.leave(id)),
    ))
    .on_tap(move |store: Store| store.select_chat(id))
}

// ---------------------------------------------------------------------------
// Chat detail
// ---------------------------------------------------------------------------

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
    let subtitle = store
        .chats
        .map(move |rows| {
            rows.iter()
                .find(|r| r.id == chat_id)
                .map(|r| {
                    if r.online {
                        Str::from("online")
                    } else {
                        Str::from("")
                    }
                })
                .unwrap_or_default()
        })
        .distinct();

    let has_more = store.no_more_history.not();
    let reply_open = store.reply_to.is_some();
    let edit_open = store.editing.is_some();
    let reply_label = store.reply_label.clone();
    let messages = SignalCollection::new(store.messages.clone());
    let has_attach = store.attach.map(|v| !v.is_empty()).distinct();
    let scroller = store.scroll.clone();
    let composer_b = store.composer.clone();
    let has_pinned = store.pinned_label.map(|s: Str| !s.is_empty()).distinct();
    let pinned_label = store.pinned_label.clone();
    let search_open = store.chat_search_open.clone();
    let search_open2 = store.chat_search_open.clone();
    let search_b = store.chat_search.clone();
    let search_debounced = store.chat_search.debounce(Duration::from_millis(400));
    let search_results_b = store.chat_search_results.clone();
    let stickers_open = store.stickers_open.clone();
    let sticker_items = store.sticker_items.clone();
    let store_cells = store.clone();
    let inner = store.clone();

    vstack((
        when(has_pinned, move || {
            hstack((
                pin().tint(Accent).size(14.0, 14.0),
                text!("{pinned_label}")
                    .caption()
                    .line_limit(ONE)
                    .foreground(Accent),
                spacer(),
            ))
            .padding_with((12.0, 6.0))
            .background(Surface)
            .on_tap(|store: Store| store.jump_to_message(store.pinned_id.get()))
        }),
        scroll(
            vstack((
                when(has_more, || {
                    button("Load earlier messages…")
                        .label_style(LabelDisplayMode::TitleOnly)
                        .action(|store: Store| store.load_older())
                }),
                Lazy::vstack(ForEach::new(messages, move |row: MessageRow| {
                    message_bubble(inner.clone(), row)
                })),
            ))
            .leading()
            .padding_with((12.0, 8.0)),
        )
        .scroll_controller(&scroller),
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
                .padding_with((10.0, 6.0)),
                hstack((
                    field("Group title", &store.admin_title).hide_label(),
                    button("Rename").action(|store: Store| store.rename_chat()),
                ))
                .spacing(6.0)
                .padding_with((10.0, 0.0)),
                hstack((
                    field("Group description", &store.admin_desc).hide_label(),
                    button("Set").action(|store: Store| store.set_chat_desc()),
                ))
                .spacing(6.0)
                .padding_with((10.0, 0.0)),
                hstack((
                    FilePicker::open(
                        label("Photo").icon(image_outline()),
                        &store.chat_avatar_pick,
                    ),
                    button("Set photo").action(|store: Store| store.set_chat_avatar()),
                ))
                .spacing(6.0)
                .padding_with((10.0, 0.0)),
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
                .padding_with((10.0, 4.0)),
                scroll(Lazy::vstack(ForEach::new(
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
                        .padding_with((10.0, 4.0))
                        .on_tap(move |store: Store| {
                            if is_user {
                                store.open_profile(uid)
                            }
                        })
                        .context_menu((
                            "Kick".action(move |store: Store| {
                                store.kick_member(&r_kick)
                            }),
                            "Block".action(move |store: Store| {
                                store.toggle_block(&row, true)
                            }),
                        ))
                    },
                )))
                .max_height(200.0),
            ))
            .background(Surface)
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
                .padding_with((10.0, 6.0)),
                scroll(Lazy::vstack(ForEach::new(
                    SignalCollection::new(search_results_b.clone()),
                    move |row: MessageRow| {
                        hstack((
                            text(row.time.clone()).caption().muted(),
                            text(row.text.clone()).caption().line_limit(ONE),
                            spacer(),
                        ))
                        .spacing(6.0)
                        .padding_with((10.0, 4.0))
                        .on_tap(move |store: Store| store.jump_to_message(row.id))
                    },
                )))
                .max_height(160.0),
            ))
            .background(Surface)
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
        when(has_attach, || {
            hstack((
                paperclip().tint(Accent).size(16.0, 16.0),
                text("File attached — press send")
                    .caption()
                    .foreground(Accent),
                spacer(),
                icon_button(close(), "Remove", |store: Store| {
                    store.attach.set(Vec::new())
                }),
            ))
            .padding_with((12.0, 6.0))
            .background(Surface)
        }),
        when(stickers_open, move || {
            let cells = store_cells.clone();
            vstack((
                hstack((
                    field("Emoji search", &store.sticker_query)
                        .prompt("😀 emoji")
                        .hide_label(),
                    icon_button(magnify(), "Search stickers", |store: Store| {
                        store.search_stickers_by()
                    }),
                ))
                .spacing(6.0)
                .padding_with((8.0, 2.0)),
            {
                let packs = store.sticker_packs.clone();
                scroll(Lazy::hstack(ForEach::new(
                    SignalCollection::new(packs.clone()),
                    move |pack: PackRow| {
                        let id = pack.id;
                        text(pack.title.clone())
                            .caption()
                            .padding_with((8.0, 2.0))
                            .background(
                                RoundedRectangle::new(0.5).fill(SurfaceVariant),
                            )
                            .on_tap(move |store: Store| {
                                store.open_sticker_pack(id)
                            })
                    },
                )))
                .max_height(28.0)
            },
            scroll(Lazy::hstack(ForEach::new(
                SignalCollection::new(sticker_items.clone()),
                move |item: StickerItem| {
                    let cell_store = cells.clone();
                    let thumb = item.thumb;
                    let fallback: Str = if item.gif {
                        "GIF".into()
                    } else {
                        item.emoji.clone()
                    };
                    let it = item.clone();
                    let cell = if thumb != 0 {
                        let has = cell_store
                            .file_signal(thumb)
                            .map(|p: Str| !p.is_empty())
                            .distinct();
                        let url = cell_store.file_signal(thumb).map(Url::from_file_path_str);
                        when(has, move || {
                            Photo::new(url.clone()).max_width(72.0).max_height(72.0)
                        })
                        .otherwise(move || text(fallback.clone()).headline())
                        .anyview()
                    } else {
                        text(fallback).headline().anyview()
                    };
                    cell.padding_with((6.0, 6.0))
                        .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
                        .on_tap(move |store: Store| store.send_sticker(it.clone()))
                },
            )))
            .max_height(96.0),
            ))
        }),
        hstack((
            FilePicker::open(label("Attach file").icon(paperclip()), &store.attach)
                .label_style(LabelDisplayMode::IconOnly),
            icon_button(emoticon(), "Stickers & GIFs", |store: Store| {
                store.toggle_stickers()
            }),
            field("Message", &composer_b)
                .prompt("Message")
                .hide_label(),
            icon_button(magnify(), "Search in chat", move |store: Store| {
                store.chat_search_open.set(!search_open2.get())
            }),
            icon_button(account_group(), "Members", move |store: Store| {
                let open = !store.members_open.get();
                store.members_open.set(open);
                if open {
                    store.load_members();
                }
            }),
            icon_button(send(), "Send", |store: Store| store.send()).context_menu((
                "Send silently".action(|store: Store| store.send_silent()),
                "Send in 1 hour".action(|store: Store| store.send_later(3600)),
            )),
        ))
        .spacing(6.0)
        .padding_with((10.0, 8.0))
        .background(Surface),
    ))
    .on_change(&search_debounced, |q: Str, store: Store| store.run_chat_search(q))
    .on_change(&composer_b, |_: Str, store: Store| store.typing_ping())
    .title(text!("{title}"))
    .navigation_subtitle(text!("{subtitle}"))
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
    .padding_with((12.0, 6.0))
    .background(Surface)
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
    let reactions = row.reactions.clone();
    let has_reactions = !reactions.is_empty();
    let time = row.time.clone();
    let media = media_slot(&store, &row);
    let sender = row.sender.clone();
    let r1 = row.clone();
    let r2 = row.clone();
    let r3 = row.clone();
    let r4 = row.clone();
    let r5 = row.id;

    let bubble = vstack((
        when(has_fwd, move || {
            text(fwd.clone()).italic(true).caption().line_limit(ONE).muted()
        }),
        when(has_reply, move || {
            text(reply_excerpt.clone()).caption().line_limit(TWO).muted()
        }),
        media,
        when(
            !row.outgoing && !row.sender.is_empty(),
            move || text(sender.clone()).caption().bold().foreground(Accent),
        ),
        when(has_text, move || -> AnyView {
            if has_styled {
                text(body_styled.clone()).body().anyview()
            } else {
                text(body_text.clone()).body().anyview()
            }
        }),
        when(has_webpage, move || {
            text(webpage.clone()).caption().line_limit(TWO).muted()
        }),
        hstack((
            when(has_reactions, move || text(reactions.clone()).caption()),
            spacer(),
            text(time).caption(),
            if row.pending {
                clock_outline().size(11.0, 11.0).anyview()
            } else if row.failed {
                alert_circle().tint(Error).size(11.0, 11.0).anyview()
            } else if row.outgoing && row.read_out {
                text("✓✓").caption().anyview()
            } else if row.outgoing {
                text("✓").caption().anyview()
            } else {
                spacer().width(1.0).anyview()
            },
        ))
        .spacing(3.0),
    ))
    .spacing(4.0)
    .padding_with(10.0)
    .max_width(420.0)
    .leading()
    .background(if row.outgoing {
        RoundedRectangle::new(0.18).fill(Accent)
    } else {
        RoundedRectangle::new(0.18).fill(Surface)
    });

    let bubble = bubble.foreground(if row.outgoing {
        Color::from(AccentForeground)
    } else {
        Color::from(Foreground)
    });

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
    let bubble = bubble.context_menu((
        "Reply".action(move |store: Store| store.start_reply(&r1)),
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

    if row.outgoing {
        hstack((spacer(), bubble)).anyview()
    } else {
        hstack((bubble, spacer())).anyview()
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
                    video_player(url.get())
                        .max_height(56.0)
                        .max_width(320.0)
                        .anyview()
                } else {
                    video_player(url.get())
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
                .padding_with((8.0, 4.0))
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
                .padding_with((8.0, 4.0))
                .background(RoundedRectangle::new(0.2).fill(SurfaceVariant))
            }).anyview()
    } else if !row.media_label.is_empty() {
        hstack((
                file().tint(MutedForeground).size(14.0, 14.0),
                text(row.media_label.clone()).caption().muted(),
            ))
            .padding_with((8.0, 4.0))
            .background(RoundedRectangle::new(0.2).fill(SurfaceVariant)).anyview()
    } else {
        spacer().width(0.0).anyview()
    }
}

// ---------------------------------------------------------------------------
// Pushed routes
// ---------------------------------------------------------------------------

pub(crate) fn settings_view(store: Store) -> NavigationView {
    store.start_notification_watchers();
    let me = store.me.clone();
    let dark = store.dark.clone();

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
            hstack((
                field("First name", &store.edit_first).hide_label(),
                field("Last name", &store.edit_last).hide_label(),
            ))
            .spacing(8.0),
            field("Bio", &store.edit_bio).prompt("a few words about you").hide_label(),
            field("Username", &store.edit_username)
                .prompt("username (no @)")
                .hide_label(),
            hstack((
                text!("{profile_note}", profile_note = store.profile_note.clone())
                    .caption()
                    .muted(),
                spacer(),
                button("Save").action(|store: Store| store.save_profile()),
            )),
            hstack((
                FilePicker::open(label("Choose photo").icon(image_outline()), &store.avatar_pick),
                button("Set avatar").action(|store: Store| store.set_avatar()),
            ))
            .spacing(8.0),
        ))
        .spacing(10.0)
        .leading(),
        vstack((
            text("Privacy").caption().muted(),
            hstack((
                text("Two-step verification"),
                spacer(),
                text!("{twofa}", twofa = store.twofa.clone()).muted(),
            )),
            Lazy::vstack(ForEach::new(
                SignalCollection::new(store.privacy_rows.clone()),
                move |row: PrivacyRow| {
                    let k = row.key.clone();
                    let k2 = row.key.clone();
                    let k3 = row.key.clone();
                    hstack((
                        text(row.setting.clone()).caption(),
                        spacer(),
                        text(row.audience.clone()).caption().muted(),
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
                    ))
                },
            )),
            vstack((
                text("Notifications").caption().muted(),
                toggle("Private chats", &store.notif_private),
                toggle("Groups", &store.notif_groups),
                toggle("Channels", &store.notif_channels),
            ))
            .spacing(4.0),
            hstack((
                text("Active sessions").caption().muted(),
                spacer(),
                text("Terminate other sessions")
                    .caption()
                    .foreground(Error)
                    .on_tap(|store: Store| store.terminate_all_sessions()),
            )),
            Lazy::vstack(ForEach::new(
                SignalCollection::new(store.sessions.clone()),
                move |row: SessionRow| {
                    let is_current = row.current;
                    hstack((
                        vstack((
                            text(row.title.clone()).caption().bold(),
                            text(row.subtitle.clone()).caption().muted(),
                        ))
                        .spacing(2.0)
                        .leading(),
                        spacer(),
                        when(is_current, || text("current").caption().foreground(Accent)),
                    ))
                    .padding_with((0.0, 4.0))
                    .context_menu(("Terminate".action(move |store: Store| {
                        store.terminate_session_by_id(row.id)
                    }),))
                },
            )),
        ))
        .spacing(10.0)
        .leading(),
        vstack((
            text("Storage").caption().muted(),
            hstack((
                text!("{storage}", storage = store.storage_summary.clone())
                    .caption()
                    .muted(),
                spacer(),
                button("Refresh").action(|store: Store| store.load_storage()),
            )),
            hstack((
                delete_sweep().tint(Error).size(14.0, 14.0),
                text("Clear cached media")
                    .caption()
                    .foreground(Error)
                    .on_tap(|store: Store| store.clear_storage()),
                spacer(),
            )),
        ))
        .spacing(8.0)
        .leading(),
        text("Appearance").caption().muted(),
        toggle("Dark mode", &dark),
        text("Session").caption().muted(),
        button("Log out").action(|store: Store| store.logout()),
    ))
    .spacing(10.0)
    .padding_with((16.0, 12.0)))
    .task({
        let store = store.clone();
        async move {
            store.load_sessions();
            store.load_twofa();
            store.load_storage();
            store.load_privacy();
        }
    });
    NavigationView::new("Settings", content)
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
                let id = uid.get();
                if id != 0 {
                    store.nav.pop();
                    store.start_chat_with(id);
                }
            }),
        )),
    ))
    .spacing(10.0)
    .padding_with((16.0, 12.0)));
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
        Lazy::vstack(ForEach::new(
            SignalCollection::new(store.contacts.clone()),
            move |row: MemberRow| {
                hstack((
                    text(row.name.clone()).caption(),
                    spacer(),
                    text(row.status.clone()).caption().muted(),
                ))
                .spacing(6.0)
                .padding_with((0.0, 4.0))
                .on_tap(move |store: Store| store.start_chat_with(row.key))
                .context_menu(("Remove contact".action(move |store: Store| {
                    store.remove_contact(row.key)
                }),))
            },
        )),
    ))
    .spacing(10.0)
    .padding_with((16.0, 12.0)))
    .task({
        let store = store.clone();
        async move {
            store.load_contacts();
        }
    });
    NavigationView::new("New chat", content)
}
