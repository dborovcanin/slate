use super::{
    contrast_fg_for_bg, cursor_render_char_col, display_cell_pipe_positions,
    display_cols_prefix_and_total, draw_framed_surface, draw_row_at_styled,
    find_table_formula_segments, format_formula_display_value, formula_marker_token, goto,
    is_markdown_table_line, line_display_cols, min, pad_right, reformat_table_row_for_display,
    table_block_bounds_for_line, table_cell_info_at_char, table_cursor_cell_index,
    table_display_col_widths, viewport_col_for_display_col, AnsiStyle, DatePickerAction,
    TableFormulaSegment, TerminalApp, UiMode, EDITOR_TOP_ROW, OVERFLOW_LEFT_MARKER,
    OVERFLOW_RIGHT_MARKER, TITLE_ROW, WIKI_LINK_AUTOCOMPLETE_MAX_VISIBLE,
};
use crate::terminal::render;
use crate::terminal::text_utils::{compute_line_viewport, derive_title_from_lines};
use crate::terminal::{date_picker, input, media_sources, notifications, switcher};
use crate::terminal::{
    date_picker::DatePickerView,
    switcher::{
        content_search_box_geometry, CollectionEditView, CollectionSwitcherView, ContentSearchView,
        SwitcherView,
    },
};
use std::borrow::Cow;
use std::io::Write;

// Ownership: status/popup composition and terminal rendering/cursor placement.
impl TerminalApp {
    fn table_error_text(
        error_kind: Option<&app_core::calc::TableCellErrorKind>,
        value: &str,
    ) -> Option<String> {
        match error_kind {
            Some(app_core::calc::TableCellErrorKind::OutOfBounds) => {
                Some(String::from("table error: out_of_bounds"))
            }
            Some(app_core::calc::TableCellErrorKind::NonNumeric) => {
                Some(String::from("table error: non_numeric"))
            }
            Some(app_core::calc::TableCellErrorKind::SelfReference) => {
                Some(String::from("table error: self_reference"))
            }
            Some(app_core::calc::TableCellErrorKind::Cycle) => {
                Some(String::from("table error: cycle"))
            }
            Some(app_core::calc::TableCellErrorKind::Unknown) => {
                if let Some(code) = value.strip_prefix("!ERROR#") {
                    Some(format!("table error: {code}"))
                } else {
                    Some(String::from("table error: unknown"))
                }
            }
            None => None,
        }
    }

