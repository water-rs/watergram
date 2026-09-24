//! Text shaping and measurement.
//!
//! Shaping (parley layout) is by far the heaviest part of measuring a text leaf —
//! tens of microseconds against the tens of nanoseconds every other leaf costs — so
//! it is the one measure that genuinely must be cached. This module splits the work
//! into two phases to make that cache correct:
//!
//! 1. **Resolve** ([`resolve_text_layout_input`]) reads the [`Environment`] and
//!    reactive signals to turn a [`StyledStr`] into a self-contained
//!    [`ResolvedTextLayoutInput`], capturing everything that can affect shaping.
//! 2. **Shape** ([`TextMeasureService::shape`]) turns that input into a
//!    [`parley::Layout`]. It reads no ambient state, so it is a pure function of
//!    the resolved input and can be memoized on that input's content identity.
//!
//! The service owns the loaded fonts plus that layout cache, so the render path and
//! measurement shape identical text through one cache.

use super::*;
use core::hash::{Hash, Hasher};
use core::num::NonZeroUsize;
use core::ops::Range;
use lru::LruCache;
use rustc_hash::FxHasher;
use std::sync::{Arc, Mutex};

/// Upper bound on retained shaped layouts.
///
/// Shaping is keyed by content *and* fitted width, so one text leaf mints an
/// entry per distinct proposal a container probes it with, and reactive text — a
/// clock, a counter, a field's value — mints one per distinct string. Unbounded,
/// the cache grows for the life of the process. This holds several times a dense
/// screen's working set, so steady-state UI still never misses, while a long
/// session evicts what it has stopped drawing.
const TEXT_LAYOUT_CACHE_CAPACITY: usize = 4096;

/// Thread-safe text shaping service shared by the render path and layout
/// measurement. Cheaply cloneable shaping scratch is pooled so each worker
/// reuses a [`parley::FontContext`] carrying the registered resource fonts.
pub(crate) struct TextMeasureService {
    /// Fonts registered at startup; the clone source for shaping scratch.
    /// Mutated only during single-threaded font registration via
    /// [`Self::fonts_mut`], read-only afterward.
    fonts: parley::FontContext,
    /// Shared layout cache — one source of truth for the render path and
    /// measurement. Bounded at [`TEXT_LAYOUT_CACHE_CAPACITY`], evicting
    /// least-recently-shaped entries. Layouts are shared as [`Arc`] so a hit
    /// hands out a handle instead of copying the glyph runs.
    cache: Mutex<LruCache<TextLayoutCacheKey, Arc<parley::Layout<[u8; 4]>>>>,
    /// Reusable `(FontContext, LayoutContext)` shaping scratch, checked out per
    /// shape call. Built once as a clone of [`Self::fonts`] (carrying the
    /// registered resource fonts) plus a fresh layout context, then returned for
    /// the next call rather than rebuilt.
    scratch: Mutex<Option<TextShapingScratch>>,
    /// Encoded glyph scenes, keyed by the shaped layout's identity plus the
    /// draw-time line limit. The render path redraws every text leaf on every
    /// frame (whole-scene re-encode), and re-emitting glyph runs — font
    /// resolution, per-glyph iteration, run encoding — dominates a text leaf's
    /// flush cost. A fragment is encoded once at the local origin and appended
    /// under the frame's transform, so a scrolled or animated frame pays one
    /// encoding copy per text instead of a full glyph-run walk.
    scene_cache: Mutex<LruCache<TextSceneCacheKey, Arc<vello::Scene>>>,
}

/// Cache identity for an encoded glyph scene: the shaped layout it draws plus
/// the line limit and tail truncation applied while drawing.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TextSceneCacheKey {
    layout: TextLayoutCacheKey,
    max_lines: Option<usize>,
    tail_ellipsis: bool,
}

/// Owned, mutable shaping scratch for one in-flight shape call.
struct TextShapingScratch {
    font_cx: parley::FontContext,
    layout_cx: parley::LayoutContext<[u8; 4]>,
}

