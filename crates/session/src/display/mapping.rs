//! Source/display coordinates are Unicode scalar columns, never bytes or cells.
use std::ops::Range;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Before,
    After,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// Identical source text; lengths of source/display spans must match.
    Copied(Range<usize>),
    /// Generated or substituted text belongs to this source span and is
    /// non-editable. Clicking returns its owning span, not a guessed offset.
    Owned(Range<usize>),
    /// A ghost or decoration has no editable source owner.
    Unowned,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub display: Range<usize>,
    pub provenance: Provenance,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHit {
    pub owner: Option<Range<usize>>,
    pub caret: Option<usize>,
}
/// Entries include hidden source spans as zero-length display segments.
/// Keep segments ordered by display position, preserving source order at ties.
#[derive(Debug, Clone, Default)]
pub struct SourceDisplayMap {
    pub segments: Vec<Segment>,
    pub source_len: usize,
    pub display_len: usize,
}
impl SourceDisplayMap {
    pub fn identity(text: &str) -> Self {
        let len = text.chars().count();
        Self {
            segments: vec![Segment {
                display: 0..len,
                provenance: Provenance::Copied(0..len),
            }],
            source_len: len,
            display_len: len,
        }
    }
    /// Source caret to display boundary. At generated interiors, affinity
    /// selects the owning display span's edge. Hidden spans collapse to one edge.
    pub fn source_to_display(&self, source_col: usize, affinity: Affinity) -> Option<usize> {
        let col = source_col.min(self.source_len);
        let mut copied: Option<usize> = None;
        let mut owned: Option<usize> = None;
        for segment in &self.segments {
            let span = match &segment.provenance {
                Provenance::Copied(span) | Provenance::Owned(span) => span,
                Provenance::Unowned => continue,
            };
            if span.start <= col && col <= span.end {
                let (result, mapped) = match &segment.provenance {
                    Provenance::Copied(_) => {
                        (&mut copied, segment.display.start + col - span.start)
                    }
                    _ => (
                        &mut owned,
                        if !span.is_empty() && col == span.start {
                            segment.display.start
                        } else if !span.is_empty() && col == span.end {
                            segment.display.end
                        } else {
                            match affinity {
                                Affinity::Before => segment.display.start,
                                Affinity::After => segment.display.end,
                            }
                        },
                    ),
                };
                *result = Some(match (*result, affinity) {
                    (Some(old), Affinity::Before) => old.min(mapped),
                    (Some(old), Affinity::After) => old.max(mapped),
                    (None, _) => mapped,
                });
            }
        }
        // Padding can own an entire cell while its unchanged content retains
        // exact caret coordinates. Ownership is only a fallback for that source.
        copied.or(owned)
    }