    fn masked_formula_value<'a>(value: &'a str, is_error: bool) -> Cow<'a, str> {
        // Keep table columns aligned while avoiding long `!ERROR#...` payloads
        // that get awkwardly cut inside narrow cells.
        if is_error {
            Cow::Borrowed("!ERROR")
        } else {
            Cow::Borrowed(value)
        }
    }

    fn fit_formula_marker_replacement(
        value: &str,
        marker: &str,
        target_chars: usize,
        is_error: bool,
    ) -> String {
        if target_chars == 0 {
            return String::new();
        }
        let marker_chars = marker.chars().count();
        if marker_chars >= target_chars {
            return marker.chars().take(target_chars).collect();
        }

        let value_budget = target_chars - marker_chars;
        let masked_value = Self::masked_formula_value(value, is_error);
        let value_chars = masked_value.chars().count();
        let mut out = String::with_capacity(target_chars);
        if value_chars > value_budget && value_budget >= 2 {
            out.extend(masked_value.chars().take(value_budget - 1));
            out.push('…');
        } else {
            out.extend(masked_value.chars().take(value_budget));
        }
        out.push_str(marker);
        let out_chars = out.chars().count();
        if out_chars < target_chars {
            out.push_str(&" ".repeat(target_chars - out_chars));
        }
        out
    }

    fn push_rendered_segment_with_count(out: &mut String, segment: &str, char_count: &mut usize) {
        out.push_str(segment);
        *char_count += segment.chars().count();
    }

    fn push_rendered_chars_segment(
        out: &mut String,
        chars: &[char],
        from: usize,
        to: usize,
        char_count: &mut usize,
    ) {
        if from >= to {
            return;
        }
        out.extend(chars[from..to].iter());
        *char_count += to - from;
    }

    fn append_link_display_text(base: &str, heading: Option<&str>) -> String {
        let mut text = String::from(base);
        if let Some(h) = heading.filter(|value| !value.is_empty()) {
            text.push('#');
            text.push_str(h);
        }
        text
    }

    fn wiki_link_cache_entry(&mut self, short_id: &str) -> (String, bool) {
        if let Some(entry) = self.render_caches.wiki_link_render_cache.get(short_id) {
            if entry.cached_at.elapsed().as_millis() as u64 <= super::WIKI_LINK_RENDER_CACHE_TTL_MS
            {
                return (entry.display.clone(), entry.broken);
            }
            self.render_caches.wiki_link_render_cache.remove(short_id);
        }
        let (display, broken) = if let Some(entry) = self.wiki_link_prefix_index.get(short_id) {
            let title = if entry.title.trim().is_empty() {
                "Untitled"
            } else {
                entry.title.as_str()
            };
            (title.to_string(), false)
        } else {
            ("?".to_string(), true)
        };

        let is_new = !self
            .render_caches
            .wiki_link_render_cache
            .contains_key(short_id);
        self.render_caches.wiki_link_render_cache.insert(
            short_id.to_string(),
            super::WikiLinkRenderCacheEntry {
                display: display.clone(),
                broken,
                cached_at: std::time::Instant::now(),
            },
        );
        if is_new {
            self.render_caches
                .wiki_link_render_cache_order
                .push_back(short_id.to_string());
        }
        while self.render_caches.wiki_link_render_cache.len()
            > super::WIKI_LINK_RENDER_CACHE_MAX_ENTRIES
        {
            let Some(evict_key) = self.render_caches.wiki_link_render_cache_order.pop_front()
            else {
                break;
            };
            self.render_caches.wiki_link_render_cache.remove(&evict_key);
        }

        (display, broken)
    }

    fn wiki_link_line_cache_entry(
        &mut self,
        line_text: &str,
    ) -> Option<(String, Vec<(usize, usize)>)> {
        let Some(entry) = self
            .render_caches
            .wiki_link_line_render_cache
            .get(line_text)
        else {
            return None;
        };
        if entry.cached_at.elapsed().as_millis() as u64 <= super::WIKI_LINK_LINE_RENDER_CACHE_TTL_MS
        {
            return Some((entry.rendered_line.clone(), entry.underline_ranges.clone()));
        }
        self.render_caches
            .wiki_link_line_render_cache
            .remove(line_text);
        None
    }

    pub(super) fn invalidate_wiki_link_render_cache_for_short_id(&mut self, short_id: &str) {
        if short_id.is_empty() {
            return;
        }
        self.render_caches.wiki_link_render_cache.remove(short_id);
        // The line-level cache keys are full line text strings; scanning them
        // for the changed short_id would be O(entries). Clearing the whole
        // cache is O(1) and correct — entries are cheap to rebuild on the
        // next draw using the still-warm short_id cache above.
        self.render_caches.wiki_link_line_render_cache.clear();
        self.render_caches.wiki_link_line_render_cache_order.clear();
    }

    fn insert_wiki_link_line_cache(
        &mut self,
        line_text: &str,
        rendered_line: &str,
        underline_ranges: &[(usize, usize)],
    ) {
        let is_new = !self
            .render_caches
            .wiki_link_line_render_cache
            .contains_key(line_text);
        self.render_caches.wiki_link_line_render_cache.insert(
            line_text.to_string(),
            super::WikiLinkLineRenderCacheEntry {
                rendered_line: rendered_line.to_string(),
                underline_ranges: underline_ranges.to_vec(),
                cached_at: std::time::Instant::now(),
            },
        );
        if is_new {
            self.render_caches
                .wiki_link_line_render_cache_order
                .push_back(line_text.to_string());
        }
        while self.render_caches.wiki_link_line_render_cache.len()
            > super::WIKI_LINK_LINE_RENDER_CACHE_MAX_ENTRIES
        {
            let Some(evict_key) = self
                .render_caches
                .wiki_link_line_render_cache_order
                .pop_front()
            else {
                break;
            };
            self.render_caches
                .wiki_link_line_render_cache
                .remove(evict_key.as_str());
        }
    }

    fn table_formula_segments_cache_entry(
        &mut self,
        line_text: &str,
    ) -> Option<Vec<TableFormulaSegment>> {
        let Some(entry) = self
            .render_caches
            .table_formula_segment_cache
            .get(line_text)
        else {
            return None;
        };
        if entry.cached_at.elapsed().as_millis() as u64 <= super::TABLE_FORMULA_SEGMENT_CACHE_TTL_MS
        {
            return Some(entry.segments.clone());
        }
        self.render_caches
            .table_formula_segment_cache
            .remove(line_text);
        None
    }

    fn table_formula_segments_cached(&mut self, line_text: &str) -> Vec<TableFormulaSegment> {
        if let Some(cached) = self.table_formula_segments_cache_entry(line_text) {
            return cached;
        }
        let segments = find_table_formula_segments(line_text);
        let is_new = !self
            .render_caches
            .table_formula_segment_cache
            .contains_key(line_text);
        self.render_caches.table_formula_segment_cache.insert(
            line_text.to_string(),
            super::TableFormulaSegmentCacheEntry {
                segments: segments.clone(),
                cached_at: std::time::Instant::now(),
            },
        );
        if is_new {
            self.render_caches
                .table_formula_segment_cache_order
                .push_back(line_text.to_string());
        }
        while self.render_caches.table_formula_segment_cache.len()
            > super::TABLE_FORMULA_SEGMENT_CACHE_MAX_ENTRIES
        {
            let Some(evict_key) = self
                .render_caches
                .table_formula_segment_cache_order
                .pop_front()
            else {
                break;
            };
            self.render_caches
                .table_formula_segment_cache
                .remove(evict_key.as_str());
        }
        segments
    }

    /// Returns per-column visible display widths for the table block containing
    /// `line_idx`. Results are cached by `(block_start, block_hash)`.
    fn table_display_col_widths_for_line(&mut self, line_idx: usize) -> Vec<usize> {
        let Some((block_start, block_end)) =
            table_block_bounds_for_line(&self.editor.lines, line_idx)
        else {
            return Vec::new();
        };
        let block_lines = &self.editor.lines[block_start..=block_end];
        // Hash the block content for cache validation.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        block_start.hash(&mut hasher);
        for l in block_lines {
            l.hash(&mut hasher);
        }
        let block_hash = hasher.finish();
        let key = (block_start, block_hash);
        if let Some(cached) = self.render_caches.table_display_col_width_cache.get(&key) {
            return cached.clone();
        }
        let widths = table_display_col_widths(&self.editor.lines[block_start..=block_end]);
        self.render_caches
            .table_display_col_width_cache
            .insert(key, widths.clone());
        // Evict any stale entries for this block_start (different block content).
        self.render_caches
            .table_display_col_width_cache
            .retain(|k, _| k.0 != block_start || k.1 == block_hash);
        widths
    }

    fn render_wiki_link_display_line(&mut self, line_text: &str) -> (String, Vec<(usize, usize)>) {
        if let Some(cached) = self.wiki_link_line_cache_entry(line_text) {
            return cached;
        }
        let links = crate::editor_core::markdown_tokens::find_wiki_link_matches(line_text);
        if links.is_empty() {
            return (line_text.to_string(), Vec::new());
        }

        let chars: Vec<char> = line_text.chars().collect();
        let mut out = String::with_capacity(line_text.len() + 16);
        let mut underline_ranges: Vec<(usize, usize)> = Vec::new();
        let mut cursor = 0usize;
        let mut out_char_count = 0usize;

        for link in links {
            if link.title.is_some() || link.from < cursor || link.to > chars.len() {
                continue;
            }
            Self::push_rendered_chars_segment(
                &mut out,
                &chars,
                cursor,
                link.from,
                &mut out_char_count,
            );
            let (base, broken) = self.wiki_link_cache_entry(&link.short_id);
            let display = if broken {
                Self::append_link_display_text("?", link.heading.as_deref())
            } else {
                Self::append_link_display_text(base.as_str(), link.heading.as_deref())
            };
            if !display.is_empty() {
                let start = out_char_count;
                Self::push_rendered_segment_with_count(
                    &mut out,
                    display.as_str(),
                    &mut out_char_count,
                );
                underline_ranges.push((start, out_char_count));
            }
            cursor = link.to;
        }

        if cursor == 0 {
            return (line_text.to_string(), Vec::new());
        }
        Self::push_rendered_chars_segment(
            &mut out,
            &chars,
            cursor,
            chars.len(),
            &mut out_char_count,
        );
        self.insert_wiki_link_line_cache(line_text, &out, &underline_ranges);
        (out, underline_ranges)
    }

    pub(super) fn update_command_status(&mut self) {
        let hint = if self.command_completion.visible {
            self.command_completion
                .options
                .iter()
                .map(|option| option.token.clone())
                .collect::<Vec<_>>()
                .join("  ")
        } else {
            self.build_command_completion_menu()
                .map(|menu| {
                    menu.options
                        .into_iter()
                        .map(|option| option.token)
                        .collect::<Vec<_>>()
                        .join("  ")
                })
                .unwrap_or_default()
        };
        if hint.is_empty() {
            self.status = format!(":{}", self.command_input);
        } else {
            self.status = format!(":{}  [{}]", self.command_input, hint);
        }
    }

    pub(super) fn draw_status_segment(
        buf: &mut String,
        row: usize,
        col: &mut usize,
        cols: usize,
        text: &str,
        style: AnsiStyle,
    ) {
        if *col > cols || text.is_empty() {
            return;
        }
        let remaining = cols.saturating_sub(*col).saturating_add(1);
        if remaining == 0 {
            return;
        }
        let clipped: String = text.chars().take(remaining).collect();
        if clipped.is_empty() {
            return;
        }
        buf.push_str(&goto(row, *col));
        style.write_to(buf);
        buf.push_str(&clipped);
        buf.push_str(render::RESET);
        *col += clipped.chars().count();
    }

    pub(super) fn draw_command_completion_status_row(
        &self,
        buf: &mut String,
        row: usize,
        cols: usize,
        status_bg: u8,
        right_sticky_text: &str,
    ) -> bool {
        if self.mode != UiMode::CommandBar || !self.command_completion.visible {
            return false;
        }
        if self.command_completion.options.is_empty() {
            return false;
        }
        let selected_idx = self
            .command_completion
            .selected_index
            .min(self.command_completion.options.len().saturating_sub(1));

        let base_fg = contrast_fg_for_bg(status_bg);
        let base_style = AnsiStyle {
            fg: Some(base_fg),
            bg: Some(status_bg),
            ..Default::default()
        };
        let selected_style = AnsiStyle {
            fg: Some(status_bg),
            bg: Some(base_fg),
            bold: true,
            ..Default::default()
        };

        draw_row_at_styled(buf, row, 1, cols, "", base_style);
        let sticky_width = right_sticky_text.chars().count().min(cols);
        let viewport_width = cols.saturating_sub(sticky_width);
        if viewport_width > 0 {
            let suffix_text = "]";
            let mut spans: Vec<(&str, AnsiStyle)> =
                Vec::with_capacity(self.command_completion.options.len().saturating_mul(2) + 2);
            let prefix_text = format!(":{}  [", self.command_input);
            spans.push((prefix_text.as_str(), base_style));

            let selected_token = self
                .command_completion
                .options
                .get(selected_idx)
                .map(|option| option.token.as_str())
                .unwrap_or("");

            let mut selected_from = 0usize;
            let mut selected_to = 0usize;
            let mut virtual_col = prefix_text.chars().count();
            for (idx, option) in self.command_completion.options.iter().enumerate() {
                if idx > 0 {
                    spans.push(("  ", base_style));
                    virtual_col += 2;
                }
                let style = if idx == selected_idx {
                    selected_from = virtual_col;
                    selected_to = virtual_col + selected_token.chars().count();
                    selected_style
                } else {
                    base_style
                };
                spans.push((option.token.as_str(), style));
                virtual_col += option.token.chars().count();
            }
            spans.push((suffix_text, base_style));
            virtual_col += suffix_text.chars().count();

            let total_width = virtual_col;
            let mut viewport_from = 0usize;
            if total_width > viewport_width {
                // Keep selected token visible with breathing room on both sides.
                let side_padding = 3usize;
                let viewport_to = viewport_from + viewport_width;
                if selected_to.saturating_add(side_padding) > viewport_to {
                    viewport_from = selected_to
                        .saturating_add(side_padding)
                        .saturating_sub(viewport_width);
                }
                let desired_left = selected_from.saturating_sub(side_padding);
                if desired_left < viewport_from {
                    viewport_from = desired_left;
                }
                let max_from = total_width.saturating_sub(viewport_width);
                viewport_from = viewport_from.min(max_from);
            }
            let viewport_to = viewport_from + viewport_width;

            let mut draw_col = 1usize;
            let mut span_from = 0usize;
            for (text, style) in spans {
                let span_to = span_from + text.chars().count();
                let clip_from = span_from.max(viewport_from);
                let clip_to = span_to.min(viewport_to);
                if clip_to > clip_from {
                    let local_from = clip_from - span_from;
                    let local_to = clip_to - span_from;
                    let clipped: String = text
                        .chars()
                        .skip(local_from)
                        .take(local_to - local_from)
                        .collect();
                    Self::draw_status_segment(
                        buf,
                        row,
                        &mut draw_col,
                        viewport_width,
                        &clipped,
                        style,
                    );
                }
                span_from = span_to;
                if draw_col > viewport_width {
                    break;
                }
            }
        }

        if sticky_width > 0 {
            let sticky_start_col = cols.saturating_sub(sticky_width).saturating_add(1);
            let mut sticky_col = sticky_start_col;
            Self::draw_status_segment(
                buf,
                row,
                &mut sticky_col,
                cols,
                right_sticky_text,
                base_style,
            );
        }
        true
    }

    fn draw_status_row_with_right_sticky(
        &self,
        buf: &mut String,
        row: usize,
        cols: usize,
        left_text: &str,
        right_sticky_text: &str,
        style: AnsiStyle,
    ) {
        draw_row_at_styled(buf, row, 1, cols, "", style);
        let sticky_width = right_sticky_text.chars().count().min(cols);
        if sticky_width == 0 {
            draw_row_at_styled(buf, row, 1, cols, left_text, style);
            return;
        }
        let sticky_start_col = cols.saturating_sub(sticky_width).saturating_add(1);
        let left_budget_cols = sticky_start_col.saturating_sub(1);
        if left_budget_cols > 0 {
            draw_row_at_styled(buf, row, 1, left_budget_cols, left_text, style);
        }
        let mut sticky_col = sticky_start_col;
        Self::draw_status_segment(buf, row, &mut sticky_col, cols, right_sticky_text, style);
    }

    pub(super) fn search_highlights_for_line(
        &self,
        line_idx: usize,
    ) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
        // Matches are always ordered by line (recompute_search iterates lines in order),
        // so partition_point gives us the first match on this line in O(log n).
        let start = self
            .search
            .matches
            .partition_point(|&(l, _, _)| l < line_idx);
        let mut matches = Vec::new();
        let mut current = Vec::new();
        for (offset, &(line, col_start, col_end)) in self.search.matches[start..].iter().enumerate()
        {
            if line != line_idx {
                break;
            }
            if start + offset == self.search.current {
                current.push((col_start, col_end));
            } else {
                matches.push((col_start, col_end));
            }
        }
        (matches, current)
    }

    pub(super) fn line_is_in_visual_selection(&self, line_idx: usize) -> bool {
        let Some(anchor) = self.editor.selection_anchor else {
            return false;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return false;
        }

        let start_line = min(anchor.0, self.editor.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.editor.cursor_line);
        line_idx >= start_line && line_idx <= end_line
    }

    pub(super) fn append_visual_highlights(
        &self,
        line_idx: usize,
        ranges: &mut Vec<(usize, usize)>,
    ) {
        let Some(anchor) = self.editor.selection_anchor else {
            return;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return;
        }

        let start_line = min(anchor.0, self.editor.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.editor.cursor_line);

        if line_idx < start_line || line_idx > end_line {
            return;
        }

        let linewise = self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection_linewise);
        if linewise {
            ranges.push((0, usize::MAX));
            return;
        }

        let (start_col, end_col) = if anchor.0 == self.editor.cursor_line {
            (
                min(anchor.1, self.editor.cursor_col),
                std::cmp::max(anchor.1, self.editor.cursor_col),
            )
        } else if anchor.0 < self.editor.cursor_line {
            (anchor.1, self.editor.cursor_col)
        } else {
            (self.editor.cursor_col, anchor.1)
        };

        if start_line == end_line {
            ranges.push((start_col, end_col + 1));
        } else if line_idx == start_line {
            let line_len = self.editor.lines[line_idx].chars().count();
            ranges.push((start_col, line_len.max(start_col + 1)));
        } else if line_idx == end_line {
            ranges.push((0, end_col + 1));
        } else {
            ranges.push((0, usize::MAX));
        }
    }

    pub(super) fn draw_variable_autocomplete_popup(
        &self,
        buf: &mut String,
        rows: usize,
        cols: usize,
    ) {
        if self.mode != UiMode::Editor
            || !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.suggestions.is_empty()
            || cols == 0
            || rows <= EDITOR_TOP_ROW
        {
            return;
        }

        let max_editor_row = rows.saturating_sub(1);
        let available_editor_rows = max_editor_row.saturating_sub(EDITOR_TOP_ROW) + 1;
        if available_editor_rows < 3 {
            return;
        }

        let max_suggestions = available_editor_rows.saturating_sub(2).max(1);
        let visible_count = self
            .variable_autocomplete_popup
            .suggestions
            .len()
            .min(max_suggestions);
        let suggestions = &self.variable_autocomplete_popup.suggestions[..visible_count];
        let selected_index = self
            .variable_autocomplete_popup
            .selected_index
            .min(visible_count.saturating_sub(1));

        let inner_width = suggestions
            .iter()
            .map(|item| item.chars().count() + 2)
            .max()
            .unwrap_or(1)
            .min(cols.saturating_sub(2).max(1));
        let box_width = (inner_width + 2).min(cols.max(1));
        let box_height = visible_count + 2;

        let mut x = self.variable_autocomplete_popup.anchor_col.min(cols.max(1));
        if x + box_width > cols + 1 {
            x = cols.saturating_sub(box_width).saturating_add(1).max(1);
        }

        let preferred_top = self
            .variable_autocomplete_popup
            .anchor_row
            .saturating_add(1);
        let mut y = preferred_top;
        if y + box_height > max_editor_row + 1 {
            y = self
                .variable_autocomplete_popup
                .anchor_row
                .saturating_sub(box_height);
        }
        y = y
            .max(EDITOR_TOP_ROW)
            .min(max_editor_row.saturating_sub(box_height.saturating_sub(1)));

        let row_style = AnsiStyle {
            fg: Some(self.render_palette.variable),
            bg: Some(self.render_palette.surface_bg()),
            ..Default::default()
        };
        let selected_bg = self.render_palette.primary();
        let selected_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(selected_bg)),
            bg: Some(selected_bg),
            bold: true,
            ..Default::default()
        };

        draw_framed_surface(
            buf,
            y,
            x,
            box_width,
            box_height,
            self.render_palette.surface_bg(),
            self.render_palette.primary(),
            false,
        );

        for (idx, suggestion) in suggestions.iter().enumerate() {
            let row = y + 1 + idx;
            let text = format!(" {suggestion}");
            if idx == selected_index {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_width.saturating_sub(2),
                    &text,
                    selected_style,
                );
            } else {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_width.saturating_sub(2),
                    &text,
                    row_style,
                );
            }
        }
    }

    pub(super) fn draw_wiki_link_autocomplete_popup(
        &self,
        buf: &mut String,
        rows: usize,
        cols: usize,
    ) {
        let popup = &self.wiki_link_autocomplete_popup;
        if self.mode != UiMode::Editor || !popup.visible || cols == 0 || rows <= EDITOR_TOP_ROW {
            return;
        }
        if popup.suggestions.is_empty() {
            return;
        }
        let max_editor_row = rows.saturating_sub(1);
        let available_rows = max_editor_row.saturating_sub(EDITOR_TOP_ROW) + 1;
        if available_rows < 3 {
            return;
        }
        let visible_limit = available_rows
            .saturating_sub(2)
            .max(1)
            .min(WIKI_LINK_AUTOCOMPLETE_MAX_VISIBLE);
        let (start, end) = self.wiki_link_visible_window(visible_limit);
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return;
        }
        let visible_suggestions = &popup.suggestions[start..end];
        let selected_index = popup
            .selected_index
            .min(popup.suggestions.len().saturating_sub(1));
        let inner_width = visible_suggestions
            .iter()
            .map(|item| item.title.chars().count() + 2)
            .max()
            .unwrap_or(1)
            .min(cols.saturating_sub(2).max(1));
        let box_width = (inner_width + 2).min(cols.max(1));
        let box_height = visible_count + 2;
        let mut x = popup.anchor_col.min(cols.max(1));
        if x + box_width > cols + 1 {
            x = cols.saturating_sub(box_width).saturating_add(1).max(1);
        }
        let preferred_top = popup.anchor_row.saturating_add(1);
        let mut y = preferred_top;
        if y + box_height > max_editor_row + 1 {
            y = popup.anchor_row.saturating_sub(box_height);
        }
        y = y
            .max(EDITOR_TOP_ROW)
            .min(max_editor_row.saturating_sub(box_height.saturating_sub(1)));
        let row_style = AnsiStyle {
            fg: Some(self.render_palette.variable),
            bg: Some(self.render_palette.surface_bg()),
            ..Default::default()
        };
        let selected_bg = self.render_palette.primary();
        let selected_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(selected_bg)),
            bg: Some(selected_bg),
            bold: true,
            ..Default::default()
        };
        draw_framed_surface(
            buf,
            y,
            x,
            box_width,
            box_height,
            self.render_palette.surface_bg(),
            self.render_palette.primary(),
            false,
        );
        for (idx, suggestion) in visible_suggestions.iter().enumerate() {
            let row = y + 1 + idx;
            let text = format!(" {}", suggestion.title);
            let style = if start + idx == selected_index {
                selected_style
            } else {
                row_style
            };
            draw_row_at_styled(buf, row, x + 1, box_width.saturating_sub(2), &text, style);
        }
    }

    pub(super) fn draw_wiki_link_preview_popup(&self, buf: &mut String, rows: usize, cols: usize) {
        let preview = &self.wiki_link_preview;
        if !preview.visible
            || !matches!(
                self.mode,
                UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
            )
            || cols < 10
            || rows <= EDITOR_TOP_ROW
        {
            return;
        }
        let max_editor_row = rows.saturating_sub(1);
        let inner_width = 58usize.min(cols.saturating_sub(4));
        if inner_width < 4 {
            return;
        }
        let box_width = inner_width + 2;

        let body_lines = preview_wrap_text(&preview.body, inner_width);
        let shown_body = body_lines.len().min(6);
        // height = top border + title row + body rows + bottom border
        let box_height = 3 + shown_body;

        let cursor_screen_row = self
            .editor
            .cursor_line
            .saturating_sub(self.editor.scroll_line)
            + EDITOR_TOP_ROW;
        let y = if cursor_screen_row >= EDITOR_TOP_ROW + box_height {
            cursor_screen_row.saturating_sub(box_height)
        } else {
            cursor_screen_row
                .saturating_add(1)
                .min(max_editor_row.saturating_sub(box_height.saturating_sub(1)))
        }
        .max(EDITOR_TOP_ROW);

        let cursor_screen_col = self
            .editor
            .cursor_col
            .saturating_sub(self.editor.scroll_col)
            + 1;
        let x = if cursor_screen_col + box_width <= cols + 1 {
            cursor_screen_col
        } else {
            cols.saturating_sub(box_width).saturating_add(1).max(1)
        };

        let bg = self.render_palette.surface_bg();
        let border_fg = self.render_palette.primary();
        draw_framed_surface(buf, y, x, box_width, box_height, bg, border_fg, false);

        let title_style = AnsiStyle {
            fg: Some(self.render_palette.text_fg()),
            bg: Some(bg),
            bold: true,
            ..Default::default()
        };
        let title_text = format!(" {}", preview_truncate(&preview.title, inner_width));
        draw_row_at_styled(buf, y + 1, x + 1, inner_width, &title_text, title_style);

        let body_style = AnsiStyle {
            fg: Some(self.render_palette.variable),
            bg: Some(bg),
            ..Default::default()
        };
        for (i, line) in body_lines[..shown_body].iter().enumerate() {
            let text = format!(" {line}");
            draw_row_at_styled(buf, y + 2 + i, x + 1, inner_width, &text, body_style);
        }
    }

    fn parse_goto_sequence(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
        if bytes.get(start).copied()? != 0x1b || bytes.get(start + 1).copied()? != b'[' {
            return None;
        }

        let mut idx = start + 2;
        let row_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == row_start || idx >= bytes.len() || bytes[idx] != b';' {
            return None;
        }
        let row = std::str::from_utf8(&bytes[row_start..idx])
            .ok()?
            .parse::<usize>()
            .ok()?;
        idx += 1;

        let col_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == col_start || idx >= bytes.len() || bytes[idx] != b'H' {
            return None;
        }
        let _col = std::str::from_utf8(&bytes[col_start..idx])
            .ok()?
            .parse::<usize>()
            .ok()?;
        idx += 1;

        Some((idx - start, row))
    }

    fn split_frame_rows(frame: &str, row_count: usize) -> Vec<String> {
        let mut rows = vec![String::new(); row_count];
        if frame.is_empty() || row_count == 0 {
            return rows;
        }

        let bytes = frame.as_bytes();
        let mut idx = 0usize;
        let mut segment_start = 0usize;
        let mut active_row: Option<usize> = None;

        while idx < bytes.len() {
            if let Some((seq_len, row)) = Self::parse_goto_sequence(bytes, idx) {
                if let Some(target_row) = active_row {
                    if segment_start < idx {
                        rows[target_row].push_str(&frame[segment_start..idx]);
                    }
                }
                active_row = row.checked_sub(1).filter(|r| *r < row_count);
                segment_start = idx;
                idx += seq_len;
                continue;
            }
            idx += 1;
        }

        if let Some(target_row) = active_row {
            if segment_start < frame.len() {
                rows[target_row].push_str(&frame[segment_start..]);
            }
        }

        rows
    }

    pub(super) fn draw(&mut self, out: &mut impl Write) -> Result<(), String> {
        let (rows, cols) = input::terminal_size();
        let editor_height = rows.saturating_sub(2).max(1);
        let editor_bg = self.render_palette.surface_bg();
        self.ensure_calc_for_viewport(editor_height, false);
        let gutter_width = self.gutter_width();
        let line_number_width = gutter_width.saturating_sub(2);
        let mut buf = std::mem::take(&mut self.render_state.draw_buf);
        buf.clear();

        let title = derive_title_from_lines(&self.editor.lines);
        let dirty_mark = if self.dirty { " [+]" } else { "" };
        let large_note_label = if self.large_note_reduced_features() {
            " LARGE-NOTE"
        } else {
            ""
        };
        let title_line = format!(
            " note  {}  {}{}{}",
            self.active_note.id, title, dirty_mark, large_note_label
        );
        let title_bg = self.render_palette.primary();
        draw_row_at_styled(
            &mut buf,
            TITLE_ROW,
            1,
            cols,
            &title_line,
            AnsiStyle {
                fg: Some(contrast_fg_for_bg(title_bg)),
                bg: Some(title_bg),
                bold: true,
                ..Default::default()
            },
        );

        let first_real_line = self
            .real_line_for_virtual(self.editor.scroll_line)
            .unwrap_or(self.editor.lines.len());
        let (fence_in_code_block, fence_lang) = self.fence_state_before_line(first_real_line);
        let mut ctx = render::RenderContext::with_syntax_mode(
            fence_in_code_block,
            fence_lang,
            self.render_state.plain_text_file || self.large_note_reduced_features(),
            self.render_state.file_language.clone(),
            self.render_palette,
        );
        let mut last_rendered_real = if first_real_line > 0 {
            Some(first_real_line - 1)
        } else {
            None
        };
        let mut cursor_line_override: Option<(String, usize)> = None;
        let now_ms = notifications::now_epoch_ms();

        for i in 0..editor_height {
            let row = EDITOR_TOP_ROW + i;
            draw_row_at_styled(
                &mut buf,
                row,
                1,
                cols,
                "",
                AnsiStyle {
                    bg: Some(editor_bg),
                    ..Default::default()
                },
            );
            let virtual_line = self.editor.scroll_line + i;
            if let Some(line_idx) = self.real_line_for_virtual(virtual_line) {
                if let Some(prev_real) = last_rendered_real {
                    if line_idx > prev_real + 1 {
                        ctx.advance_lines(&self.editor.lines[(prev_real + 1)..line_idx]);
                    }
                }
                last_rendered_real = Some(line_idx);

                let line_no = virtual_line + 1;
                let available = cols.saturating_sub(gutter_width);
                let is_cursor_line = line_idx == self.editor.cursor_line;
                let mut line_cursor_col = if is_cursor_line {
                    Some(self.editor.cursor_col)
                } else {
                    None
                };
                let mut calc_ghost = self
                    .calc
                    .results
                    .get(line_idx)
                    .and_then(|r| r.as_ref().map(|value| value.to_string()));
                let mut calc_ghost_override: Option<String> = None;
                let mut reminder_ghost_override: Option<String> = None;
                let mut reminder_strikethrough = false;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let mut wiki_link_underline_ranges: Vec<(usize, usize)> = Vec::new();
                let mut formula_segments: Vec<TableFormulaSegment> = Vec::new();
                let mut formula_segment_char_delta_prefix: Vec<isize> = Vec::new();
                // Set by table reflow when cursor line is reformatted; holds
                // output char positions of (left_pipe, right_pipe) for the
                // cursor cell in the reformatted string.
                let mut table_reflow_cell_pipes: Option<(usize, usize)> = None;
                let line_text = self.editor.lines[line_idx].clone();
                let mut rendered_line: Cow<'_, str> = Cow::Borrowed(line_text.as_str());
                let collapsed_hidden_count = self
                    .folds
                    .placeholder_hidden_lines
                    .get(line_idx)
                    .and_then(|entry| *entry);
                let is_fold_placeholder = collapsed_hidden_count.is_some();

                if let Some(hidden_count) = collapsed_hidden_count {
                    let suffix = if hidden_count == 1 { "" } else { "s" };
                    rendered_line = Cow::Borrowed(line_text.as_str());
                    calc_ghost = None;
                    reminder_ghost_override = Some(format!("{hidden_count} line{suffix} folded"));
                } else {
                    if !is_cursor_line {
                        let (rendered, underlines) = self.render_wiki_link_display_line(&line_text);
                        rendered_line = Cow::Owned(rendered);
                        wiki_link_underline_ranges = underlines;
                    }
                    if let Some(reminder) = self.reminder_ghosts.get(&line_idx) {
                        reminder_ghost_override = Some(format!("⏰ {}", reminder.display_at));
                        reminder_strikethrough = reminder.remind_at_ms <= now_ms;
                    }

                    formula_segments = self.table_formula_segments_cached(&line_text);
                    if !formula_segments.is_empty() {
                        // Formula rows render a marker in-cell (`value*`,
                        // `value**`, …) and keep the detailed per-formula
                        // explanation as a line-end ghost. Per-cell values
                        // come from `cell_results`.
                        calc_ghost = None;

                        let cell_results = self
                            .calc
                            .cell_results
                            .get(line_idx)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        let mut cell_result_by_index: Vec<
                            Option<&app_core::calc::TableCellEvaluation>,
                        > = Vec::new();
                        for entry in cell_results {
                            if entry.cell_index >= cell_result_by_index.len() {
                                cell_result_by_index.resize(entry.cell_index + 1, None);
                            }
                            cell_result_by_index[entry.cell_index] = Some(entry);
                        }
                        let value_for_cell = |cell_index: usize| {
                            cell_result_by_index
                                .get(cell_index)
                                .and_then(|entry| *entry)
                        };

                        let mut out = String::with_capacity(line_text.len() + 16);
                        let mut last_byte = 0usize;
                        let mut char_delta: isize = 0;
                        let mut trailer_parts: Vec<String> = Vec::new();
                        formula_segment_char_delta_prefix.clear();
                        formula_segment_char_delta_prefix.push(0);
                        // Char position of the cursor in the rendered line; we
                        // collect this only when the cursor sits inside a
                        // focused (un-masked) formula cell.
                        let mut focused_cursor_col: Option<usize> = None;

                        for (fi, seg) in formula_segments.iter().enumerate() {
                            let marker = formula_marker_token(fi);
                            let eval = value_for_cell(seg.cell_index);
                            let value = eval
                                .map(|entry| format_formula_display_value(&entry.value))
                                .unwrap_or_else(|| String::from("…"));
                            let has_error =
                                eval.and_then(|entry| entry.error_kind.as_ref()).is_some();
                            let source_text =
                                line_text[seg.from_byte..seg.to_byte].trim().to_string();

                            let is_focused = is_cursor_line
                                && self.editor.cursor_col >= seg.cell_from_char
                                && self.editor.cursor_col < seg.cell_to_char;

                            // Ghost trailer: focused cell shows the value
                            // (so the user can see the result while editing),
                            // resting cells show the formula source.
                            let trailer_text = if let Some(err) = Self::table_error_text(
                                eval.and_then(|entry| entry.error_kind.as_ref()),
                                eval.map(|entry| entry.value.as_str()).unwrap_or(""),
                            ) {
                                err
                            } else if is_focused && !has_error {
                                value.clone()
                            } else {
                                source_text
                            };
                            if !trailer_text.is_empty() {
                                trailer_parts.push(format!("{marker} ➜ {trailer_text}"));
                            }

                            out.push_str(&line_text[last_byte..seg.from_byte]);

                            if is_focused {
                                out.push_str(&line_text[seg.from_byte..seg.to_byte]);
                                let mapped =
                                    (self.editor.cursor_col as isize + char_delta).max(0) as usize;
                                focused_cursor_col = Some(mapped);
                                formula_segment_char_delta_prefix.push(char_delta);
                            } else {
                                let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                let replacement = Self::fit_formula_marker_replacement(
                                    &value, &marker, old_chars, has_error,
                                );
                                let rendered_chars = replacement.chars().count();
                                let marker_char = ((seg.from_char as isize) + char_delta) as usize
                                    + rendered_chars.saturating_sub(marker.chars().count());
                                let marker_end = marker_char + marker.chars().count();
                                ghost_dim_ranges.push((marker_char, marker_end));
                                char_delta += rendered_chars as isize - old_chars as isize;
                                formula_segment_char_delta_prefix.push(char_delta);
                                out.push_str(&replacement);
                            }
                            last_byte = seg.to_byte;
                        }
                        out.push_str(&line_text[last_byte..]);
                        rendered_line = Cow::Owned(out);

                        calc_ghost_override = Some(trailer_parts.join("  "));

                        if is_cursor_line {
                            let mapped_col = focused_cursor_col.unwrap_or_else(|| {
                                // Cursor is outside every formula cell. Walk
                                // the segments that lie entirely before the
                                // cursor and accumulate their rendered-vs-
                                // source char delta.
                                let seg_count = formula_segments
                                    .iter()
                                    .take_while(|seg| seg.cell_to_char <= self.editor.cursor_col)
                                    .count();
                                let delta = formula_segment_char_delta_prefix
                                    .get(seg_count)
                                    .copied()
                                    .unwrap_or(0);
                                ((self.editor.cursor_col as isize) + delta).max(0) as usize
                            });
                            line_cursor_col = Some(mapped_col);
                            cursor_line_override =
                                Some((rendered_line.as_ref().to_string(), mapped_col));
                        }
                    }
                }

                // Table display reflow: collapse inline markers per cell and
                // align columns to visible widths. Skipped for formula rows
                // (already transformed above) and fold placeholders.
                if !is_fold_placeholder
                    && formula_segments.is_empty()
                    && is_markdown_table_line(rendered_line.as_ref())
                {
                    let col_widths = self.table_display_col_widths_for_line(line_idx);
                    if !col_widths.is_empty() {
                        if is_cursor_line && cursor_line_override.is_none() {
                            // Cursor row: render from a RAW-marker reflow so
                            // `render_line` still applies inline styles (bold,
                            // italic, code) and reveals markers near the cursor —
                            // collapsing here would strip `**` and lose bold.
                            // Cursor-column positioning uses a separate
                            // cursor-aware collapsed reflow (markers removed),
                            // matching what `render_line` actually displays.
                            let cursor = Some(line_cursor_col.unwrap_or(self.editor.cursor_col));
                            let (raw_display, _, _) = reformat_table_row_for_display(
                                rendered_line.as_ref(),
                                &col_widths,
                                None,
                            );
                            let (collapsed_display, mapped_col, _) = reformat_table_row_for_display(
                                rendered_line.as_ref(),
                                &col_widths,
                                cursor,
                            );
                            // Focused-cell pipe highlight in RAW display coords.
                            let cell_idx = table_cursor_cell_index(
                                rendered_line.as_ref(),
                                self.editor.cursor_col,
                            );
                            table_reflow_cell_pipes = cell_idx
                                .and_then(|idx| display_cell_pipe_positions(&raw_display, idx));
                            let mc = mapped_col.unwrap_or(line_cursor_col.unwrap_or(0));
                            line_cursor_col = Some(mc);
                            cursor_line_override = Some((collapsed_display, mc));
                            rendered_line = Cow::Owned(raw_display);
                        } else {
                            let (display_line, _, _) = reformat_table_row_for_display(
                                rendered_line.as_ref(),
                                &col_widths,
                                None,
                            );
                            rendered_line = Cow::Owned(display_line);
                        }
                    }
                }

                if !is_fold_placeholder && (!is_cursor_line || cursor_line_override.is_none()) {
                    let media_transform = media_sources::collapse_media_sources_for_display(
                        rendered_line.as_ref(),
                        line_cursor_col,
                    );
                    if media_transform.changed {
                        rendered_line = Cow::Owned(media_transform.rendered_line);
                        line_cursor_col = media_transform.mapped_cursor_col;
                    }
                }

                let (search_ranges, current_search_ranges) = if is_fold_placeholder {
                    (Vec::new(), Vec::new())
                } else {
                    self.search_highlights_for_line(line_idx)
                };
                let mut visual_highlight_ranges = Vec::new();
                if is_fold_placeholder {
                    if self.line_is_in_visual_selection(line_idx) {
                        visual_highlight_ranges
                            .push((0, rendered_line.as_ref().chars().count().max(1)));
                    }
                } else {
                    self.append_visual_highlights(line_idx, &mut visual_highlight_ranges);
                }

                if is_cursor_line && cursor_line_override.is_none() && !is_fold_placeholder {
                    let source_cursor_col = line_cursor_col.unwrap_or(self.editor.cursor_col);
                    let force_formatting_boundary_exit = self
                        .editor
                        .markdown_formatting_right_boundary_exit
                        .is_some_and(|(line, _)| line == line_idx);
                    let (collapsed_line, mapped_col) =
                        render::collapse_markdown_line_for_cursor_with_formatting_boundary_exit(
                            rendered_line.as_ref(),
                            source_cursor_col,
                            force_formatting_boundary_exit,
                        );
                    if collapsed_line != rendered_line.as_ref() || mapped_col != source_cursor_col {
                        cursor_line_override = Some((collapsed_line, mapped_col));
                    }
                }

                if is_cursor_line
                    && cursor_line_override.is_none()
                    && (rendered_line.as_ref() != line_text.as_str()
                        || line_cursor_col.unwrap_or(self.editor.cursor_col)
                            != self.editor.cursor_col)
                {
                    cursor_line_override = Some((
                        rendered_line.as_ref().to_string(),
                        line_cursor_col.unwrap_or(self.editor.cursor_col),
                    ));
                }

                let effective_calc_ghost = calc_ghost_override.as_deref().or(calc_ghost.as_deref());
                let effective_reminder_ghost = reminder_ghost_override.as_deref();
                let render_cursor_col = if is_cursor_line {
                    line_cursor_col
                } else {
                    None
                };
                let line_scroll_col = self.editor.scroll_col;
                let line_width = line_display_cols(rendered_line.as_ref());
                let viewport = compute_line_viewport(line_width, line_scroll_col, available);

                // Highlight the focused table cell's pipe characters in accent so
                // the active cell is obvious.
                //
                // When table reflow ran, pipe positions in `rendered_line` differ
                // from the source — use the output positions returned by the
                // reflow. Otherwise fall back to source positions translated by
                // the formula-mask delta.
                let mut focused_pipe_ranges: Vec<(usize, usize)> = Vec::new();
                if self.note_table_module_enabled() && is_cursor_line && !is_fold_placeholder {
                    if let Some((lp, rp)) = table_reflow_cell_pipes {
                        focused_pipe_ranges.push((lp, lp + 1));
                        focused_pipe_ranges.push((rp, rp + 1));
                    } else if let Some(info) = table_cell_info_at_char(
                        &self.editor.lines,
                        line_idx,
                        self.editor.cursor_col,
                    ) {
                        let left_pipe_char = line_text[..info.left_pipe].chars().count();
                        let right_pipe_char = line_text[..info.right_pipe].chars().count();
                        let translate = |src_col: usize| -> usize {
                            let seg_count = formula_segments
                                .iter()
                                .take_while(|seg| seg.cell_to_char <= src_col)
                                .count();
                            let delta = formula_segment_char_delta_prefix
                                .get(seg_count)
                                .copied()
                                .unwrap_or(0);
                            ((src_col as isize) + delta).max(0) as usize
                        };
                        let lp = translate(left_pipe_char);
                        let rp = translate(right_pipe_char);
                        focused_pipe_ranges.push((lp, lp + 1));
                        focused_pipe_ranges.push((rp, rp + 1));
                    }
                }

                let rendered_text = if ghost_dim_ranges.is_empty()
                    && visual_highlight_ranges.is_empty()
                    && focused_pipe_ranges.is_empty()
                    && wiki_link_underline_ranges.is_empty()
                {
                    ctx.render_line_window_with_reminder_cursor(
                        &rendered_line,
                        viewport.text_width,
                        viewport.text_window_col,
                        effective_calc_ghost,
                        effective_reminder_ghost,
                        reminder_strikethrough,
                        &search_ranges,
                        &current_search_ranges,
                        &self.calc.variable_names,
                        render_cursor_col,
                    )
                } else {
                    ctx.render_line_full(
                        rendered_line.as_ref(),
                        viewport.text_width,
                        viewport.text_window_col,
                        effective_calc_ghost,
                        effective_reminder_ghost,
                        reminder_strikethrough,
                        &search_ranges,
                        &current_search_ranges,
                        &self.calc.variable_names,
                        &ghost_dim_ranges,
                        &visual_highlight_ranges,
                        &focused_pipe_ranges,
                        &wiki_link_underline_ranges,
                        render_cursor_col,
                    )
                };
                buf.push_str(&goto(row, 1));
                let gutter_style = if is_cursor_line {
                    AnsiStyle {
                        fg: Some(self.render_palette.variable),
                        bg: Some(editor_bg),
                        bold: true,
                        ..Default::default()
                    }
                } else {
                    AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        bg: Some(editor_bg),
                        dim: true,
                        ..Default::default()
                    }
                };
                gutter_style.write_to(&mut buf);
                buf.push_str(&format!("{line_no:>line_number_width$}  "));
                buf.push_str(render::RESET);
                if viewport.has_left_overflow {
                    let indicator_style = AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        bg: Some(editor_bg),
                        dim: true,
                        ..Default::default()
                    };
                    indicator_style.write_to(&mut buf);
                    buf.push(OVERFLOW_LEFT_MARKER);
                    buf.push_str(render::RESET);
                }
                AnsiStyle {
                    bg: Some(editor_bg),
                    ..Default::default()
                }
                .write_to(&mut buf);
                buf.push_str(&rendered_text);
                if viewport.has_right_overflow {
                    let indicator_style = AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        bg: Some(editor_bg),
                        dim: true,
                        ..Default::default()
                    };
                    indicator_style.write_to(&mut buf);
                    buf.push(OVERFLOW_RIGHT_MARKER);
                    buf.push_str(render::RESET);
                }
            } else {
                buf.push_str(&goto(row, 1));
                AnsiStyle {
                    fg: Some(self.render_palette.code_comment),
                    bg: Some(editor_bg),
                    dim: true,
                    ..Default::default()
                }
                .write_to(&mut buf);
                buf.push_str(&pad_right("~", cols));
                buf.push_str(render::RESET);
            }
        }

        let editor_status_owned = if self.mode == UiMode::Editor {
            // Keep status bar stable during wiki-link popup usage; the popup
            // itself already renders suggestions and selection state.
            self.variable_autocomplete_status_hint()
                .map(|hint| format!("{}  [{}]", self.status, hint))
        } else {
            None
        };
        let status_base = match self.mode {
            UiMode::Editor => editor_status_owned.as_deref().unwrap_or(&self.status),
            UiMode::Normal
            | UiMode::CommandBar
            | UiMode::Search
            | UiMode::Visual
            | UiMode::VisualLine => &self.status,
            UiMode::Switcher => {
                if self.switcher.open_confirm.is_some() {
                    "Open note: type password, Enter confirm, Esc cancel"
                } else if let Some(confirm) = self.switcher.delete_confirm.as_ref() {
                    if confirm.requires_password {
                        "Confirm delete: type password, Enter confirm, Esc cancel"
                    } else {
                        "Confirm delete: Enter/Y confirm, Esc/N cancel"
                    }
                } else {
                    &self.status
                }
            }
            UiMode::CollectionSwitcher => {
                if self.collection_switcher.edit_dialog.is_some() {
                    "Collection edit: Tab/Shift+Tab field, Enter save, Esc cancel"
                } else {
                    &self.status
                }
            }
            UiMode::ContentSearch => {
                if self.switcher.open_confirm.is_some() {
                    "Open note: type password, Enter confirm, Esc cancel"
                } else {
                    &self.status
                }
            }
            UiMode::DatePicker => {
                "Date picker: arrows navigate, Ctrl+arrows months, Enter insert, Esc cancel"
            }
        };
        let collection_sticky = self.working_collection_status_suffix();
        let status_bg = self.render_palette.primary();
        let status_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(status_bg)),
            bg: Some(status_bg),
            ..Default::default()
        };
        if !self.draw_command_completion_status_row(
            &mut buf,
            rows,
            cols,
            status_bg,
            &collection_sticky,
        ) {
            self.draw_status_row_with_right_sticky(
                &mut buf,
                rows,
                cols,
                status_base,
                &collection_sticky,
                status_style,
            );
        }

        if self.mode == UiMode::Switcher {
            switcher::draw_switcher(
                &SwitcherView {
                    query: &self.switcher.query,
                    items: &self.switcher.items,
                    matches: &self.switcher.matches,
                    selected: self.switcher.selected,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(confirm) = self.switcher.delete_confirm.as_ref() {
                switcher::draw_delete_confirm(
                    &confirm.note_title,
                    confirm.requires_password,
                    confirm.password.chars().count(),
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
            if let Some(confirm) = self.switcher.open_confirm.as_ref() {
                switcher::draw_open_confirm(
                    &confirm.note_title,
                    confirm.password.chars().count(),
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
        }

        if self.mode == UiMode::CollectionSwitcher {
            switcher::draw_collection_switcher(
                &CollectionSwitcherView {
                    query: &self.collection_switcher.query,
                    items: &self.collection_switcher.items,
                    matches: &self.collection_switcher.matches,
                    selected: self.collection_switcher.selected,
                    working_collection_id: self.working_collection_id.as_deref(),
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(dialog) = self.collection_switcher.edit_dialog.as_ref() {
                switcher::draw_collection_edit_dialog(
                    &CollectionEditView {
                        name: &dialog.name,
                        description: &dialog.description,
                        default_tags: &dialog.default_tags,
                        selected_field: dialog.selected_field,
                    },
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
        }

        if self.mode == UiMode::ContentSearch {
            switcher::draw_content_search(
                &ContentSearchView {
                    query: &self.content_search.query,
                    results: &self.content_search.results,
                    selected: self.content_search.selected,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(confirm) = self.switcher.open_confirm.as_ref() {
                switcher::draw_open_confirm(
                    &confirm.note_title,
                    confirm.password.chars().count(),
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
        }

        if self.mode == UiMode::DatePicker {
            date_picker::draw_date_picker(
                &DatePickerView {
                    year: self.date_picker.year,
                    month: self.date_picker.month,
                    day: self.date_picker.day,
                    hour: self.date_picker.hour,
                    minute: self.date_picker.minute,
                    include_time: self.date_picker.include_time,
                    require_time: self.date_picker.require_time,
                    is_remind: self.date_picker.action == DatePickerAction::SetRemind,
                    date_format: &self.date_picker.format,
                    date_time_format: &self.date_picker.time_format,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
        }
        self.draw_variable_autocomplete_popup(&mut buf, rows, cols);
        self.draw_wiki_link_autocomplete_popup(&mut buf, rows, cols);
        self.draw_wiki_link_preview_popup(&mut buf, rows, cols);

        let (cursor_row, mut cursor_col) = self.cursor_position(rows, cols);
        if let Some((line_text, mapped_col)) = cursor_line_override {
            if matches!(
                self.mode,
                UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
            ) {
                let available = cols.saturating_sub(gutter_width);
                let display_char_col = cursor_render_char_col(
                    &line_text,
                    mapped_col,
                    matches!(
                        self.mode,
                        UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                    ),
                );
                let (display_col, line_width) =
                    display_cols_prefix_and_total(&line_text, display_char_col);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.editor.scroll_col,
                    available,
                );
                cursor_col = (gutter_width + visible_col + 1).min(cols.max(1)).max(1);
            }
        }
        let cursor_block = matches!(
            self.mode,
            UiMode::Normal | UiMode::Visual | UiMode::VisualLine
        );
        let cursor_style = if cursor_block {
            "\x1b[1 q" // Blinking Block
        } else {
            "\x1b[5 q" // Blinking Bar
        };

        let row_chunks = Self::split_frame_rows(&buf, rows);
        let dims_changed = self.render_state.last_drawn_rows_dim != (rows, cols)
            || self.render_state.last_drawn_rows.len() != row_chunks.len();
        let overlay_active = matches!(
            self.mode,
            UiMode::Switcher
                | UiMode::CollectionSwitcher
                | UiMode::ContentSearch
                | UiMode::DatePicker
        ) || self.switcher.open_confirm.is_some()
            || self.switcher.delete_confirm.is_some();
        // Full-screen repaint only on overlay transitions (open/close) so
        // content search remains stable while typing.
        let overlay_transition = overlay_active != self.render_state.last_draw_had_overlay;
        let force_full_redraw = dims_changed || overlay_transition;

        let mut changed_rows = Vec::new();
        if force_full_redraw {
            changed_rows.extend(0..row_chunks.len());
        } else {
            for (row_idx, row_text) in row_chunks.iter().enumerate() {
                if self
                    .render_state
                    .last_drawn_rows
                    .get(row_idx)
                    .map(|prev| prev != row_text)
                    .unwrap_or(true)
                {
                    changed_rows.push(row_idx);
                }
            }
        }

        let cursor_changed = dims_changed
            || self.render_state.last_cursor_row != cursor_row
            || self.render_state.last_cursor_col != cursor_col
            || self.render_state.last_cursor_block != cursor_block;

        if changed_rows.is_empty() && !cursor_changed {
            self.render_state.last_draw_had_overlay = overlay_active;
            self.render_state.draw_buf = buf;
            return Ok(());
        }

        let mut out_buf = String::new();
        out_buf.push_str("\x1b[?25l");
        if force_full_redraw {
            out_buf.push_str("\x1b[2J\x1b[H");
        }
        for row_idx in changed_rows {
            if let Some(chunk) = row_chunks.get(row_idx) {
                out_buf.push_str(&goto(row_idx + 1, 1));
                out_buf.push_str("\x1b[2K");
                out_buf.push_str(chunk);
            }
        }
        out_buf.push_str(&goto(cursor_row, cursor_col));
        out_buf.push_str(cursor_style);
        out_buf.push_str("\x1b[?25h");

        let result = out
            .write_all(out_buf.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to draw terminal UI: {e}"));
        if result.is_ok() {
            self.render_state.last_drawn_rows = row_chunks;
            self.render_state.last_drawn_rows_dim = (rows, cols);
            self.render_state.last_cursor_row = cursor_row;
            self.render_state.last_cursor_col = cursor_col;
            self.render_state.last_cursor_block = cursor_block;
            self.render_state.last_draw_had_overlay = overlay_active;
        }
        self.render_state.draw_buf = buf;
        result
    }

    pub(super) fn cursor_position(&self, rows: usize, cols: usize) -> (usize, usize) {
        match self.mode {
            UiMode::CommandBar => {
                let col = (1 + 1 + self.command_input.chars().count()).min(cols.max(1));
                (rows, col.max(1))
            }
            UiMode::Search => {
                let col = (1 + 1 + self.search.query.chars().count()).min(cols.max(1));
                (rows, col.max(1))
            }
            UiMode::DatePicker => {
                // Hide cursor inside the date picker
                (1, 1)
            }
            UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine => {
                let cursor_virtual = self.current_virtual_line();
                let row = EDITOR_TOP_ROW
                    + cursor_virtual
                        .saturating_sub(self.editor.scroll_line)
                        .min(rows.saturating_sub(2));
                let line_text = self.current_line();
                let display_char_col = cursor_render_char_col(
                    line_text,
                    self.editor.cursor_col,
                    matches!(
                        self.mode,
                        UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                    ),
                );
                let gutter_width = self.gutter_width();
                let available = cols.saturating_sub(gutter_width);
                let (display_col, line_width) =
                    display_cols_prefix_and_total(line_text, display_char_col);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.editor.scroll_col,
                    available,
                );
                let col = (gutter_width + visible_col + 1).min(cols.max(1));
                (row.max(1), col.max(1))
            }
            UiMode::Switcher => {
                let box_w = min(cols.saturating_sub(4).max(30), 72);
                let box_h = min(rows.saturating_sub(4).max(8), 14);
                let x = (cols.saturating_sub(box_w)) / 2 + 1;
                let y = (rows.saturating_sub(box_h)) / 2 + 1;
                let prompt = " search: ";
                let col = (x + 1 + prompt.chars().count() + self.switcher.query.chars().count())
                    .min(cols.max(1));
                (y + 1, col.max(1))
            }
            UiMode::CollectionSwitcher => {
                let box_w = min(cols.saturating_sub(4).max(30), 72);
                let box_h = min(rows.saturating_sub(4).max(8), 14);
                let x = (cols.saturating_sub(box_w)) / 2 + 1;
                let y = (rows.saturating_sub(box_h)) / 2 + 1;
                if let Some(dialog) = self.collection_switcher.edit_dialog.as_ref() {
                    let edit_w = min(cols.saturating_sub(4).max(48), 88);
                    let edit_h = min(rows.saturating_sub(4).max(10), 12);
                    let edit_x = (cols.saturating_sub(edit_w)) / 2 + 1;
                    let edit_y = (rows.saturating_sub(edit_h)) / 2 + 1;
                    let label_width = match dialog.selected_field {
                        0 => " Name: ".chars().count(),
                        1 => " Description: ".chars().count(),
                        _ => " Default tags: ".chars().count(),
                    };
                    let text_len = match dialog.selected_field {
                        0 => dialog.name.chars().count(),
                        1 => dialog.description.chars().count(),
                        _ => dialog.default_tags.chars().count(),
                    };
                    let col = (edit_x + 1 + label_width + text_len).min(cols.max(1));
                    let row = edit_y + 4 + dialog.selected_field.min(2);
                    (row.max(1), col.max(1))
                } else {
                    let prompt = " collections: ";
                    let col = (x
                        + 1
                        + prompt.chars().count()
                        + self.collection_switcher.query.chars().count())
                    .min(cols.max(1));
                    (y + 1, col.max(1))
                }
            }
            UiMode::ContentSearch => {
                let (x, y, _box_w, _box_h) = content_search_box_geometry(rows, cols);
                let prompt = " content: ";
                let col = (x + 1 + prompt.chars().count() + self.content_search.cursor_col)
                    .min(cols.max(1));
                (y + 1, col.max(1))
            }
        }
    }
}

fn preview_wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        if paragraph.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

fn preview_truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{truncated}…")
    }
}
