//! Measure/layout for the retained tree: [`RenderNode::layout`] re-reads
//! signals, re-measures through [`NodeSubView`], and caches each container's
//! child frames for the flush pass.

use super::*;
use waterui_graphics::{resolve_scene_proposal, scene_stretch_axis};

impl RenderNode {
    /// The stretch axis this node exposes when it is a layout child.
    /// The layout priority this subtree carries, or `0` when nothing set one.
    ///
    /// Only layout-transparent wrappers are walked through: a priority applies to
    /// the child it wraps, not across a container boundary.
    pub(super) fn priority(&self) -> i32 {
        match self {
            RenderNode::Wrapper(node) => match &node.effect {
                WrapperEffect::LayoutPriority(priority) => priority.get(),
                _ => node.child.priority(),
            },
            RenderNode::Opacity(node) => node.child.priority(),
            RenderNode::Scale(node) => node.child.priority(),
            RenderNode::Rotation(node) => node.child.priority(),
            RenderNode::Offset(node) => node.child.priority(),
            RenderNode::Retain(node) => node.child.priority(),
            RenderNode::Env(node) => node.child.priority(),
            RenderNode::Dynamic(node) => node.child.borrow().priority(),
            RenderNode::AppliedFilter(node) => node.child.priority(),
            RenderNode::Widget(node) => node.behavior.priority(),
            _ => 0,
        }
    }

    /// Whether this subtree draws nothing — WaterUI's empty view `()`, or a
    /// container/wrapper whose every descendant does the same.
    ///
    /// This is a semantic answer, not a measured size: a zero-size `Color` or
    /// a collapsed `Spacer` still renders and still answers `false`. A stack
    /// treats a child answering `true` as a non-member (§4.4: it takes no
    /// slot and no spacing), and a `Dynamic` flipping between `()` and content
    /// is a membership change — `layout` re-asks this every pass.
    pub(super) fn is_empty(&self) -> bool {
        match self {
            RenderNode::Widget(node) => node.behavior.renders_nothing(),
            // A container is always a member: even a frame wrapping `()`
            // explicitly claims its configured slot, like a `Spacer` does.
            RenderNode::Container(_) => false,
            RenderNode::Opacity(node) => node.child.is_empty(),
            RenderNode::Scale(node) => node.child.is_empty(),
            RenderNode::Rotation(node) => node.child.is_empty(),
            RenderNode::Offset(node) => node.child.is_empty(),
            RenderNode::Retain(node) => node.child.is_empty(),
            RenderNode::Env(node) => node.child.is_empty(),
            RenderNode::Dynamic(node) => node.child.borrow().is_empty(),
            RenderNode::Wrapper(node) => node.child.is_empty(),
            RenderNode::AppliedFilter(node) => node.child.is_empty(),
            // An effect over a child that draws nothing draws nothing itself.
            RenderNode::ViewEffect(node) => node.child.borrow().is_empty(),
            _ => false,
        }
    }

