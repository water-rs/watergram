use crate::renderer::bounded_proposal;
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(feature = "accessibility")]
use crate::renderer::{AccessibilityActionTarget, RenderContext};
use crate::renderer::{
    HydroNativeView, HydroState, RetainedSubview, WidgetRenderContext, measure_tabs_layout,
    tabs_bar_and_content_rect, tabs_button_rect, tabs_content_proposal,
};
#[cfg(feature = "accessibility")]
use accesskit::{
    Action as AccessibilityAction, Node as AccessibilityNode, Role as AccessibilityNodeRole,
};
use nami::Binding;
use waterui::navigation::tab::{NativeTabStyle, TabsLayout};
use waterui_core::id::Id;
use waterui_core::layout::{ProposalSize, Size as LayoutSize, ViewDimensions};
use waterui_core::{AnyView, Environment, Native};

#[cfg(feature = "accessibility")]
use crate::widgets::util::widget_disabled;

/// The retained render state of one tab. Its `label` is a move-only `AnyView`, so
/// it is held as a [`RetainedSubview`] built once and re-flushed each frame; its
/// `content` is a cloneable `Rc`-backed builder rebuilt fresh each frame; `tag`
/// drives selection.
struct TabRenderState {
    tag: Id,
    label: RetainedSubview,
    content: RetainedSubview,
    enabled: nami::Computed<bool>,
}

/// The retained render state of a `TabsLayout` container. The selection `Binding` and tab
/// native style are kept by value; each tab is a [`TabRenderState`].
pub(crate) struct TabsRenderState {
    selection: Binding<Id>,
    style: NativeTabStyle,
    tabs: Vec<TabRenderState>,
}

impl TabsRenderState {
    pub(crate) fn from_tabs(tabs: TabsLayout) -> Self {
        assert!(
            !(tabs.tabs.is_empty()),
            "hydrolysis Tabs requires at least one tab"
        );
        // `TabsLayout` is `#[non_exhaustive]`, so access fields rather than destructuring.
        let selection = tabs.selection;
        let style = tabs.style;
        let tabs = tabs
            .tabs
            .into_iter()
            .map(|tab| TabRenderState {
                tag: tab.id,
                label: RetainedSubview::new(tab.label),
                content: RetainedSubview::new(AnyView::new(tab.content.build())),
                enabled: tab.enabled,
            })
            .collect();
        Self {
            selection,
            style,
            tabs,
        }
    }

    /// Eagerly build the tab-label sub-views (the measure path has no renderer to
    /// build on).
    pub(crate) fn prebuild_labels(
        &mut self,
        renderer: &mut crate::renderer::SemanticCore,
        env: &Environment,
    ) {
        for tab in &mut self.tabs {
            tab.label.ensure_built(renderer, env);
            tab.content.ensure_built(renderer, env);
        }
    }

    fn selected_index(&self, selected_id: Id) -> usize {
        self.tabs
            .iter()
            .position(|tab| tab.tag == selected_id)
            .unwrap_or_else(|| panic!("hydrolysis Tabs selection is not present in tabs"))
    }
}

impl HydroNativeView for Native<TabsLayout> {
    fn intrinsic(
        state: &mut HydroState,
        view: &Self,
        env: &Environment,
        theme: &Rc<dyn crate::engine::WidgetTheme>,
    ) -> LayoutSize {
        measure_tabs_layout(
            view.as_inner(),
            ProposalSize::UNSPECIFIED,
            state,
            env,
            theme,
        )
    }

    fn dimensions(
        state: &mut HydroState,
        view: &Self,
        env: &Environment,
        theme: &Rc<dyn crate::engine::WidgetTheme>,
        proposal: ProposalSize,
    ) -> ViewDimensions {
        ViewDimensions::new(measure_tabs_layout(
            view.as_inner(),
            proposal,
            state,
            env,
            theme,
        ))
    }
}