impl TextMeasureService {
    pub(crate) fn new() -> Self {
        Self {
            fonts: parley::FontContext::new(),
            cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(TEXT_LAYOUT_CACHE_CAPACITY)
                    .expect("text layout cache capacity must be non-zero"),
            )),
            scratch: Mutex::new(None),
            scene_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(TEXT_LAYOUT_CACHE_CAPACITY)
                    .expect("text scene cache capacity must be non-zero"),
            )),
        }
    }

    /// Mutable access to the registered fonts, for startup font registration.
    ///
    /// Registering fonts invalidates any shaping scratch and cached layouts
    /// produced before the registration, so text shaped before its fonts were
    /// installed can never be reused. Requires unique ownership of the service,
    /// which holds during single-threaded setup before any subview clones it.
    pub(crate) fn fonts_mut(&mut self) -> &mut parley::FontContext {
        self.cache
            .get_mut()
            .expect("text layout cache mutex must not be poisoned")
            .clear();
        self.scene_cache
            .get_mut()
            .expect("text scene cache mutex must not be poisoned")
            .clear();
        *self
            .scratch
            .get_mut()
            .expect("text shaping scratch mutex must not be poisoned") = None;
        &mut self.fonts
    }

    /// Shape `input` into a parley layout, reusing the shared cache.
    ///
    /// The layout is shared rather than copied: every consumer only reads it,
    /// and copying glyph runs on every probe is the bulk of a cache hit's cost.
    pub(crate) fn shape(
        &self,
        input: &ResolvedTextLayoutInput,
        max_width: Option<f32>,
    ) -> Arc<parley::Layout<[u8; 4]>> {
        if input.plain.is_empty() {
            return Arc::new(parley::Layout::new());
        }

        let cache_key = input.cache_key(max_width);
        if let Some(layout) = self
            .cache
            .lock()
            .expect("text layout cache mutex must not be poisoned")
            .get(&cache_key)
        {
            return Arc::clone(layout);
        }

        let mut scratch = self.checkout_scratch();
        let layout = Arc::new(build_parley_layout(&mut scratch, input, max_width));
        self.return_scratch(scratch);

        self.cache
            .lock()
            .expect("text layout cache mutex must not be poisoned")
            .put(cache_key, Arc::clone(&layout));
        layout
    }

    /// Shape `input` with at most `max_lines` laid-out lines, truncating the
    /// last allowed line to a trailing ellipsis when the text does not fit.
    /// The truncated line is respelled — its kept clusters plus the marker —
    /// and re-shaped, so the layout's widest line is the honest laid-out
    /// width the leaf reports, and the drawn line is cached and encoded
    /// through the same paths as any other.
    pub(crate) fn shape_limited(
        &self,
        input: &ResolvedTextLayoutInput,
        max_width: Option<f32>,
        max_lines: Option<usize>,
    ) -> Arc<parley::Layout<[u8; 4]>> {
        let mut layout = self.shape(input, max_width);
        let Some(limit) = max_lines.filter(|limit| *limit > 0) else {
            return layout;
        };
        if !needs_tail_truncation(&layout, limit) {
            return layout;
        }

        let ellipsis_advance = self.ellipsis_advance(input, &layout, limit);
        let mut text = input.plain.clone();
        let mut spans = input.spans.clone();
        while let Some(cut) = truncate_layout_tail(&layout, &text, &spans, limit, ellipsis_advance)
        {
            if cut.0 == text {
                break;
            }
            text = cut.0;
            spans = cut.1;
            layout = self.shape(&input.respell(text.clone(), spans.clone()), max_width);
            if !needs_tail_truncation(&layout, limit) {
                break;
            }
        }
        layout
    }

    /// The marker's advance in the style at the tail of the last allowed line,
    /// so the truncation cut can reserve room for it.
    fn ellipsis_advance(
        &self,
        input: &ResolvedTextLayoutInput,
        layout: &parley::Layout<[u8; 4]>,
        limit: usize,
    ) -> f32 {
        let Some(line) = layout.get(layout.len().min(limit) - 1) else {
            return 0.0;
        };
        let end = line.text_range().end;
        let spans = input
            .spans
            .iter()
            .rev()
            .find(|(range, _)| range.start < end)
            .map_or_else(Vec::new, |(_, style)| {
                vec![(0..TAIL_ELLIPSIS.len_utf8(), style.clone())]
            });
        self.shape(&input.respell(String::from(TAIL_ELLIPSIS), spans), None)
            .get(0)
            .map_or(0.0, |line| line.metrics().advance)
    }

    /// The encoded glyph scene for `input` at `max_width`, drawn with its
    /// `tail` treatment, at the local origin (identity transform). A miss
    /// shapes through [`Self::shape`] — or [`Self::shape_limited`] for
    /// [`TailMark::Ellipsis`] — and encodes once via `encode`; a hit returns
    /// the shared fragment so the caller only pays a transformed append into
    /// the frame's scene.
    pub(crate) fn glyph_scene_with(
        &self,
        input: &ResolvedTextLayoutInput,
        max_width: Option<f32>,
        tail: TailMark,
        encode: impl FnOnce(&parley::Layout<[u8; 4]>, &mut vello::Scene),
    ) -> Arc<vello::Scene> {
        let (max_lines, tail_ellipsis) = tail.parts();
        let key = TextSceneCacheKey {
            layout: input.cache_key(max_width),
            max_lines,
            tail_ellipsis,
        };
        if let Some(scene) = self
            .scene_cache
            .lock()
            .expect("text scene cache mutex must not be poisoned")
            .get(&key)
        {
            return Arc::clone(scene);
        }
        let layout = if tail_ellipsis {
            self.shape_limited(input, max_width, max_lines)
        } else {
            self.shape(input, max_width)
        };
        let mut scene = vello::Scene::new();
        encode(&layout, &mut scene);
        let scene = Arc::new(scene);
        self.scene_cache
            .lock()
            .expect("text scene cache mutex must not be poisoned")
            .put(key, Arc::clone(&scene));
        scene
    }

    fn checkout_scratch(&self) -> TextShapingScratch {
        self.scratch
            .lock()
            .expect("text shaping scratch mutex must not be poisoned")
            .take()
            .unwrap_or_else(|| TextShapingScratch {
                font_cx: self.fonts.clone(),
                layout_cx: parley::LayoutContext::new(),
            })
    }

    fn return_scratch(&self, scratch: TextShapingScratch) {
        *self
            .scratch
            .lock()
            .expect("text shaping scratch mutex must not be poisoned") = Some(scratch);
    }
}