    pub(super) fn stretch(&self) -> StretchAxis {
        match self {
            RenderNode::Color(_) => StretchAxis::Both,
            RenderNode::Text(_) => StretchAxis::None,
            RenderNode::Container(container) => {
                // The retained children answer for themselves, so a transparent
                // layout reports what its content currently claims rather than
                // what it claimed when the tree was built.
                let child_axes: Vec<StretchAxis> =
                    container.children.iter().map(RenderNode::stretch).collect();
                container.layout.stretch_axis(&child_axes)
            }
            RenderNode::Opacity(node) => node.child.stretch(),
            RenderNode::Scale(node) => node.child.stretch(),
            RenderNode::Rotation(node) => node.child.stretch(),
            RenderNode::Offset(node) => node.child.stretch(),
            RenderNode::Retain(node) => node.child.stretch(),
            RenderNode::Env(node) => node.child.stretch(),
            RenderNode::Dynamic(node) => node.child.borrow().stretch(),
            // Scene content that is naturally a size is content-sized and claims
            // no leftover space; content that has no size of its own fills.
            RenderNode::SceneView(node) => {
                scene_stretch_axis(node.content.borrow().intrinsic_size())
            }
            // A GpuSurface fills its proposal (`GpuView::stretch_axis` default);
            // a ViewEffect is a `StretchAxis::None` raw view; an AppliedFilter is
            // a layout-transparent wrapper delegating to its child.
            RenderNode::GpuSurface(_) => StretchAxis::Both,
            RenderNode::ViewEffect(_) => StretchAxis::None,
            RenderNode::AppliedFilter(node) => node.child.stretch(),
            RenderNode::Scroll(_) => StretchAxis::Both,
            // The same stack laid out eagerly is content-sized on both axes, so a
            // lazy one has to be too: making a stack virtualizable must not change
            // how it sizes. Rows that want the full cross axis ask for it
            // themselves, exactly as they do in the eager path.
            RenderNode::LazyStack(_) => StretchAxis::None,
            RenderNode::Collection(node) => {
                // A collection's membership is reactive; its layout is one of the
                // content-sized stacks, which ignores the children anyway.
                node.layout.stretch_axis(&[])
            }
            RenderNode::Wrapper(node) => node.child.stretch(),
            RenderNode::Widget(node) => node.stretch,
        }
    }

    /// Measure this node under a proposal (recursive). Text shaping runs through
    /// the renderer's [`HydroState`] on the main thread.
    pub(in crate::renderer) fn measure(
        &self,
        state: &mut HydroState,
        env: &Environment,
        theme: &Rc<dyn crate::engine::WidgetTheme>,
        proposal: ProposalSize,
    ) -> ViewDimensions {
        match self {
            RenderNode::Color(_) => ViewDimensions::new(Size::new(
                proposal.width.unwrap_or(0.0),
                proposal.height.unwrap_or(0.0),
            )),
            RenderNode::Text(text) => HydrolysisRenderer::measure_text_dimensions(
                state,
                text.content.get(),
                text.alignment.get(),
                env,
                proposal.width,
                text.line_limit,
            ),
            RenderNode::Container(container) => {
                let cell = RefCell::new(state);
                let subs: Vec<NodeSubView> = container
                    .children
                    .iter()
                    .map(|child| NodeSubView::new(child, &cell, env, theme))
                    .collect();
                let refs: Vec<&dyn SubView> = subs.iter().map(|sub| sub as &dyn SubView).collect();
                ViewDimensions::new(container.layout.size_that_fits(proposal, &refs))
            }
            // Transform/opacity wrappers are layout-transparent.
            RenderNode::Opacity(node) => node.child.measure(state, env, theme, proposal),
            RenderNode::Scale(node) => node.child.measure(state, env, theme, proposal),
            RenderNode::Rotation(node) => node.child.measure(state, env, theme, proposal),
            RenderNode::Offset(node) => node.child.measure(state, env, theme, proposal),
            RenderNode::Retain(node) => node.child.measure(state, env, theme, proposal),
            RenderNode::Env(node) => node.child.measure(state, &node.env, theme, proposal),
            RenderNode::Dynamic(node) => node.child.borrow().measure(state, env, theme, proposal),
            // Scene content that is naturally a size (an SVG's viewBox, a
            // formula's typeset box) answers with it on whichever axis the
            // container left open; content that is not fills the proposal.
            RenderNode::SceneView(node) => {
                let resolved =
                    resolve_scene_proposal(node.content.borrow().intrinsic_size(), proposal);
                ViewDimensions::new(Size::new(
                    resolved.width.unwrap_or(0.0),
                    resolved.height.unwrap_or(0.0),
                ))
            }
            // A GpuSurface fills its proposal, like a self-drawn scene.
            RenderNode::GpuSurface(_) => ViewDimensions::new(Size::new(
                proposal.width.unwrap_or(0.0),
                proposal.height.unwrap_or(0.0),
            )),
            // A ViewEffect and an AppliedFilter are sized by their content: the
            // effect captures the child into a texture at the child's bounds.
            RenderNode::ViewEffect(node) => node
                .child
                .borrow()
                .measure(state, &node.env, theme, proposal),
            RenderNode::AppliedFilter(node) => {
                node.child.measure(state, &node.env, theme, proposal)
            }
            RenderNode::Scroll(_) => ViewDimensions::new(Size::new(
                proposal.width.unwrap_or(0.0),
                proposal.height.unwrap_or(0.0),
            )),
            RenderNode::LazyStack(node) => node.measure(state, theme, proposal),
            RenderNode::Collection(node) => node.measure(state, theme, proposal),
            // Layout-transparent: the wrapper measures its child under the node's
            // scoped environment (effect colors/a11y read env every frame).
            RenderNode::Wrapper(node) => node.child.measure(state, &node.env, theme, proposal),
            RenderNode::Widget(node) => node.behavior.measure(state, proposal, &node.env, theme),
        }
    }

