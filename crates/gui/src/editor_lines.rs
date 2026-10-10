//! Painting one editor line from a `LineView`.
use crate::images::ImageSlot;
use crate::note_view::{LineKind, LineView, Run, TableCell};
use crate::theme::Theme;
use gpui::{
    canvas, div, img, prelude::*, px, AnyElement, FontWeight, HighlightStyle, Hsla, SharedString,
    StyledText, TextLayout,
};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// Vim Normal and Visual modes.
    Block,
    /// Insert mode and standard editing.
    Bar,
}

pub struct LineStyle<'a> {
    pub theme: &'a Theme,
    pub sans: &'a SharedString,
    pub cursor: CursorShape,
    /// Whether the window has keyboard focus; an unfocused cursor is hollow.
    pub focused: bool,
    /// The decoded image of an `LineKind::Image` line.
    pub image: Option<&'a ImageSlot>,
}

/// Styled text for `runs`, with the selection and a block cursor drawn as
/// highlights. A bar cursor is drawn between two texts by the caller.
fn styled(
    runs: &[Run],
    selection: Option<&Range<usize>>,
    block: Option<usize>,
    s: &LineStyle,
) -> (StyledText, Option<usize>) {
    let t = s.theme;
    let mut text = String::new();
    let mut char_bytes = Vec::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for run in runs {
        let start = text.len();
        for ch in run.text.chars() {
            char_bytes.push(text.len());
            text.push(ch);
        }
        highlights.push((start..text.len(), t.highlight(run.style)));
    }
    if block.is_some_and(|c| c >= char_bytes.len()) {
        char_bytes.push(text.len());
        text.push(' ');
    }
    char_bytes.push(text.len());
    let bytes = |r: Range<usize>| {
        let last = char_bytes.len() - 1;
        char_bytes[r.start.min(last)]..char_bytes[r.end.min(last)]
    };
    // Later highlights override earlier ones; split the base runs so the
    // overlays below always land on whole runs.
    let mut overlays: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    if let Some(sel) = selection {
        overlays.push((
            bytes(sel.clone()),
            HighlightStyle {
                background_color: Some(t.blue.opacity(0.32)),
                ..Default::default()
            },
        ));
    }
    let mut bar = None;
    if let (Some(c), true, CursorShape::Bar) = (block, s.focused, s.cursor) {
        // Insert mode: a thin bar painted over the text, not a highlight.
        bar = Some(bytes(c..c).start);
    } else if let Some(c) = block {
        let style = match (s.focused, s.cursor) {
            (true, CursorShape::Block) => HighlightStyle {
                background_color: Some(t.text),
                color: Some(t.bg),
                ..Default::default()
            },
            (true, CursorShape::Bar) => unreachable!(),
            (false, _) => HighlightStyle {
                background_color: Some(t.muted.opacity(0.4)),
                ..Default::default()
            },
        };
        overlays.push((bytes(c..c + 1), style));
    }
    (
        StyledText::new(text).with_highlights(merge(highlights, overlays)),
        bar,
    )
}

/// Base runs with overlays applied on top, as sorted, non-overlapping ranges.
fn merge(
    base: Vec<(Range<usize>, HighlightStyle)>,
    overlays: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = base.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    cuts.extend(overlays.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let mut style = base
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, s)| *s)
            .unwrap_or_default();
        for (r, o) in &overlays {
            if r.start <= a && b <= r.end {
                style = style.highlight(*o);
            }
        }
        out.push((a..b, style));
    }
    out
}