impl core::fmt::Debug for TextMeasureService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TextMeasureService").finish_non_exhaustive()
    }
}

/// A fully-resolved, self-contained text layout description.
///
/// Every reactive input (signal values, [`Str`]-backed font families) has been
/// read out and projected into owned data, so shaping this input is a pure
/// function of it — which is what makes the content-keyed cache correct.
pub(crate) struct ResolvedTextLayoutInput {
    plain: String,
    spans: Vec<(Range<usize>, ResolvedTextStyleSpec)>,
    default_font: ResolvedFontSpec,
    default_brush: [u8; 4],
    locale: String,
    alignment: HorizontalAlignment,
    right_to_left: bool,
    /// This input's width-independent cache identity, built with the input.
    identity: Arc<TextLayoutIdentity>,
}

impl ResolvedTextLayoutInput {
    fn cache_key(&self, max_width: Option<f32>) -> TextLayoutCacheKey {
        TextLayoutCacheKey {
            identity: Arc::clone(&self.identity),
            max_width: max_width.map(f32::to_bits),
        }
    }

    /// The same shaping defaults respelled over different text — how the
    /// ellipsis probe and each truncated re-shape mint their inputs.
    fn respell(&self, plain: String, spans: Vec<(Range<usize>, ResolvedTextStyleSpec)>) -> Self {
        let alignment_id = self.alignment.stable_id();
        let identity = Arc::new(TextLayoutIdentity::new(
            plain.clone(),
            spans
                .iter()
                .map(|(range, style)| span_cache_key(range, style))
                .collect(),
            text_layout_font_cache_key(&self.default_font),
            self.default_brush,
            self.locale.clone(),
            TextLayoutAlignmentCacheKey {
                low: alignment_id.low(),
                high: alignment_id.high(),
                right_to_left: self.right_to_left,
            },
        ));
        Self {
            plain,
            spans,
            default_font: self.default_font.clone(),
            default_brush: self.default_brush,
            locale: self.locale.clone(),
            alignment: self.alignment,
            right_to_left: self.right_to_left,
            identity,
        }
    }
}

/// `Send` projection of a resolved font (the `!Send` [`Str`] family is copied
/// into an owned [`String`]).
#[derive(Clone)]
struct ResolvedFontSpec {
    size: f32,
    weight: TextFontWeight,
    line_height: Option<f32>,
    letter_spacing: f32,
    family: Option<String>,
}