    /// Display boundary to caret or generated owner. Hidden boundary affinity
    /// selects the preceding/following source edge without byte conversion.
    pub fn display_to_source(&self, display_col: usize, affinity: Affinity) -> Option<SourceHit> {
        let col = display_col.min(self.display_len);
        let mut matches = self
            .segments
            .iter()
            .filter(|s| s.display.start <= col && col <= s.display.end);
        let segment = match affinity {
            Affinity::Before => matches.next(),
            Affinity::After => matches.next_back(),
        }?;
        Some(match &segment.provenance {
            Provenance::Copied(span) => SourceHit {
                owner: Some(span.clone()),
                caret: Some(span.start + col - segment.display.start),
            },
            Provenance::Owned(span) => SourceHit {
                owner: Some(span.clone()),
                caret: if segment.display.is_empty() {
                    Some(match affinity {
                        Affinity::Before => span.start,
                        Affinity::After => span.end,
                    })
                } else {
                    None
                },
            },
            Provenance::Unowned => SourceHit {
                owner: None,
                caret: None,
            },
        })
    }
}
/// Builder receives original-source provenance while transforms emit text.
/// Padding/formula markers must be Owned, ghosts Unowned. Hidden ranges must
/// still be emitted as empty Owned segments so source cursor mapping survives.
#[derive(Default)]
pub struct MappedLineBuilder {
    pub text: String,
    pub map: SourceDisplayMap,
}
impl MappedLineBuilder {
    pub fn new(source_len: usize) -> Self {
        Self {
            map: SourceDisplayMap {
                source_len,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    pub fn push(&mut self, text: &str, provenance: Provenance) {
        let start = self.map.display_len;
        let len = text.chars().count();
        if let Provenance::Copied(span) = &provenance {
            assert_eq!(
                span.end - span.start,
                len,
                "Copied spans require exact provenance"
            );
        }
        self.text.push_str(text);
        self.map.display_len += len;
        self.map.segments.push(Segment {
            display: start..start + len,
            provenance,
        });
    }
}

impl SourceDisplayMap {
    /// Append decoration text; it has no editable source owner.
    pub fn append_ghost(&mut self, text: &str) {
        let start = self.display_len;
        self.display_len += text.chars().count();
        self.segments.push(Segment {
            display: start..self.display_len,
            provenance: Provenance::Unowned,
        });
    }

    /// Compose an intermediate-to-display map with the original-to-intermediate
    /// map. Transformations provide provenance explicitly; text is never diffed.
    pub fn compose(&self, previous: &Self) -> Self {
        assert_eq!(self.source_len, previous.display_len);
        let mut segments = Vec::new();
        for next in &self.segments {
            match &next.provenance {
                Provenance::Unowned => segments.push(next.clone()),
                Provenance::Owned(span) => {
                    let owner = previous.owner_for_range(span.clone());
                    segments.push(Segment {
                        display: next.display.clone(),
                        provenance: owner.map_or(Provenance::Unowned, Provenance::Owned),
                    });
                }
                Provenance::Copied(span) => {
                    for old in &previous.segments {
                        let start = span.start.max(old.display.start);
                        let end = span.end.min(old.display.end);
                        if start > end || (start == end && !old.display.is_empty()) {
                            continue;
                        }
                        let provenance = match &old.provenance {
                            Provenance::Copied(source) => Provenance::Copied(
                                source.start + start - old.display.start
                                    ..source.start + end - old.display.start,
                            ),
                            Provenance::Owned(source) => Provenance::Owned(source.clone()),
                            Provenance::Unowned => Provenance::Unowned,
                        };
                        segments.push(Segment {
                            display: next.display.start + start - span.start
                                ..next.display.start + end - span.start,
                            provenance,
                        });
                    }
                }
            }
        }
        Self {
            segments,
            source_len: previous.source_len,
            display_len: self.display_len,
        }
    }

    fn owner_for_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        let mut owner: Option<Range<usize>> = None;
        for segment in &self.segments {
            let start = range.start.max(segment.display.start);
            let end = range.end.min(segment.display.end);
            if start > end || (start == end && !range.is_empty() && !segment.display.is_empty()) {
                continue;
            }
            let source = match &segment.provenance {
                Provenance::Copied(span) => {
                    span.start + start - segment.display.start
                        ..span.start + end - segment.display.start
                }
                Provenance::Owned(span) => span.clone(),
                Provenance::Unowned => continue,
            };
            owner = Some(match owner {
                Some(old) => old.start.min(source.start)..old.end.max(source.end),
                None => source,
            });
        }
        owner
    }

    /// Visible source-range pieces for selections/search. Generated text is
    /// included only when its owning source overlaps; unowned ghosts are excluded.
    pub fn source_range_to_display(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        if range.is_empty() {
            return ranges;
        }
        for segment in &self.segments {
            let display = match &segment.provenance {
                Provenance::Copied(source) => {
                    let start = range.start.max(source.start);
                    let end = range.end.min(source.end);
                    if start >= end {
                        continue;
                    }
                    segment.display.start + start - source.start
                        ..segment.display.start + end - source.start
                }
                Provenance::Owned(source) => {
                    if range.start >= source.end || range.end <= source.start {
                        continue;
                    }
                    segment.display.clone()
                }
                Provenance::Unowned => continue,
            };
            if display.is_empty() {
                continue;
            }
            if let Some(last) = ranges.last_mut() {
                if last.end == display.start {
                    last.end = display.end;
                    continue;
                }
            }
            ranges.push(display);
        }
        ranges
    }

    /// Build an explicit hiding transform for this displayed text, then
    /// compose it back to the original source. Ranges use scalar columns.
    pub fn hide(&self, text: &str, hidden: &[Range<usize>]) -> MappedLineBuilder {
        assert_eq!(text.chars().count(), self.display_len);
        let mut ranges = hidden
            .iter()
            .filter_map(|range| {
                let start = range.start.min(self.display_len);
                let end = range.end.min(self.display_len);
                (start < end).then_some(start..end)
            })
            .collect::<Vec<_>>();
        ranges.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            if let Some(last) = merged.last_mut() {
                if range.start <= last.end {
                    last.end = last.end.max(range.end);
                    continue;
                }
            }
            merged.push(range);
        }
        let mut builder = MappedLineBuilder::new(self.display_len);
        let mut cursor = 0;
        for range in merged {
            if cursor < range.start {
                builder.push(
                    &text[scalar_to_byte(text, cursor)..scalar_to_byte(text, range.start)],
                    Provenance::Copied(cursor..range.start),
                );
            }
            builder.push("", Provenance::Owned(range.clone()));
            cursor = range.end;
        }
        if cursor < self.display_len {
            builder.push(
                &text[scalar_to_byte(text, cursor)..],
                Provenance::Copied(cursor..self.display_len),
            );
        }
        if builder.map.segments.is_empty() {
            builder.push("", Provenance::Copied(0..0));
        }
        builder.map = builder.map.compose(self);
        builder
    }
}

/// Clamp to a scalar boundary, rounding byte/UTF-16 interiors toward the start.
pub fn scalar_to_byte(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}
pub fn byte_to_scalar(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(offset, _)| *offset < byte.min(text.len()))
        .filter(|(offset, ch)| offset + ch.len_utf8() <= byte.min(text.len()))
        .count()
}
pub fn scalar_to_utf16(text: &str, column: usize) -> usize {
    text.chars().take(column).map(char::len_utf16).sum()
}
pub fn utf16_to_scalar(text: &str, units: usize) -> usize {
    let mut consumed = 0;
    text.chars()
        .take_while(|ch| {
            consumed += ch.len_utf16();
            consumed <= units
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_unicode_markers_keep_boundary_affinity_and_range_mapping() {
        let source = "**é🙂** tail";
        let hidden = SourceDisplayMap::identity(source).hide(source, &[0..2, 4..6]);
        assert_eq!(hidden.text, "é🙂 tail");
        assert_eq!(
            hidden
                .map
                .display_to_source(0, Affinity::Before)
                .unwrap()
                .caret,
            Some(0)
        );
        assert_eq!(
            hidden
                .map
                .display_to_source(0, Affinity::After)
                .unwrap()
                .caret,
            Some(2)
        );
        assert_eq!(hidden.map.source_range_to_display(2..4), vec![0..2]);
        assert_eq!(hidden.map.source_to_display(5, Affinity::After), Some(2));
    }
    #[test]
    fn composed_formula_padding_and_ghost_keep_exact_owners() {
        let mut first = MappedLineBuilder::new(5);
        first.push("a", Provenance::Copied(0..1));
        first.push("42", Provenance::Owned(1..4));
        first.push("z", Provenance::Copied(4..5));
        let mut second = MappedLineBuilder::new(4);
        second.push(" ", Provenance::Owned(0..1));
        second.push("a42z", Provenance::Copied(0..4));
        second.push("ghost", Provenance::Unowned);
        let map = second.map.compose(&first.map);
        let hit = map.display_to_source(2, Affinity::After).unwrap();
        assert_eq!(hit.owner, Some(1..4));
        assert_eq!(hit.caret, None);
        assert_eq!(map.source_to_display(4, Affinity::Before), Some(4));
        assert_eq!(map.source_to_display(1, Affinity::After), Some(2));
        assert_eq!(
            map.display_to_source(0, Affinity::Before).unwrap().owner,
            Some(0..1)
        );
        assert_eq!(
            map.display_to_source(6, Affinity::After).unwrap().owner,
            None
        );
        assert_eq!(map.source_range_to_display(1..4), vec![2..4]);
    }
    #[test]
    fn unicode_coordinate_conversion_clamps_partial_units() {
        let text = "é🙂λ";
        assert_eq!(scalar_to_byte(text, 2), 6);
        assert_eq!(byte_to_scalar(text, 3), 1);
        assert_eq!(byte_to_scalar(text, 99), 3);
        assert_eq!(scalar_to_utf16(text, 2), 3);
        assert_eq!(utf16_to_scalar(text, 2), 1);
        assert_eq!(utf16_to_scalar(text, 99), 3);
    }
    #[test]
    fn cell_padding_does_not_override_copied_content_carets() {
        let mut line = MappedLineBuilder::new(3);
        line.push("  ", Provenance::Owned(0..3));
        line.push("abc", Provenance::Copied(0..3));
        line.push("   ", Provenance::Owned(0..3));
        for affinity in [Affinity::Before, Affinity::After] {
            assert_eq!(line.map.source_to_display(1, affinity), Some(3));
            assert_eq!(line.map.source_to_display(0, affinity), Some(2));
            assert_eq!(line.map.source_to_display(3, affinity), Some(5));
        }
        assert_eq!(
            line.map
                .display_to_source(1, Affinity::After)
                .unwrap()
                .caret,
            None
        );
        assert_eq!(
            line.map
                .display_to_source(3, Affinity::After)
                .unwrap()
                .caret,
            Some(1)
        );
    }
    #[test]
    fn hidden_shared_boundary_and_multiple_copies_obey_affinity() {
        let source = "a**b";
        let hidden = SourceDisplayMap::identity(source).hide(source, std::slice::from_ref(&(1..3)));
        assert_eq!(
            hidden
                .map
                .display_to_source(1, Affinity::Before)
                .unwrap()
                .caret,
            Some(1)
        );
        assert_eq!(
            hidden
                .map
                .display_to_source(1, Affinity::After)
                .unwrap()
                .caret,
            Some(3)
        );
        let mut duplicated = MappedLineBuilder::new(2);
        duplicated.push("ab", Provenance::Copied(0..2));
        duplicated.push(" ", Provenance::Unowned);
        duplicated.push("ab", Provenance::Copied(0..2));
        assert_eq!(
            duplicated.map.source_to_display(0, Affinity::Before),
            Some(0)
        );
        assert_eq!(
            duplicated.map.source_to_display(0, Affinity::After),
            Some(3)
        );
        assert_eq!(
            duplicated.map.source_to_display(2, Affinity::Before),
            Some(2)
        );
        assert_eq!(
            duplicated.map.source_to_display(2, Affinity::After),
            Some(5)
        );
    }
}
