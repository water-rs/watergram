//! Popup-window accessibility merge in the rendered runtime.
//!
//! `HeadlessRuntime::pump_at` returns one tree update for every open
//! window — the main window and each popup — exactly as
//! `SemanticRuntime`'s merged update does. A context menu opened by a
//! secondary click mounts as a popup window whose nodes used to stay in
//! the popup core's own pending update, unreachable by label.

use std::time::Instant;

use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId, TreeUpdate};
use nami::Binding;
use waterui::ViewExt as _;
use waterui_controls::button::button;
use waterui_controls::menu::CommandExt as _;
use waterui_core::AnyView;
use waterui_core::handler::AnyViewBuilder;
use waterui_form::picker::color::ColorPicker;
use waterui_graphics::Color;
use waterui_layout::frame::Frame;
use waterui_layout::stack::vstack;

use super::{MinimalTestTheme, test_environment};
use crate::HeadlessRuntime;
use crate::platform::{InputEvent, PointerButton, PointerKind};

const WINDOW_SIZE: f32 = 160.0;

/// The node labelled `label` with `role`, if present.
pub(super) fn find_by_label<'a>(
    update: &'a TreeUpdate,
    role: Role,
    label: &str,
) -> Option<(NodeId, &'a Node)> {
    update.nodes.iter().find_map(|(id, node)| {
        (node.role() == role && node.label() == Some(label)).then_some((*id, node))
    })
}

fn act(runtime: &mut HeadlessRuntime, action: Action, target: NodeId) -> bool {
    runtime.perform_accessibility_action(ActionRequest {
        action,
        target_node: target,
        target_tree: TreeId::ROOT,
        data: None,
    })
}

fn secondary_click(x: f32, y: f32) -> [InputEvent; 2] {
    [
        InputEvent::PointerDown {
            id: 1,
            kind: PointerKind::Mouse,
            x,
            y,
            button: PointerButton::Secondary,
        },
        InputEvent::PointerUp {
            id: 1,
            kind: PointerKind::Mouse,
            x,
            y,
            button: PointerButton::Secondary,
        },
    ]
}

/// A secondary click on a `.context_menu` view mounts the menu as a popup
/// window; the pump's returned update must merge that window's nodes into
/// the main tree — the same merge `SemanticRuntime` applies — or a test
/// host can never reach the menu by label.
#[test]
fn secondary_click_merges_the_context_menu_popup_into_the_tree() {
    let copied = Binding::container(false);
    let copied_for_view = copied.clone();
    let builder = AnyViewBuilder::<AnyView>::new(move || {
        let copied = copied_for_view.clone();
        AnyView::new(
            Frame::new(button("host").action(|| {}))
                .width(WINDOW_SIZE)
                .height(WINDOW_SIZE)
                .context_menu(vec!["Copy".action(move || copied.set(true))]),
        )
    });
    let mut runtime = HeadlessRuntime::new_for_tests(
        test_environment(),
        builder,
        WINDOW_SIZE as u32,
        WINDOW_SIZE as u32,
        MinimalTestTheme::default(),
    );

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the first frame must publish an accessibility tree");
    assert!(
        find_by_label(&update, Role::Button, "Copy").is_none(),
        "menu items must not emit before the menu opens"
    );

    // Secondary-click the centre of the host's emitted bounds — the same
    // spot the debug Inspect element item names through `node_at_point`.
    let (_, host) = find_by_label(&update, Role::Button, "host")
        .expect("the host button must emit an accessibility node");
    let bounds = host.bounds().expect("the host button has frame bounds");
    let (x, y) = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);

    for event in secondary_click(x as f32, y as f32) {
        runtime.push_input_event(event);
    }
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the click frame must publish an accessibility tree");
    let (copy, copy_node) = find_by_label(&update, Role::Button, "Copy")
        .expect("the context menu's items must merge into the returned tree");
    assert!(copy_node.supports_action(Action::Click));

    // Debug builds append "Inspect element" under the application's own
    // items — it reaches the merged tree by the same path.
    if cfg!(debug_assertions) {
        assert!(
            find_by_label(&update, Role::Button, "Inspect element").is_some(),
            "the appended Inspect element item must merge into the returned tree"
        );
    }

    // The shifted ids are the merged tree's contract: an action aimed at one
    // demuxes back to the popup core that owns the item's target.
    assert!(act(&mut runtime, Action::Click, copy));
    assert!(
        copied.get(),
        "clicking the merged popup item must fire its command"
    );

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the activation frame must publish an accessibility tree");
    assert!(
        find_by_label(&update, Role::Button, "Copy").is_none(),
        "the popup's nodes must leave the merged tree once it closes"
    );
}