/// Emits a tab list's accessibility tree from per-tab `(tag, interaction_key,
/// default_label, is_selected)` tuples. Shared by the dispatch path and the
/// retained `Widget`-node path (which extracts each default label from its
/// tab's [`RetainedSubview`]).
#[cfg(feature = "accessibility")]
pub(crate) fn tabs_accessibility(
    renderer: &mut crate::renderer::SemanticCore,
    ctx: Option<RenderContext>,
    theme: Option<&Rc<dyn crate::engine::WidgetTheme>>,
    selection: &Binding<Id>,
    style: NativeTabStyle,
    labels: &[(Id, crate::renderer::InteractionKey, Option<String>, bool)],
    env: &Environment,
) {
    let disabled = renderer.read_signal(&widget_disabled(env));
    // Bar/button rects exist only in the rendered frame; the semantic walk
    // emits the same TabList/Tab structure with no bounds.
    let bar_rect = ctx.zip(theme).map(|(ctx, theme)| {
        let metrics = theme.tabs_metrics();
        tabs_bar_and_content_rect(ctx.bounds, style, metrics.bar_height).0
    });
    let mut tab_list = AccessibilityNode::new(
        renderer.resolve_accessibility_role(env, AccessibilityNodeRole::TabList),
    );
    let tab_list_label = renderer.resolve_accessibility_label(env, None);
    if let Some(label) = tab_list_label {
        tab_list.set_label(label);
    }
    for (index, (tag, interaction_key, default_label, is_selected)) in labels.iter().enumerate() {
        let mut tab_node = AccessibilityNode::new(
            renderer.resolve_accessibility_role(env, AccessibilityNodeRole::Tab),
        );
        let label = renderer.resolve_accessibility_label(env, default_label.clone());
        if let Some(label) = label {
            tab_node.set_label(label);
        }
        tab_node.set_selected(*is_selected);
        tab_node.add_action(AccessibilityAction::Focus);
        if disabled {
            tab_node.set_disabled();
        } else {
            tab_node.add_action(AccessibilityAction::Click);
        }
        let key = i64::from(i32::from(*tag));
        let target = (!disabled).then(|| AccessibilityActionTarget::PickerSelect {
            selection: selection.clone(),
            target: *tag,
        });
        let tab_node_id = match ctx.zip(bar_rect) {
            Some((ctx, bar_rect)) => renderer.register_accessibility_child_node_with_key(
                key,
                tab_node,
                crate::renderer::transformed_rect(
                    ctx.hit_transform,
                    tabs_button_rect(bar_rect, labels.len(), index, style),
                ),
                env,
                target,
            ),
            None => renderer
                .register_accessibility_child_node_with_key_semantic(key, tab_node, env, target),
        };
        if let Some(tab_node_id) = tab_node_id {
            tab_list.push_child(tab_node_id);
            renderer.register_accessibility_focus_link(interaction_key, tab_node_id);
        }
    }
    match ctx.zip(bar_rect) {
        Some((ctx, bar_rect)) => {
            let _ = renderer.register_accessibility_node(
                tab_list,
                crate::renderer::transformed_rect(ctx.hit_transform, bar_rect),
                env,
                None,
            );
        }
        None => {
            let _ = renderer.register_accessibility_node_semantic(tab_list, env, None);
        }
    }
}

/// Measures a retained tabs leaf from its [`TabsRenderState`]: each tab's
/// retained content answers the proposal its rendered content rect hands it —
/// the pane minus the tab bar (see [`tabs_content_proposal`]) — mirroring
/// `measure_tabs_layout`.
pub(crate) fn measure_tabs_node(
    state: &TabsRenderState,
    proposal: ProposalSize,
    hydro: &mut HydroState,
    env: &Environment,
    theme: &Rc<dyn crate::engine::WidgetTheme>,
) -> ViewDimensions {
    let metrics = theme.tabs_metrics();
    let content_proposal = tabs_content_proposal(proposal, state.style, metrics.bar_height);
    let mut max_content_width: f64 = 0.0;
    let mut max_content_height: f64 = 0.0;
    let mut bar_width = 0.0;
    for tab in &state.tabs {
        let label_size = tab.label.measure_built(hydro, env, theme);
        bar_width += (f64::from(label_size.width) + metrics.button_horizontal_inset * 2.0)
            .max(metrics.button_min_width);

        let content_size =
            tab.content
                .measure_built_with_proposal(hydro, env, theme, content_proposal);
        max_content_width = max_content_width.max(f64::from(content_size.width));
        max_content_height = max_content_height.max(f64::from(content_size.height));
    }
    let (width, height) = match state.style {
        NativeTabStyle::Automatic | NativeTabStyle::TabBar => (
            max_content_width.max(bar_width),
            max_content_height + metrics.bar_height,
        ),
        NativeTabStyle::Sidebar => (
            max_content_width + metrics.bar_height,
            max_content_height.max(metrics.button_min_width * state.tabs.len() as f64),
        ),
    };
    ViewDimensions::new(LayoutSize::new(
        proposal.width.unwrap_or(width as f32),
        proposal.height.unwrap_or(height as f32),
    ))
}