/// `Send` projection of a resolved text run style.
#[derive(Clone)]
struct ResolvedTextStyleSpec {
    font: ResolvedFontSpec,
    foreground: Option<[u8; 4]>,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

/// Resolve a [`StyledStr`] into a `Send` [`ResolvedTextLayoutInput`].
///
/// Reads the environment and reactive signals, so it must run on the main
/// thread. The returned value can then be shaped on any thread.
pub(crate) fn resolve_text_layout_input(
    styled: &StyledStr,
    alignment: HorizontalAlignment,
    env: &Environment,
) -> ResolvedTextLayoutInput {
    let mut plain = String::new();
    let mut spans = Vec::with_capacity(styled.chunks().len());
    for (chunk, style) in styled.chunks() {
        let start = plain.len();
        plain.push_str(chunk.as_str());
        let end = plain.len();
        spans.push((start..end, resolve_text_style(style, env)));
    }

    let default_font = font_spec(&waterui_text::font::Font::default().resolve(env).get());
    let default_brush = default_text_brush(env);
    let locale = text_layout_locale(env);
    let right_to_left = waterui_core::layout::layout_direction(env)
        .get()
        .is_right_to_left();
    let alignment_id = alignment.stable_id();
    let identity = Arc::new(TextLayoutIdentity::new(
        plain.clone(),
        spans
            .iter()
            .map(|(range, style)| span_cache_key(range, style))
            .collect(),
        text_layout_font_cache_key(&default_font),
        default_brush,
        locale.clone(),
        TextLayoutAlignmentCacheKey {
            low: alignment_id.low(),
            high: alignment_id.high(),
            right_to_left,
        },
    ));
    ResolvedTextLayoutInput {
        plain,
        spans,
        default_font,
        default_brush,
        locale,
        alignment,
        right_to_left,
        identity,
    }
}

fn span_cache_key(range: &Range<usize>, style: &ResolvedTextStyleSpec) -> TextLayoutSpanCacheKey {
    TextLayoutSpanCacheKey {
        start: range.start,
        end: range.end,
        font: text_layout_font_cache_key(&style.font),
        foreground: style.foreground,
        italic: style.italic,
        underline: style.underline,
        strikethrough: style.strikethrough,
    }
}

fn font_spec(font: &waterui_text::font::ResolvedFont) -> ResolvedFontSpec {
    ResolvedFontSpec {
        size: font.size,
        weight: font.weight,
        line_height: font.line_height,
        letter_spacing: font.letter_spacing,
        family: font.family.as_deref().map(String::from),
    }
}

fn resolve_text_style(style: &TextStyle, env: &Environment) -> ResolvedTextStyleSpec {
    ResolvedTextStyleSpec {
        font: font_spec(&style.font.resolve(env).get()),
        foreground: style
            .foreground
            .clone()
            .map(|color| resolved_color_to_rgba8(color.resolve(env).get())),
        italic: style.italic,
        underline: style.underline,
        strikethrough: style.strikethrough,
    }
}

fn default_text_brush(env: &Environment) -> [u8; 4] {
    let color = theme::installed_color_signal::<theme::color::Foreground>(env).map_or_else(
        || Color::srgb(0, 0, 0).resolve(env).get(),
        |signal| signal.get(),
    );
    resolved_color_to_rgba8(color)
}

fn build_parley_layout(
    scratch: &mut TextShapingScratch,
    input: &ResolvedTextLayoutInput,
    max_width: Option<f32>,
) -> parley::Layout<[u8; 4]> {
    let mut builder =
        scratch
            .layout_cx
            .ranged_builder(&mut scratch.font_cx, &input.plain, 1.0, true);
    builder.push_default(parley::StyleProperty::Brush(input.default_brush));
    builder.push_default(parley::StyleProperty::FontSize(input.default_font.size));
    builder.push_default(parley::StyleProperty::FontWeight(parley_font_weight(
        input.default_font.weight,
    )));
    if let Some(line_height) = input.default_font.line_height {
        builder.push_default(parley::StyleProperty::LineHeight(
            parley::LineHeight::Absolute(line_height),
        ));
    }
    builder.push_default(parley::StyleProperty::LetterSpacing(
        input.default_font.letter_spacing,
    ));
    let locale = input
        .locale
        .parse::<parley::Language>()
        .unwrap_or_else(|error| {
            panic!(
                "WaterUI locale `{}` is not a valid BCP 47 language tag: {error}",
                input.locale
            )
        });
    builder.push_default(parley::StyleProperty::Locale(Some(locale)));
    builder.push_default(parley::StyleProperty::FontFamily(font_family(
        input.default_font.family.as_deref(),
    )));

    for (range, style) in &input.spans {
        push_text_style(&mut builder, style, range.clone());
    }

    let mut layout = builder.build(&input.plain);
    layout.break_all_lines(max_width);
    layout.align(
        parley_alignment(input.alignment, input.right_to_left),
        parley::AlignmentOptions::default(),
    );
    layout
}

fn push_text_style(
    builder: &mut parley::RangedBuilder<'_, [u8; 4]>,
    style: &ResolvedTextStyleSpec,
    range: Range<usize>,
) {
    builder.push(
        parley::StyleProperty::FontSize(style.font.size),
        range.clone(),
    );
    builder.push(
        parley::StyleProperty::FontWeight(parley_font_weight(style.font.weight)),
        range.clone(),
    );
    if let Some(line_height) = style.font.line_height {
        builder.push(
            parley::StyleProperty::LineHeight(parley::LineHeight::Absolute(line_height)),
            range.clone(),
        );
    }
    builder.push(
        parley::StyleProperty::LetterSpacing(style.font.letter_spacing),
        range.clone(),
    );
    if let Some(family) = &style.font.family {
        builder.push(
            parley::StyleProperty::FontFamily(font_family(Some(family.as_str()))),
            range.clone(),
        );
    }
    builder.push(
        parley::StyleProperty::FontStyle(if style.italic {
            parley::FontStyle::Italic
        } else {
            parley::FontStyle::Normal
        }),
        range.clone(),
    );
    builder.push(
        parley::StyleProperty::Underline(style.underline),
        range.clone(),
    );
    builder.push(
        parley::StyleProperty::Strikethrough(style.strikethrough),
        range.clone(),
    );
    if let Some(color) = style.foreground {
        builder.push(parley::StyleProperty::Brush(color), range);
    }
}

fn font_family(family: Option<&str>) -> parley::FontFamily<'static> {
    family.map_or_else(
        || parley::style::GenericFamily::SansSerif.into(),
        |family| parley::FontFamily::Source(std::borrow::Cow::Owned(family.to_string())),
    )
}