/// The line's text with its cursor and selection, as one run so it wraps.
fn text_with_cursor(line: &LineView, s: &LineStyle, layout: &mut Option<TextLayout>) -> AnyElement {
    let (text, bar) = styled(&line.runs, line.selection.as_ref(), line.cursor, s);
    // The window maps mouse positions to characters through this layout.
    let text_layout = text.layout().clone();
    *layout = Some(text_layout.clone());
    let Some(at) = bar else {
        return text.into_any_element();
    };
    let color = s.theme.blue;
    div()
        .relative()
        .child(text)
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    // The text has been laid out by the time this paints.
                    if let Some(pos) = text_layout.position_for_index(at) {
                        let size = gpui::size(px(2.0), text_layout.line_height());
                        window.paint_quad(gpui::fill(gpui::Bounds::new(pos, size), color));
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

fn ghost(text: &str, t: &Theme) -> impl IntoElement {
    div()
        .pl(px(24.0))
        .text_color(t.amber)
        .child(text.trim().to_string())
}

fn checkbox(checked: bool, t: &Theme) -> impl IntoElement {
    let b = div().size(px(13.0)).rounded(px(3.0)).flex_none();
    if checked {
        b.bg(t.blue)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .text_color(t.bg)
            .child("✓")
    } else {
        b.border_1().border_color(t.muted)
    }
}

fn table_row(cells: &[TableCell], header: bool, t: &Theme) -> AnyElement {
    div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(t.border)
        .when(header, |d| {
            d.text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD)
        })
        .children(cells.iter().enumerate().map(|(i, cell)| {
            let color: Hsla = if header {
                t.muted
            } else if cell.formula {
                t.amber
            } else {
                t.text
            };
            div()
                .flex_1()
                .min_w_0()
                .px(px(12.0))
                .py(px(3.0))
                .text_color(color)
                .when(i > 0, |d| d.flex().justify_end())
                .overflow_hidden()
                .whitespace_nowrap()
                .child(cell.text.clone())
        }))
        .into_any_element()
}

/// An inline image, or what to show while it loads or when it fails.
fn image_box(slot: Option<&ImageSlot>, alt: &str, t: &Theme) -> AnyElement {
    let note = |text: String| {
        div()
            .py(px(4.0))
            .text_color(t.faint)
            .child(text)
            .into_any_element()
    };
    match slot {
        Some(ImageSlot::Ready {
            image,
            width,
            height,
        }) => div()
            .py(px(4.0))
            .child(
                img(image.clone())
                    .w(px(*width))
                    .h(px(*height))
                    .rounded(px(4.0)),
            )
            .into_any_element(),
        Some(ImageSlot::Failed(why)) => note(format!("⚠ {alt}: {why}")),
        _ => note(format!("{alt} …")),
    }
}

/// The body of a line (without gutter). `layout` receives the text layout
/// of lines that have editable text, for mouse hit-testing.
pub fn body(line: &LineView, s: &LineStyle, layout: &mut Option<TextLayout>) -> AnyElement {
    let t = s.theme;
    match &line.kind {
        LineKind::Heading(level) => {
            let size = match level {
                1 => 26.0,
                2 => 18.0,
                _ => 16.0,
            };
            div()
                .pt(px(if *level <= 2 { 8.0 } else { 4.0 }))
                .font_family(s.sans.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(size))
                .line_height(px(size * 1.4))
                .text_color(t.heading)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(div().min_w_0().child(text_with_cursor(line, s, layout)))
                        .when_some(line.ghost.as_deref(), |d, g| d.child(ghost(g, t))),
                )
                .into_any_element()
        }
        LineKind::Checklist { checked } => div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(checkbox(*checked, t))
            .child(
                div()
                    .min_w_0()
                    .when(*checked, |d| d.text_color(t.faint).line_through())
                    .child(text_with_cursor(line, s, layout)),
            )
            .when_some(line.ghost.as_deref(), |d, g| d.child(ghost(g, t)))
            .into_any_element(),
        LineKind::TableRow { cells, header } => table_row(cells, *header, t),
        // The header row already draws the rule under it.
        LineKind::TableDelimiter => div().into_any_element(),
        LineKind::Image { alt, .. } => image_box(s.image, alt, t),
        LineKind::Text => div()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().min_w_0().child(text_with_cursor(line, s, layout)))
                    .when_some(line.ghost.as_deref(), |d, g| d.child(ghost(g, t))),
            )
            .when(line.below.is_some(), |d| d.child(image_box(s.image, "", t)))
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_applies_overlays_on_whole_ranges() {
        let base = vec![(0..4, HighlightStyle::default())];
        let over = HighlightStyle {
            fade_out: Some(0.5),
            ..Default::default()
        };
        let merged = merge(base, vec![(1..2, over)]);
        let ranges: Vec<_> = merged.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, vec![0..1, 1..2, 2..4]);
        assert_eq!(merged[1].1.fade_out, Some(0.5));
    }
}
