use crate::markdown_tokens;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FoldKind {
    Heading,
    Fence,
    List,
    Table,
    Paragraph,
}

impl FoldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FoldKind::Heading => "heading",
            FoldKind::Fence => "fence",
            FoldKind::List => "list",
            FoldKind::Table => "table",
            FoldKind::Paragraph => "paragraph",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldRange {
    pub start_line: usize, // 0-based
    pub end_line: usize,   // 0-based, inclusive
    pub kind: FoldKind,
}

#[derive(Clone, Copy)]
struct FoldBuildOptions {
    include_heading: bool,
    include_fence: bool,
    include_list: bool,
    include_table: bool,
    include_paragraph: bool,
    trim_heading_trailing_blank: bool,
}

const FOLD_OPTIONS_UI: FoldBuildOptions = FoldBuildOptions {
    include_heading: true,
    include_fence: true,
    include_list: true,
    include_table: true,
    include_paragraph: true,
    trim_heading_trailing_blank: true,
};

const FOLD_OPTIONS_TERMINAL: FoldBuildOptions = FoldBuildOptions {
    include_heading: true,
    include_fence: true,
    include_list: true,
    include_table: true,
    include_paragraph: true,
    trim_heading_trailing_blank: false,
};

fn is_table_fold_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn is_list_fold_line(line: &str) -> bool {
    markdown_tokens::list_marker_end(line).is_some()
}

fn build_fold_ranges_with_options(
    lines: &[String],
    options: FoldBuildOptions,
) -> Vec<FoldRange> {
    if lines.len() <= 1 {
        return Vec::new();
    }

    let analyzed = markdown_tokens::analyze_lines(lines, false, None).lines;
    let line_count = lines.len();
    let mut ranges = Vec::new();

    if options.include_heading {
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
            if options.trim_heading_trailing_blank {
                while end_line > line_idx && lines[end_line].trim().is_empty() {
                    end_line -= 1;
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
    }

    if options.include_fence {
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
    }

    if options.include_list || options.include_table || options.include_paragraph {
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

            if options.include_list && is_list_fold_line(text) {
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

            if options.include_table && is_table_fold_line(text) {
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

            if !options.include_paragraph
                || line.info.heading_level.is_some()
                || line.info.is_horizontal_rule
            {
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
                    || (options.include_list && is_list_fold_line(next_text))
                    || (options.include_table && is_table_fold_line(next_text))
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
    }

    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    ranges
}

pub fn build_fold_ranges_ui(lines: &[String]) -> Vec<FoldRange> {
    build_fold_ranges_with_options(lines, FOLD_OPTIONS_UI)
}

pub fn build_fold_ranges_terminal(lines: &[String]) -> Vec<FoldRange> {
    build_fold_ranges_with_options(lines, FOLD_OPTIONS_TERMINAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    fn describe(ranges: &[FoldRange]) -> Vec<(usize, usize, &'static str)> {
        ranges
            .iter()
            .map(|range| (range.start_line, range.end_line, range.kind.as_str()))
            .collect()
    }

    #[test]
    fn ui_ranges_include_heading_fence_and_list_blocks() {
        let lines = as_lines(&[
            "# One",
            "alpha",
            "- list",
            "- items",
            "```ts",
            "const x = 1",
            "```",
        ]);
        let described = describe(&build_fold_ranges_ui(&lines));
        assert_eq!(
            described,
            vec![(0, 6, "heading"), (2, 3, "list"), (4, 6, "fence")]
        );
    }

    #[test]
    fn ui_heading_ranges_trim_trailing_blank_lines() {
        let lines = as_lines(&["# One", "alpha", "", "", "# Two", "beta", ""]);
        let described = describe(&build_fold_ranges_ui(&lines));
        assert_eq!(described, vec![(0, 1, "heading"), (4, 5, "heading")]);
    }

    #[test]
    fn terminal_ranges_include_list_table_and_paragraph_blocks() {
        let lines = as_lines(&[
            "# One",
            "alpha",
            "",
            "- a",
            "- b",
            "",
            "| h | v |",
            "| --- | --- |",
            "| a | 1 |",
            "",
            "para",
            "text",
        ]);
        let described = describe(&build_fold_ranges_terminal(&lines));
        assert_eq!(
            described,
            vec![
                (0, 11, "heading"),
                (3, 4, "list"),
                (6, 8, "table"),
                (10, 11, "paragraph"),
            ]
        );
    }

    #[test]
    fn paragraph_ranges_stop_at_empty_line_separator() {
        let lines = as_lines(&["alpha", "beta", "", "gamma", "delta", "", "# Heading"]);
        let ui_described = describe(&build_fold_ranges_ui(&lines));
        let terminal_described = describe(&build_fold_ranges_terminal(&lines));

        assert!(
            ui_described.contains(&(0, 1, "paragraph"))
                && ui_described.contains(&(3, 4, "paragraph"))
        );
        assert!(
            terminal_described.contains(&(0, 1, "paragraph"))
                && terminal_described.contains(&(3, 4, "paragraph"))
        );
    }
}