fn text_layout_locale(env: &Environment) -> String {
    waterui_locale::locale_binding(env).get().canonical_tag()
}

/// The marker a truncated line's tail is cut for.
const TAIL_ELLIPSIS: char = '\u{2026}';

/// How a drawn text treats the tail that a line limit cuts off.
#[derive(Clone, Copy)]
pub(crate) enum TailMark {
    /// No line limit — every laid-out line draws.
    None,
    /// At most the given lines draw; the rest are clipped.
    Clip(usize),
    /// At most the given lines draw and the last carries a trailing
    /// ellipsis — what a `Text` leaf's `line_limit` means.
    Ellipsis(usize),
}

impl TailMark {
    pub(crate) const fn parts(self) -> (Option<usize>, bool) {
        match self {
            Self::None => (None, false),
            Self::Clip(limit) => (Some(limit), false),
            Self::Ellipsis(limit) => (Some(limit), true),
        }
    }
}

/// Whether the laid-out text needs its tail truncated: more lines than the
/// limit allows, or a last visible line that overruns the bound — which is
/// how a single unbreakable cluster presents. Lines a plain wrap produced
/// stay inside the bound, so a small epsilon keeps benign rounding from
/// declaring a truncation.
fn needs_tail_truncation(layout: &parley::Layout<[u8; 4]>, limit: usize) -> bool {
    if layout.is_empty() {
        return false;
    }
    if layout.len() > limit {
        return true;
    }
    layout
        .get(layout.len() - 1)
        .is_some_and(|line| line.metrics().advance - layout.layout_max_advance() > 0.01)
}

/// The text and styled spans a tail cut leaves for the re-shape.
type RespelledTail = (String, Vec<(Range<usize>, ResolvedTextStyleSpec)>);