    /// Run the layout-time prepare pass over this subtree: every widget leaf
    /// applies theme paint that could not be resolved at tree-build time —
    /// build contexts carry no theme — and builds the retained sub-views its
    /// measure path then reads. Called once at each layout or measure entry
    /// point (`RetainedSubview::{measure_intrinsic, patch_and_measure,
    /// flush_in_rect, flush_in_ctx, render_built_scene}` and the window's
    /// layout pump), before any node is measured. A semantic runtime never
    /// runs this pass, so no theme reaches it.
    pub(in crate::renderer) fn prepare_for_measure(&mut self, renderer: &mut HydrolysisRenderer) {
        match self {
            RenderNode::Widget(node) => node.behavior.prepare(renderer, &node.env),
            RenderNode::Opacity(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Scale(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Rotation(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Offset(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Retain(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Dynamic(node) => node.child.borrow_mut().prepare_for_measure(renderer),
            RenderNode::Env(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Wrapper(node) => node.child.prepare_for_measure(renderer),
            RenderNode::AppliedFilter(node) => node.child.prepare_for_measure(renderer),
            RenderNode::ViewEffect(node) => {
                node.child.borrow_mut().prepare_for_measure(renderer);
            }
            RenderNode::Container(node) => {
                for child in &mut node.children {
                    child.prepare_for_measure(renderer);
                }
            }
            RenderNode::Scroll(node) => node.child.prepare_for_measure(renderer),
            RenderNode::Collection(node) => {
                for entry in &mut node.entries {
                    entry.node.prepare_for_measure(renderer);
                }
            }
            RenderNode::LazyStack(node) => {
                node.item_cache.borrow_mut().prepare_for_measure(renderer)
            }
            RenderNode::Color(_)
            | RenderNode::Text(_)
            | RenderNode::SceneView(_)
            | RenderNode::GpuSurface(_) => {}
        }
    }

    /// Re-measure and re-place this subtree, caching each container's child
    /// frames. Run on build and whenever a geometry-affecting input changes.
    pub(crate) fn layout(
        &mut self,
        renderer: &mut HydrolysisRenderer,
        env: &Environment,
        proposal: ProposalSize,
        size: Size,
    ) {
        // The selected proposal and resolved size are distinct layout inputs.
        // Transparent wrappers preserve both without reconstructing an offer.
        let theme = renderer.theme();
        match self {
            RenderNode::Container(container) => {
                let placements = {
                    let cell = RefCell::new(&mut renderer.state);
                    let subs: Vec<NodeSubView> = container
                        .children
                        .iter()
                        .map(|child| NodeSubView::new(child, &cell, env, &theme))
                        .collect();
                    let refs: Vec<&dyn SubView> =
                        subs.iter().map(|sub| sub as &dyn SubView).collect();
                    container
                        .layout
                        .place(Rect::from_size(size), proposal, &refs)
                };
                for (child, placement) in container.children.iter_mut().zip(&placements) {
                    child.layout(renderer, env, placement.proposal, *placement.frame.size());
                }
                container.placed = placements
                    .into_iter()
                    .map(|placement| placement.frame)
                    .collect();
            }
            // Transform/opacity wrappers are layout-transparent: the child lays out
            // at the same concrete size as the wrapper.
            RenderNode::Opacity(node) => node.child.layout(renderer, env, proposal, size),
            RenderNode::Scale(node) => node.child.layout(renderer, env, proposal, size),
            RenderNode::Rotation(node) => node.child.layout(renderer, env, proposal, size),
            RenderNode::Offset(node) => node.child.layout(renderer, env, proposal, size),
            RenderNode::Retain(node) => node.child.layout(renderer, env, proposal, size),
            RenderNode::Env(node) => {
                let node_env = node.env.clone();
                node.child.layout(renderer, &node_env, proposal, size);
            }
            // Layout-transparent: the child lays out at the same concrete size,
            // under the wrapper's scoped environment.
            RenderNode::Wrapper(node) => {
                let node_env = node.env.clone();
                node.child.layout(renderer, &node_env, proposal, size);
            }
            RenderNode::Dynamic(node) => {
                node.child
                    .borrow_mut()
                    .layout(renderer, env, proposal, size);
            }
            RenderNode::Scroll(node) => {
                let child_proposal = match node.axis {
                    ScrollAxis::Horizontal => ProposalSize::new(None, Some(size.height)),
                    ScrollAxis::Vertical => ProposalSize::new(Some(size.width), None),
                    ScrollAxis::All => ProposalSize::UNSPECIFIED,
                    _ => panic!("hydrolysis render tree: unsupported scroll axis"),
                };
                let intrinsic = node
                    .child
                    .measure(&mut renderer.state, env, &theme, child_proposal)
                    .size;
                let content_size = match node.axis {
                    ScrollAxis::Horizontal => {
                        Size::new(intrinsic.width.max(size.width), size.height)
                    }
                    ScrollAxis::Vertical => {
                        Size::new(size.width, intrinsic.height.max(size.height))
                    }
                    ScrollAxis::All => Size::new(
                        intrinsic.width.max(size.width),
                        intrinsic.height.max(size.height),
                    ),
                    _ => panic!("hydrolysis render tree: unsupported scroll axis"),
                };
                node.child
                    .layout(renderer, env, child_proposal, content_size);
                let handle = if let Some(handle) = node.handle.borrow_mut().as_mut() {
                    handle.rebind(
                        node.axis,
                        f64::from(size.width),
                        f64::from(size.height),
                        f64::from(content_size.width),
                        f64::from(content_size.height),
                    )
                } else {
                    ScrollHandle::new(
                        node.axis,
                        f64::from(size.width),
                        f64::from(size.height),
                        f64::from(content_size.width),
                        f64::from(content_size.height),
                    )
                };
                if let Some(controller) = &node.controller {
                    let generation = renderer.read_signal(&controller.generation());
                    if generation != node.applied_scroll_generation.get() {
                        let target = renderer.read_signal(&controller.target());
                        let _ = handle.scroll_to(f64::from(target.x), f64::from(target.y));
                        node.applied_scroll_generation.set(generation);
                    }
                }
                *node.handle.borrow_mut() = Some(handle);
                node.content_size = content_size;
                node.viewport = size;
            }
            RenderNode::Collection(node) => node.layout(renderer, proposal, size),
            // Effects preserve the selected proposal even when their bounds stay equal.
            RenderNode::ViewEffect(node) => {
                let node_env = node.env.clone();
                node.child
                    .borrow_mut()
                    .layout(renderer, &node_env, proposal, size);
            }
            RenderNode::AppliedFilter(node) => {
                let node_env = node.env.clone();
                node.child.layout(renderer, &node_env, proposal, size);
            }
            // A lazy stack places its items lazily at flush (offset-dependent); a
            // widget leaf or GpuSurface renders itself at flush from `ctx.bounds`.
            // Nothing to pre-lay-out for any of these.
            RenderNode::Color(_)
            | RenderNode::Text(_)
            | RenderNode::SceneView(_)
            | RenderNode::GpuSurface(_)
            | RenderNode::LazyStack(_)
            | RenderNode::Widget(_) => {}
        }
    }
}
