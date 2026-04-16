use crate::editor_core::markdown_tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Heading,
    Fence,
    List,
    Table,
    Paragraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldKind,
}

fn is_table_fold_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn is_list_fold_line(line: &str) -> bool {
    markdown_tokens::list_marker_end(line).is_some()
}

pub fn build_fold_ranges(lines: &[String]) -> Vec<FoldRange> {
    if lines.len() <= 1 {
        return Vec::new();
    }

    let analyzed = markdown_tokens::analyze_lines(lines, false, None).lines;
    let line_count = lines.len();
    let mut ranges = Vec::new();

    // Headings fold until the next heading of the same level.
    for line_idx in 0..line_count {
        let line = &analyzed[line_idx];
        if line.in_code_block || line.info.heading_level.is_none() {
            continue;
        }
        let mut end_line = line_count - 1;
        for next_idx in (line_idx + 1)..line_count {
            let next = &analyzed[next_idx];
            if next.in_code_block {
                continue;
            }
            if next.info.heading_level == line.info.heading_level {
                end_line = next_idx.saturating_sub(1);
                break;
            }
        }
        if end_line > line_idx {
            ranges.push(FoldRange {
                start_line: line_idx,
                end_line,
                kind: FoldKind::Heading,
            });
        }
    }

    // Fenced code blocks fold from opening fence through closing fence.
    let mut open_fence_line: Option<usize> = None;
    for line_idx in 0..line_count {
        let line = &analyzed[line_idx];
        if !line.info.is_code_fence {
            continue;
        }
        if !line.in_code_block {
            open_fence_line = Some(line_idx);
            continue;
        }

        if let Some(start_line) = open_fence_line.take() {
            if line_idx > start_line {
                ranges.push(FoldRange {
                    start_line,
                    end_line: line_idx,
                    kind: FoldKind::Fence,
                });
            }
        }
    }
    if let Some(start_line) = open_fence_line {
        if start_line + 1 < line_count {
            ranges.push(FoldRange {
                start_line,
                end_line: line_count - 1,
                kind: FoldKind::Fence,
            });
        }
    }

    // List/table/paragraph block folds.
    let mut line_idx = 0usize;
    while line_idx < line_count {
        let line = &analyzed[line_idx];
        let text = lines[line_idx].as_str();
        let trimmed = text.trim();

        if line.in_code_block || line.info.is_code_fence || trimmed.is_empty() {
            line_idx += 1;
            continue;
        }

        if is_list_fold_line(text) {
            let start_line = line_idx;
            line_idx += 1;
            while line_idx < line_count {
                let next = &analyzed[line_idx];
                let next_text = lines[line_idx].as_str();
                if next.in_code_block
                    || next.info.is_code_fence
                    || next_text.trim().is_empty()
                    || !is_list_fold_line(next_text)
                {
                    break;
                }
                line_idx += 1;
            }
            let end_line = line_idx.saturating_sub(1);
            if end_line > start_line {
                ranges.push(FoldRange {
                    start_line,
                    end_line,
                    kind: FoldKind::List,
                });
            }
            continue;
        }

        if is_table_fold_line(text) {
            let start_line = line_idx;
            line_idx += 1;
            while line_idx < line_count {
                let next = &analyzed[line_idx];
                let next_text = lines[line_idx].as_str();
                if next.in_code_block
                    || next.info.is_code_fence
                    || next_text.trim().is_empty()
                    || !is_table_fold_line(next_text)
                {
                    break;
                }
                line_idx += 1;
            }
            let end_line = line_idx.saturating_sub(1);
            if end_line > start_line {
                ranges.push(FoldRange {
                    start_line,
                    end_line,
                    kind: FoldKind::Table,
                });
            }
            continue;
        }

        if line.info.heading_level.is_some() || line.info.is_horizontal_rule {
            line_idx += 1;
            continue;
        }

        let start_line = line_idx;
        line_idx += 1;
        while line_idx < line_count {
            let next = &analyzed[line_idx];
            let next_text = lines[line_idx].as_str();
            let next_trimmed = next_text.trim();
            if next_trimmed.is_empty()
                || next.in_code_block
                || next.info.is_code_fence
                || next.info.heading_level.is_some()
                || next.info.is_horizontal_rule
                || is_list_fold_line(next_text)
                || is_table_fold_line(next_text)
            {
                break;
            }
            line_idx += 1;
        }
        let end_line = line_idx.saturating_sub(1);
        if end_line > start_line {
            ranges.push(FoldRange {
                start_line,
                end_line,
                kind: FoldKind::Paragraph,
            });
        }
    }

    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    ranges
}

#[cfg(test)]
pub fn describe_fold_ranges(lines: &[&str]) -> Vec<(usize, usize, &'static str)> {
    let owned = lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    build_fold_ranges(&owned)
        .into_iter()
        .map(|range| {
            let kind = match range.kind {
                FoldKind::Heading => "heading",
                FoldKind::Fence => "fence",
                FoldKind::List => "list",
                FoldKind::Table => "table",
                FoldKind::Paragraph => "paragraph",
            };
            (range.start_line, range.end_line, kind)
        })
        .collect()
}