/// The `text`/`spans` pair with `text`'s last allowed line shortened to make
/// room for — and end in — the ellipsis marker. `None` when the text is
/// already exactly that, which is how the re-shape loop knows it cannot
/// shorten further.
fn truncate_layout_tail(
    layout: &parley::Layout<[u8; 4]>,
    text: &str,
    spans: &[(Range<usize>, ResolvedTextStyleSpec)],
    limit: usize,
    ellipsis_advance: f32,
) -> Option<RespelledTail> {
    let last_index = layout.len().min(limit) - 1;
    let line = layout.get(last_index)?;
    let bound = layout.layout_max_advance();
    let line_start = line.text_range().start;

    // The tail is every cluster from the last allowed line on: the truncated
    // line refills its bound from content a wrap placed on later lines. Byte
    // order cuts the *logical* tail, which is the edge the ellipsis belongs on
    // for either direction. A hard break ends the kept prefix — the marker
    // replaces the paragraph's tail, never the break itself — and a marker an
    // earlier pass appended is skipped so re-truncating only ever removes
    // real content.
    let marker_start = text.len().saturating_sub(TAIL_ELLIPSIS.len_utf8());
    let has_marker = text.ends_with(TAIL_ELLIPSIS);
    let mut clusters: Vec<(Range<usize>, f32)> = Vec::new();
    'lines: for line_index in last_index..layout.len() {
        let Some(tail_line) = layout.get(line_index) else {
            break;
        };
        for run in tail_line.runs() {
            for cluster in run.clusters() {
                if cluster.is_hard_line_break() {
                    break 'lines;
                }
                let range = cluster.text_range();
                if has_marker && range.end > marker_start {
                    continue;
                }
                clusters.push((range, cluster.advance()));
            }
        }
    }
    clusters.sort_by_key(|(range, _)| range.start);

    let mut used = 0.0_f32;
    let mut cut = line_start;
    for (range, advance) in clusters {
        if used + advance + ellipsis_advance > bound {
            break;
        }
        used += advance;
        cut = range.end;
    }

    let kept_end = line_start + text[line_start..cut].trim_end().len();
    let mut truncated = String::with_capacity(kept_end + TAIL_ELLIPSIS.len_utf8());
    truncated.push_str(&text[..line_start]);
    truncated.push_str(&text[line_start..kept_end]);
    truncated.push(TAIL_ELLIPSIS);
    if truncated == text {
        return None;
    }
    let truncated_len = truncated.len();
    Some((truncated, truncate_spans(spans, kept_end, truncated_len)))
}

/// Span ranges for the truncated text: clamped to what the cut kept, then the
/// span covering the cut extended to dress the marker too — the way a native
/// truncation takes the last visible run's style.
fn truncate_spans(
    spans: &[(Range<usize>, ResolvedTextStyleSpec)],
    kept_end: usize,
    truncated_len: usize,
) -> Vec<(Range<usize>, ResolvedTextStyleSpec)> {
    let mut truncated: Vec<_> = spans
        .iter()
        .filter_map(|(range, style)| {
            let start = range.start.min(kept_end);
            let end = range.end.min(kept_end);
            (start < end).then(|| (start..end, style.clone()))
        })
        .collect();
    if let Some((range, _)) = truncated
        .iter_mut()
        .rev()
        .find(|(range, _)| range.end == kept_end)
    {
        range.end = truncated_len;
    }
    truncated
}

/// Compute view dimensions (size plus first/last baselines) from a shaped
/// layout. Pure; safe to call on any thread.
pub(crate) fn text_dimensions_from_layout(
    layout: &parley::Layout<[u8; 4]>,
    max_lines: Option<usize>,
) -> ViewDimensions {
    if layout.is_empty() {
        return ViewDimensions::new(LayoutSize::zero());
    }

    let mut width = 0.0_f32;
    let mut height = 0.0_f32;
    let mut first_baseline = None;
    let mut last_baseline = None;

    for (index, line) in layout.lines().enumerate() {
        if max_lines.is_some_and(|limit| index >= limit) {
            break;
        }
        let metrics = line.metrics();
        width = width.max(metrics.advance);
        height += metrics.line_height;
        if first_baseline.is_none() {
            first_baseline = Some(metrics.baseline);
        }
        last_baseline = Some(metrics.baseline);
    }

    let mut dimensions = ViewDimensions::new(LayoutSize::new(width, height));
    if let Some(first_baseline) = first_baseline {
        dimensions.set_vertical(VerticalAlignment::FirstBaseline, first_baseline);
    }
    if let Some(last_baseline) = last_baseline {
        dimensions.set_vertical(VerticalAlignment::LastBaseline, last_baseline);
    }
    dimensions
}