/// Renders a retained tabs leaf every flush: emits the tab-list a11y (unless
/// hidden) then the bar + selected content, reading the selection signal live.
pub(crate) fn render_tabs_node(
    ctx: &mut WidgetRenderContext<'_>,
    state: &Rc<RefCell<TabsRenderState>>,
    env: &Environment,
) {
    #[cfg(feature = "accessibility")]
    {
        let hidden = env
            .get::<waterui::accessibility::AccessibilityHidden>()
            .is_some_and(waterui::accessibility::AccessibilityHidden::is_hidden);
        if !hidden {
            let selected_id = ctx.renderer_mut().read_signal(&state.borrow().selection);
            let (selection, style, labels) = {
                let st = state.borrow();
                let selected_index = st.selected_index(selected_id);
                let labels: Vec<(Id, crate::renderer::InteractionKey, Option<String>, bool)> = st
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        (
                            tab.tag,
                            crate::renderer::InteractionKey::for_rc(
                                state,
                                i32::from(tab.tag) as u32 as usize,
                            ),
                            tab.label.default_a11y_label(),
                            index == selected_index,
                        )
                    })
                    .collect();
                (st.selection.clone(), st.style, labels)
            };
            let render_ctx = ctx.render_context();
            let theme = ctx.theme();
            tabs_accessibility(
                ctx.renderer_mut(),
                Some(render_ctx),
                Some(&theme),
                &selection,
                style,
                &labels,
                env,
            );
        }
    }
    render_tabs_parts(ctx, state, env);
}

pub(crate) fn render_tabs_parts(
    ctx: &mut WidgetRenderContext<'_>,
    state: &Rc<RefCell<TabsRenderState>>,
    env: &Environment,
) {
    let (selection, style, tab_count) = {
        let st = state.borrow();
        assert!(
            !st.tabs.is_empty(),
            "hydrolysis Tabs requires at least one tab"
        );
        (st.selection.clone(), st.style, st.tabs.len())
    };
    let selected_id = ctx.renderer_mut().read_signal(&selection);
    let selected_index = state.borrow().selected_index(selected_id);

    let theme_metrics = ctx.theme().tabs_metrics();
    let (bar_rect, content_rect) =
        tabs_bar_and_content_rect(ctx.bounds, style, theme_metrics.bar_height);

    {
        let theme = ctx.theme();
        let mut draw = ctx.draw_context();
        theme.draw_tabs_bar(&mut draw, bar_rect, false);
    }

    for index in 0..tab_count {
        let button_rect = tabs_button_rect(bar_rect, tab_count, index, style);
        let tab_id = state.borrow().tabs[index].tag;
        let interaction_key =
            crate::renderer::InteractionKey::for_rc(state, i32::from(tab_id) as u32 as usize);
        // The label sub-view is prebuilt (node path: `prebuild_labels`; dispatch
        // path: `render` calls `prebuild_labels`), so measure it directly for
        // placement.
        let label_size = {
            let cell = state.borrow();
            let theme = ctx.theme();
            cell.tabs[index]
                .label
                .measure_built(ctx.state_mut(), env, &theme)
        };
        {
            let hit_bounds = crate::renderer::transformed_rect(ctx.hit_transform, button_rect);
            let (interaction, press_slot, _) =
                ctx.renderer_mut()
                    .bind_interaction_target(interaction_key, hit_bounds, env);
            let interaction =
                crate::renderer::local_interaction_state(interaction, ctx.hit_transform);
            let is_selected = index == selected_index;
            {
                let theme = ctx.theme();
                let mut draw = ctx.draw_context();
                if is_selected {
                    let highlight = tabs_active_indicator_rect(
                        button_rect,
                        style,
                        theme_metrics.active_indicator_height,
                        if matches!(style, NativeTabStyle::Sidebar) {
                            f64::from(label_size.height)
                        } else {
                            f64::from(label_size.width)
                        },
                    );
                    theme.draw_tabs_highlight(&mut draw, highlight);
                }
                theme.draw_tabs_button_state_layer(
                    &mut draw,
                    button_rect,
                    is_selected,
                    interaction,
                );
            }
            let selection_binding = selection.clone();
            let enabled = {
                let st = state.borrow();
                ctx.renderer_mut().read_signal(&st.tabs[index].enabled)
            };
            if enabled {
                ctx.renderer_mut().register_interactive_pointer_target(
                    hit_bounds,
                    press_slot,
                    move |_renderer, _point, _env| {
                        if selection_binding.get() != tab_id {
                            selection_binding.set(tab_id);
                        }
                        true
                    },
                );
            }
        }
        let label_rect = tabs_label_rect(button_rect, label_size, theme_metrics);
        if label_rect.width() > 0.0 && label_rect.height() > 0.0 {
            // The tab label's a11y is emitted by `tabs_accessibility`, so suppress
            // the sub-view's own a11y (matching the dispatch path's
            // `dispatch_in_rect_without_accessibility`).
            #[cfg(feature = "accessibility")]
            ctx.renderer_mut().push_accessibility_suppression();
            // A tab gets an equal share of the bar and no more. Without this a
            // long label drew straight over its neighbour and off the edge of
            // the bar, since the label lays out at its natural width.
            ctx.push_layer_rect(1.0, button_rect);
            let render_ctx = ctx.render_context();
            state.borrow_mut().tabs[index].label.flush_in_rect(
                ctx.renderer_mut(),
                render_ctx,
                env,
                ProposalSize::UNSPECIFIED,
                label_rect,
            );
            ctx.pop_layer();
            #[cfg(feature = "accessibility")]
            ctx.renderer_mut().pop_accessibility_suppression();
        }
    }

    if content_rect.width() > 0.0 && content_rect.height() > 0.0 {
        let mut st = state.borrow_mut();
        let render_ctx = ctx.render_context();
        st.tabs[selected_index].content.flush_in_rect(
            ctx.renderer_mut(),
            render_ctx,
            env,
            bounded_proposal(content_rect),
            content_rect,
        );
    }
}