/// water-rs/hydrolysis#140: a popup runs in the environment of the view that
/// opened it, so `.state(&store)` on an ancestor reaches the item action's
/// extractors — here through the secondary-click path rather than menu
/// activation.
#[test]
fn context_menu_item_action_reads_state_inherited_from_the_opening_view() {
    #[waterui::prelude::state]
    #[derive(Clone)]
    struct Store {
        hits: Binding<u32>,
    }

    let store = Store {
        hits: Binding::container(0),
    };
    let store_for_view = store.clone();
    let builder = AnyViewBuilder::<AnyView>::new(move || {
        let store = store_for_view.clone();
        AnyView::new(
            Frame::new(button("host").action(|| {}))
                .width(WINDOW_SIZE)
                .height(WINDOW_SIZE)
                .context_menu(vec!["Bump".action(|store: Store| {
                    store.hits.set(store.hits.get() + 1);
                })])
                .state(&store),
        )
    });
    let mut runtime = HeadlessRuntime::new_for_tests(
        test_environment(),
        builder,
        WINDOW_SIZE as u32,
        WINDOW_SIZE as u32,
        MinimalTestTheme::default(),
    );

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the first frame must publish an accessibility tree");
    let (_, host) = find_by_label(&update, Role::Button, "host")
        .expect("the host button must emit an accessibility node");
    let bounds = host.bounds().expect("the host button has frame bounds");
    let (x, y) = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);

    for event in secondary_click(x as f32, y as f32) {
        runtime.push_input_event(event);
    }
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the click frame must publish an accessibility tree");
    let (bump, _) = find_by_label(&update, Role::Button, "Bump")
        .expect("the context menu's items must merge into the returned tree");
    assert!(act(&mut runtime, Action::Click, bump));
    assert_eq!(
        store.hits.get(),
        1,
        "the item action did not reach the injected store"
    );
}
/// `is_settled` counts a window's pending frame — the main window's and each
/// popup's alike — so a test host cannot read a tree that predates a frame the
/// runtime already committed to. An accessibility action that changes its
/// window marks that window's frame pending: nothing is queued and no renderer
/// work is scheduled, yet the runtime is not settled until the pump that runs
/// the frame.
#[test]
fn a_window_with_a_pending_frame_is_not_settled() {
    let mut runtime = HeadlessRuntime::new_for_tests(
        test_environment(),
        AnyViewBuilder::<AnyView>::new(move || {
            AnyView::new(
                Frame::new(button("host").action(|| {}))
                    .width(WINDOW_SIZE)
                    .height(WINDOW_SIZE)
                    .context_menu(vec!["Copy".action(|| {})]),
            )
        }),
        WINDOW_SIZE as u32,
        WINDOW_SIZE as u32,
        MinimalTestTheme::default(),
    );

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the first frame must publish an accessibility tree");
    assert!(
        runtime.is_settled(),
        "a runtime with no pending frame settles"
    );

    // The focus action lands on the main window's core: only the main window's
    // frame mode can account for the runtime being unsettled.
    let (host, _) =
        find_by_label(&update, Role::Button, "host").expect("the host button is missing");
    assert!(act(&mut runtime, Action::Focus, host));
    assert!(
        !runtime.is_settled(),
        "a main window with a pending frame must not report settled"
    );
    assert!(
        runtime.has_pending_semantic_update(),
        "a pending frame is a state change requested but not yet flushed"
    );
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the pending frame must publish an accessibility tree");
    assert!(runtime.is_settled(), "the main window's pending frame ran");

    let (_, host) =
        find_by_label(&update, Role::Button, "host").expect("the host button must still emit");
    let bounds = host.bounds().expect("the host button has frame bounds");
    let (x, y) = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    for event in secondary_click(x as f32, y as f32) {
        runtime.push_input_event(event);
    }
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the click frame must publish the merged tree");
    let (copy, _) = find_by_label(&update, Role::Button, "Copy")
        .expect("the context menu's items must merge into the returned tree");
    // The mount pump ran the popup's first scene pump; the click on its item
    // arms the popup's next frame — mounted, pending, not yet run — and only
    // the popup's own frame mode can account for the runtime being unsettled.
    assert!(act(&mut runtime, Action::Click, copy));
    assert!(
        !runtime.is_settled(),
        "a popup with a pending frame must not report settled"
    );
    assert!(
        runtime.has_pending_semantic_update(),
        "a pending popup frame is a state change requested but not yet flushed"
    );
    let _ = runtime.pump_at(false, Instant::now());
    assert!(
        runtime.is_settled(),
        "the popup's pending frame ran and the runtime settled"
    );
}