fn text_layout_font_cache_key(font: &ResolvedFontSpec) -> TextLayoutFontCacheKey {
    TextLayoutFontCacheKey {
        size: font.size.to_bits(),
        weight: text_font_weight_cache_key(font.weight),
        line_height: font.line_height.map(f32::to_bits),
        letter_spacing: font.letter_spacing.to_bits(),
        family: font.family.clone(),
    }
}

const fn text_font_weight_cache_key(weight: TextFontWeight) -> u16 {
    match weight {
        TextFontWeight::Thin => 100,
        TextFontWeight::UltraLight => 200,
        TextFontWeight::Light => 300,
        TextFontWeight::Normal => 400,
        TextFontWeight::Medium => 500,
        TextFontWeight::SemiBold => 600,
        TextFontWeight::Bold => 700,
        TextFontWeight::UltraBold => 800,
        TextFontWeight::Black => 900,
    }
}

/// Everything that identifies a shaped layout except the width it is fitted to.
///
/// Built once per [`ResolvedTextLayoutInput`] and shared by every width probe of
/// that text: a container measuring one leaf against several proposals then
/// allocates this once instead of once per probe. All fields are owned and
/// `Send`, so the cache can be shared across threads.
#[derive(Debug, Eq)]
struct TextLayoutIdentity {
    text: String,
    spans: Vec<TextLayoutSpanCacheKey>,
    default_font: TextLayoutFontCacheKey,
    default_brush: [u8; 4],
    locale: String,
    alignment: TextLayoutAlignmentCacheKey,
    /// Hash of every field above, computed once at construction. [`Hash`] writes
    /// only this, so a cache probe never re-walks the text and its spans;
    /// equality still compares the fields, so a collision cannot return the
    /// wrong layout.
    hash: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TextLayoutAlignmentCacheKey {
    low: u64,
    high: u64,
    right_to_left: bool,
}

impl TextLayoutIdentity {
    fn new(
        text: String,
        spans: Vec<TextLayoutSpanCacheKey>,
        default_font: TextLayoutFontCacheKey,
        default_brush: [u8; 4],
        locale: String,
        alignment: TextLayoutAlignmentCacheKey,
    ) -> Self {
        let mut hasher = FxHasher::default();
        text.hash(&mut hasher);
        spans.hash(&mut hasher);
        default_font.hash(&mut hasher);
        default_brush.hash(&mut hasher);
        locale.hash(&mut hasher);
        alignment.hash(&mut hasher);
        Self {
            text,
            spans,
            default_font,
            default_brush,
            locale,
            alignment,
            hash: hasher.finish(),
        }
    }
}

impl PartialEq for TextLayoutIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.text == other.text
            && self.spans == other.spans
            && self.default_font == other.default_font
            && self.default_brush == other.default_brush
            && self.locale == other.locale
            && self.alignment == other.alignment
    }
}

impl Hash for TextLayoutIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

/// Cache key for a shaped [`parley::Layout`]: what to shape, and how wide.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TextLayoutCacheKey {
    identity: Arc<TextLayoutIdentity>,
    max_width: Option<u32>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TextLayoutSpanCacheKey {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) font: TextLayoutFontCacheKey,
    pub(crate) foreground: Option<[u8; 4]>,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strikethrough: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TextLayoutFontCacheKey {
    pub(crate) size: u32,
    pub(crate) weight: u16,
    pub(crate) line_height: Option<u32>,
    pub(crate) letter_spacing: u32,
    pub(crate) family: Option<String>,
}

#[cfg(test)]
mod font_family_tests {
    use super::{font_family, text_layout_locale};
    use parley::style::GenericFamily;
    use waterui_core::Environment;
    use waterui_locale::locales;

    #[test]
    fn explicit_family_list_is_preserved_for_parley_css_parsing() {
        let family = font_family(Some("Roboto, Noto Sans CJK SC, sans-serif"));

        assert_eq!(
            family,
            parley::FontFamily::Source("Roboto, Noto Sans CJK SC, sans-serif".into())
        );
    }

    #[test]
    fn missing_family_uses_sans_serif_generic() {
        assert_eq!(
            font_family(None),
            parley::FontFamily::from(GenericFamily::SansSerif)
        );
    }

    #[test]
    fn text_layout_locale_uses_environment_locale() {
        let mut env = Environment::new();
        env.insert(locales::ZH_TW);

        assert_eq!(text_layout_locale(&env), "zh-TW");
    }
}