fn tabs_label_rect(
    button_rect: vello::kurbo::Rect,
    label_size: waterui_core::layout::Size,
    metrics: waterui_backend_core::widget::TabsMetrics,
) -> vello::kurbo::Rect {
    let max_width = (button_rect.width() - metrics.button_horizontal_inset * 2.0).max(0.0);
    let width = f64::from(label_size.width).min(max_width);
    let height = f64::from(label_size.height).min(button_rect.height());
    let x0 = button_rect.x0 + (button_rect.width() - width) * 0.5;
    let y0 = button_rect.y0 + (button_rect.height() - height) * 0.5;
    vello::kurbo::Rect::new(x0, y0, x0 + width, y0 + height)
}

fn tabs_active_indicator_rect(
    button_rect: vello::kurbo::Rect,
    style: NativeTabStyle,
    thickness: f64,
    label_extent: f64,
) -> vello::kurbo::Rect {
    match style {
        NativeTabStyle::Automatic | NativeTabStyle::TabBar => {
            let width = label_extent.clamp(0.0, button_rect.width());
            let x0 = button_rect.x0 + (button_rect.width() - width) * 0.5;
            let x1 = x0 + width;
            vello::kurbo::Rect::new(
                x0,
                button_rect.y0,
                x1,
                (button_rect.y0 + thickness).min(button_rect.y1),
            )
        }
        NativeTabStyle::Sidebar => {
            let height = label_extent.clamp(0.0, button_rect.height());
            let y0 = button_rect.y0 + (button_rect.height() - height) * 0.5;
            vello::kurbo::Rect::new(
                (button_rect.x1 - thickness).max(button_rect.x0),
                y0,
                button_rect.x1,
                y0 + height,
            )
        }
    }
}

/// Emits a retained tabs layout's accessibility tree for the semantic walk:
/// the tab bar and tab nodes `tabs_accessibility` registers (labels are
/// suppressed in the sub-view flush, so they emit nothing themselves), then
/// the selected tab's content subtree — it flushes unsuppressed in the
/// rendered path, so it emits its own nodes here too.
#[cfg(feature = "accessibility")]
pub(crate) fn emit_tabs_accessibility(
    renderer: &mut crate::renderer::SemanticCore,
    state: &Rc<RefCell<TabsRenderState>>,
    env: &Environment,
) {
    let owner = state;
    let mut state = state.borrow_mut();
    let selected_id = renderer.read_signal(&state.selection);
    let (selection, style, labels) = {
        let selected_index = state.selected_index(selected_id);
        let labels: Vec<(Id, crate::renderer::InteractionKey, Option<String>, bool)> = state
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                (
                    tab.tag,
                    crate::renderer::InteractionKey::for_rc(
                        owner,
                        i32::from(tab.tag) as u32 as usize,
                    ),
                    tab.label.default_a11y_label(),
                    index == selected_index,
                )
            })
            .collect();
        (state.selection.clone(), state.style, labels)
    };
    tabs_accessibility(renderer, None, None, &selection, style, &labels, env);
    let selected_index = state.selected_index(selected_id);
    state.tabs[selected_index]
        .content
        .emit_accessibility(renderer, env);
}