/// A popup that changes while the main window is clean must still publish:
/// the merged update describes every open window, not only the one that
/// emitted. A `Focus` inside a colour picker's swatch panel re-emits the
/// popup core without touching the main window — the pump's update carries
/// the popup's changed tree anyway.
#[test]
fn a_popup_only_change_publishes_the_merged_tree() {
    let tint = Binding::container(Color::srgb(0, 0, 0));
    let tint_for_view = tint.clone();
    let mut runtime = HeadlessRuntime::new_for_tests(
        test_environment(),
        AnyViewBuilder::<AnyView>::new(move || {
            AnyView::new(vstack((ColorPicker::new("Tint", &tint_for_view),)))
        }),
        320,
        240,
        MinimalTestTheme::default(),
    );

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the first frame must publish an accessibility tree");
    let (trigger, _) =
        find_by_label(&update, Role::Button, "Tint").expect("the color picker is missing");
    assert!(
        find_by_label(&update, Role::Button, "Red").is_none(),
        "swatches must not appear before the picker opens"
    );
    assert!(act(&mut runtime, Action::Click, trigger));

    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the picker's first frame must publish an accessibility tree");
    let (swatch, _) = find_by_label(&update, Role::Button, "Red")
        .expect("the swatch panel must merge into the returned tree");

    // Focusing the swatch re-emits only the popup core; the main window has
    // no pending update, yet the pump must still publish the merged tree.
    assert!(act(&mut runtime, Action::Focus, swatch));
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("a popup-only change must still publish the merged tree");
    assert!(
        find_by_label(&update, Role::Button, "Red").is_some(),
        "the published update must carry the popup's nodes"
    );
    assert!(
        find_by_label(&update, Role::Button, "Tint").is_some(),
        "the published update must still carry the clean main window's nodes"
    );

    // The focused popup owns the merged tree's focus: the published focus
    // is the swatch's shifted id, not the main tree's.
    assert_eq!(
        update.focus, swatch,
        "the merged focus must follow the focused popup item"
    );

    // Closing the panel retires the popup; the merged focus returns to the
    // main tree's focused node — the trigger that opened it.
    assert!(act(&mut runtime, Action::Click, swatch));
    let update = runtime
        .pump_at(false, Instant::now())
        .tree_update
        .expect("the close frame must publish an accessibility tree");
    assert_eq!(
        update.focus, trigger,
        "the merged focus must return to the main tree once the popup closes"
    );
}

/// Pumps until the runtime settles and returns the last tree update it
/// emitted — `None` when nothing changed since the previous emit.
fn pump_until_settled(runtime: &mut HeadlessRuntime) -> Option<TreeUpdate> {
    let mut last = None;
    for _ in 0..64 {
        let result = runtime.pump_at(false, Instant::now());
        if let Some(update) = result.tree_update {
            last = Some(update);
        }
        if runtime.is_settled() {
            break;
        }
    }
    assert!(
        !runtime.has_pending_semantic_update(),
        "headless runtime never settled"
    );
    last
}

/// `tree_update` is the "the tree changed" signal: a pump where no window —
/// main or popup — emitted must publish `None`, or a test host invalidates
/// its snapshot on every pump.
#[test]
fn a_clean_pump_publishes_no_tree_update() {
    let copied = Binding::container(false);
    let copied_for_view = copied.clone();
    let builder = AnyViewBuilder::<AnyView>::new(move || {
        let copied = copied_for_view.clone();
        AnyView::new(
            Frame::new(button("host").action(|| {}))
                .width(WINDOW_SIZE)
                .height(WINDOW_SIZE)
                .context_menu(vec!["Copy".action(move || copied.set(true))]),
        )
    });
    let mut runtime = HeadlessRuntime::new_for_tests(
        test_environment(),
        builder,
        WINDOW_SIZE as u32,
        WINDOW_SIZE as u32,
        MinimalTestTheme::default(),
    );

    let update = pump_until_settled(&mut runtime)
        .expect("the first frame must publish an accessibility tree");
    assert!(
        runtime.pump_at(false, Instant::now()).tree_update.is_none(),
        "a settled pump with no popup must publish nothing"
    );

    // With a popup open the same rule holds: the menu's window is mounted
    // and merged, but once every core is clean the next pump publishes
    // nothing.
    let (_, host) =
        find_by_label(&update, Role::Button, "host").expect("the host button is missing");
    let bounds = host.bounds().expect("the host button has frame bounds");
    let (x, y) = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    for event in secondary_click(x as f32, y as f32) {
        runtime.push_input_event(event);
    }
    let update =
        pump_until_settled(&mut runtime).expect("the click frame must publish the merged tree");
    assert!(
        find_by_label(&update, Role::Button, "Copy").is_some(),
        "the menu's window must be merged before the clean-pump check"
    );
    assert!(
        runtime.pump_at(false, Instant::now()).tree_update.is_none(),
        "a settled pump with an open popup must publish nothing"
    );
}