#[cfg(test)]
mod truncation_tests {
    use super::*;
    use crate::renderer::tests::test_environment;

    fn test_input(env: &Environment, text: &'static str) -> ResolvedTextLayoutInput {
        resolve_text_layout_input(&StyledStr::plain(text), HorizontalAlignment::Leading, env)
    }

    /// The logically-last cluster's character on a line — where the marker
    /// sits after tail truncation.
    fn last_cluster_char(line: &parley::Line<'_, [u8; 4]>) -> Option<char> {
        let mut tail = None;
        for run in line.runs() {
            for cluster in run.clusters() {
                tail = Some(cluster.source_char());
            }
        }
        tail
    }

    #[test]
    fn a_truncated_line_ends_in_an_ellipsis_inside_its_bound() {
        let env = test_environment();
        let service = TextMeasureService::new();
        let input = test_input(
            &env,
            "a preview long enough that a single line cannot hold it",
        );

        let layout = service.shape_limited(&input, Some(60.0), Some(1));

        assert_eq!(layout.len(), 1, "a one-line limit lays out one line");
        let line = layout.get(0).expect("one line");
        assert!(
            line.metrics().advance <= 60.0,
            "the truncated line stays inside its bound"
        );
        assert!(
            line.metrics().advance > 45.0,
            "the cut fills the bound to glyph granularity"
        );
        assert_eq!(
            last_cluster_char(&line),
            Some(TAIL_ELLIPSIS),
            "the drawn line ends in an ellipsis"
        );
    }

    #[test]
    fn a_multiline_limit_carries_the_ellipsis_on_the_last_line() {
        let env = test_environment();
        let service = TextMeasureService::new();
        let input = test_input(
            &env,
            "a preview long enough that it wraps well past the two lines it may show",
        );

        let layout = service.shape_limited(&input, Some(80.0), Some(2));

        assert_eq!(
            layout.len(),
            2,
            "the truncated text keeps its allowed lines"
        );
        let last = layout.get(1).expect("last line");
        assert!(last.metrics().advance <= 80.0);
        assert_eq!(last_cluster_char(&last), Some(TAIL_ELLIPSIS));
        let first = layout.get(0).expect("first line");
        assert_ne!(
            last_cluster_char(&first),
            Some(TAIL_ELLIPSIS),
            "only the last line carries the marker"
        );
    }

    #[test]
    fn a_text_that_fits_its_limit_is_not_truncated() {
        let env = test_environment();
        let service = TextMeasureService::new();
        let input = test_input(&env, "short");

        let layout = service.shape_limited(&input, Some(200.0), Some(1));

        let line = layout.get(0).expect("one line");
        assert_eq!(last_cluster_char(&line), Some('t'));
    }

    /// The leaf contract (docs/layout-spec.md §6): the answer is the laid-out
    /// width of the truncated line — its kept clusters plus the ellipsis —
    /// never the proposal. The cut packs clusters to glyph granularity, so
    /// the laid-out line sits within one dropped cluster of the bound.
    #[test]
    fn a_truncated_leaf_reports_the_drawn_line() {
        let env = test_environment();
        let mut state = HydroState::default();
        let styled = StyledStr::plain("aaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

        let dimensions = HydrolysisRenderer::measure_text_dimensions(
            &mut state,
            styled,
            HorizontalAlignment::Leading,
            &env,
            Some(100.0),
            Some(1),
        );

        assert!(
            dimensions.size.width <= 100.0,
            "the laid-out line stays inside its bound"
        );
        assert!(
            dimensions.size.width > 85.0,
            "the laid-out line fills the bound to within a cluster"
        );
    }

    /// A tail the line count alone cuts — the kept line never reached the
    /// bound — reports its drawn extent, not the bound.
    #[test]
    fn a_line_count_truncation_reports_the_drawn_extent() {
        let env = test_environment();
        let mut state = HydroState::default();
        let styled = StyledStr::plain("a\nb\nc");

        let dimensions = HydrolysisRenderer::measure_text_dimensions(
            &mut state,
            styled,
            HorizontalAlignment::Leading,
            &env,
            Some(200.0),
            Some(1),
        );

        assert!(
            dimensions.size.width < 200.0,
            "a bound the text never filled is not reported"
        );
        assert!(dimensions.size.width > 0.0);
    }
}
