use super::{
    contrast_fg_for_bg, cursor_render_char_col, display_cols_for_prefix, draw_box_border,
    draw_row_at_styled, find_table_formula_segments, format_formula_display_value,
    formula_marker_token, goto, line_display_cols, min, pad_right, table_cell_info_at_char,
    viewport_col_for_display_col, AnsiStyle, DatePickerAction, TableFormulaSegment, TerminalApp,
    UiMode, EDITOR_TOP_ROW, OVERFLOW_LEFT_MARKER, OVERFLOW_RIGHT_MARKER, TITLE_ROW,
};
use crate::terminal::render;
use crate::terminal::text_utils::{compute_line_viewport, derive_title_from_lines};
use crate::terminal::{date_picker, input, notifications, switcher};
use crate::terminal::{
    date_picker::DatePickerView,
    switcher::{content_search_box_geometry, ContentSearchView, SwitcherView},
};
use std::io::Write;

// Ownership: status/popup composition and terminal rendering/cursor placement.
impl TerminalApp {
    fn wiki_link_cache_entry(&mut self, short_id: &str) -> (String, bool) {
        if let Some(entry) = self.wiki_link_render_cache.get(short_id) {
            if entry.cached_at.elapsed().as_millis() as u64 <= super::WIKI_LINK_RENDER_CACHE_TTL_MS
            {
                return (entry.display.clone(), entry.broken);
            }
            self.wiki_link_render_cache.remove(short_id);
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

        self.wiki_link_render_cache.insert(
            short_id.to_string(),
            super::WikiLinkRenderCacheEntry {
                display: display.clone(),
                broken,
                cached_at: std::time::Instant::now(),
            },
        );

        while self.wiki_link_render_cache.len() > super::WIKI_LINK_RENDER_CACHE_MAX_ENTRIES {
            let Some(oldest_key) = self
                .wiki_link_render_cache
                .iter()
                .min_by_key(|(_, entry)| entry.cached_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.wiki_link_render_cache.remove(oldest_key.as_str());
        }

        (display, broken)
    }

    fn render_wiki_link_display_line(&mut self, line_text: &str) -> String {
        let links = crate::editor_core::markdown_tokens::find_wiki_link_matches(line_text);
        if links.is_empty() {
            return line_text.to_string();
        }

        let chars: Vec<char> = line_text.chars().collect();
        let mut out = String::with_capacity(line_text.len() + 16);
        let mut cursor = 0usize;

        for link in links {
            if link.title.is_some() || link.from < cursor || link.to > chars.len() {
                continue;
            }
            out.extend(chars[cursor..link.from].iter());
            let (base, broken) = self.wiki_link_cache_entry(&link.short_id);
            if broken {
                out.push('?');
            } else {
                out.push_str(base.as_str());
            }
            if let Some(heading) = link.heading.as_deref().filter(|value| !value.is_empty()) {
                out.push('#');
                out.push_str(heading);
            }
            cursor = link.to;
        }

        if cursor == 0 {
            return line_text.to_string();
        }
        out.extend(chars[cursor..].iter());
        out
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

        let mut col = 1usize;
        Self::draw_status_segment(
            buf,
            row,
            &mut col,
            cols,
            &format!(":{}  [", self.command_input),
            base_style,
        );
        for (idx, option) in self.command_completion.options.iter().enumerate() {
            if idx > 0 {
                Self::draw_status_segment(buf, row, &mut col, cols, "  ", base_style);
            }
            let style = if idx == selected_idx {
                selected_style
            } else {
                base_style
            };
            Self::draw_status_segment(buf, row, &mut col, cols, &option.token, style);
        }
        Self::draw_status_segment(buf, row, &mut col, cols, "]", base_style);
        true
    }

    pub(super) fn search_highlights_for_line(
        &self,
        line_idx: usize,
    ) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
        let mut matches = Vec::new();
        let mut current = Vec::new();
        for (idx, &(line, start, end)) in self.search_matches.iter().enumerate() {
            if line != line_idx {
                continue;
            }
            if idx == self.search_current {
                current.push((start, end));
            } else {
                matches.push((start, end));
            }
        }
        (matches, current)
    }

    pub(super) fn line_is_in_visual_selection(&self, line_idx: usize) -> bool {
        let Some(anchor) = self.selection_anchor else {
            return false;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return false;
        }

        let start_line = min(anchor.0, self.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.cursor_line);
        line_idx >= start_line && line_idx <= end_line
    }

    pub(super) fn append_visual_highlights(
        &self,
        line_idx: usize,
        ranges: &mut Vec<(usize, usize)>,
    ) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return;
        }

        let start_line = min(anchor.0, self.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.cursor_line);

        if line_idx < start_line || line_idx > end_line {
            return;
        }

        let linewise = self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection_linewise);
        if linewise {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((0, line_len.max(1)));
            return;
        }

        let (start_col, end_col) = if anchor.0 == self.cursor_line {
            (
                min(anchor.1, self.cursor_col),
                std::cmp::max(anchor.1, self.cursor_col),
            )
        } else if anchor.0 < self.cursor_line {
            (anchor.1, self.cursor_col)
        } else {
            (self.cursor_col, anchor.1)
        };

        if start_line == end_line {
            ranges.push((start_col, end_col + 1));
        } else if line_idx == start_line {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((start_col, line_len.max(start_col + 1)));
        } else if line_idx == end_line {
            ranges.push((0, end_col + 1));
        } else {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((0, line_len.max(1)));
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
                .saturating_sub(box_height.saturating_sub(1));
        }
        y = y
            .max(EDITOR_TOP_ROW)
            .min(max_editor_row.saturating_sub(box_height.saturating_sub(1)));

        let border_style = AnsiStyle {
            fg: Some(self.render_palette.code_type),
            ..Default::default()
        };
        let row_style = AnsiStyle {
            fg: Some(self.render_palette.variable),
            ..Default::default()
        };
        let selected_bg = self.render_palette.primary();
        let selected_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(selected_bg)),
            bg: Some(selected_bg),
            bold: true,
            ..Default::default()
        };

        draw_box_border(buf, y, x, box_width, box_height, border_style);

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
        let suggestions: Vec<String> = self
            .filtered_wiki_link_suggestions()
            .into_iter()
            .map(|s| s.title.clone())
            .collect();
        if suggestions.is_empty() {
            return;
        }
        let max_editor_row = rows.saturating_sub(1);
        let available_rows = max_editor_row.saturating_sub(EDITOR_TOP_ROW) + 1;
        if available_rows < 3 {
            return;
        }
        let visible_count = suggestions.len().min(available_rows.saturating_sub(2).max(1));
        let selected_index = popup.selected_index.min(visible_count.saturating_sub(1));
        let inner_width = suggestions
            .iter()
            .take(visible_count)
            .map(|t| t.chars().count() + 2)
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
            y = popup.anchor_row.saturating_sub(box_height.saturating_sub(1));
        }
        y = y.max(EDITOR_TOP_ROW).min(max_editor_row.saturating_sub(box_height.saturating_sub(1)));
        let border_style = AnsiStyle { fg: Some(self.render_palette.code_type), ..Default::default() };
        let row_style = AnsiStyle { fg: Some(self.render_palette.code_string), ..Default::default() };
        let selected_bg = self.render_palette.primary();
        let selected_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(selected_bg)),
            bg: Some(selected_bg),
            bold: true,
            ..Default::default()
        };
        draw_box_border(buf, y, x, box_width, box_height, border_style);
        for (idx, title) in suggestions.iter().take(visible_count).enumerate() {
            let row = y + 1 + idx;
            let text = format!(" {title}");
            let style = if idx == selected_index { selected_style } else { row_style };
            draw_row_at_styled(buf, row, x + 1, box_width.saturating_sub(2), &text, style);
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
        self.ensure_calc_for_viewport(editor_height, false);
        let gutter_width = self.gutter_width();
        let line_number_width = gutter_width.saturating_sub(2);
        let mut buf = std::mem::take(&mut self.draw_buf);
        buf.clear();

        let title = derive_title_from_lines(&self.lines);
        let dirty_mark = if self.dirty { " [+]" } else { "" };
        let mode_label = match self.mode {
            UiMode::Normal => "",
            UiMode::Editor => " INSERT",
            UiMode::Visual => " VISUAL",
            UiMode::VisualLine => " V-LINE",
            UiMode::CommandBar => " CMD",
            UiMode::Search => " SEARCH",
            UiMode::Switcher => " SWITCH",
            UiMode::ContentSearch => " SEARCH",
            UiMode::DatePicker => " DATE",
        };
        let title_line = format!(
            " note  {}  {}{}{}",
            self.active_note.id, title, dirty_mark, mode_label
        );
        let title_bg = self.render_palette.search_match;
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
            .real_line_for_virtual(self.scroll_line)
            .unwrap_or(self.lines.len());
        let (fence_in_code_block, fence_lang) = self.fence_state_before_line(first_real_line);
        let mut ctx = render::RenderContext::with_fence_state(
            fence_in_code_block,
            fence_lang,
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
            let virtual_line = self.scroll_line + i;
            if let Some(line_idx) = self.real_line_for_virtual(virtual_line) {
                if let Some(prev_real) = last_rendered_real {
                    if line_idx > prev_real + 1 {
                        ctx.advance_lines(&self.lines[(prev_real + 1)..line_idx]);
                    }
                }
                last_rendered_real = Some(line_idx);

                let line_no = virtual_line + 1;
                let available = cols.saturating_sub(gutter_width);
                let is_cursor_line = line_idx == self.cursor_line;
                let mut calc_ghost = self
                    .calc
                    .results
                    .get(line_idx)
                    .and_then(|r| r.as_ref().map(|value| value.to_string()));
                let mut calc_ghost_override: Option<String> = None;
                let mut reminder_ghost_override: Option<String> = None;
                let mut reminder_strikethrough = false;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let mut formula_segments: Vec<TableFormulaSegment> = Vec::new();
                let line_text = self.lines[line_idx].clone();
                let mut rendered_line = line_text.clone();
                let collapsed_hidden_count = self
                    .folds
                    .placeholder_hidden_lines
                    .get(line_idx)
                    .and_then(|entry| *entry);
                let is_fold_placeholder = collapsed_hidden_count.is_some();

                if let Some(hidden_count) = collapsed_hidden_count {
                    let suffix = if hidden_count == 1 { "" } else { "s" };
                    rendered_line = "".to_string();
                    calc_ghost = None;
                    reminder_ghost_override = Some(format!("{hidden_count} line{suffix} folded"));
                } else {
                    if !is_cursor_line {
                        rendered_line = self.render_wiki_link_display_line(&line_text);
                    }
                    if let Some(reminder) = self.reminder_ghosts.get(&line_idx) {
                        reminder_ghost_override = Some(format!("⏰ {}", reminder.display_at));
                        reminder_strikethrough = reminder.remind_at_ms <= now_ms;
                    }

                    formula_segments = find_table_formula_segments(&line_text);
                    if !formula_segments.is_empty() {
                        // Formula rows render a marker in-cell (`value*`,
                        // `value**`, …) and keep the detailed per-formula
                        // explanation as a line-end ghost. The first formula
                        // value in the row is also stored in `calc_results`
                        // for backward compatibility; per-cell values come
                        // from `cell_calc_results`.
                        calc_ghost = None;

                        let cell_results = self
                            .calc
                            .cell_results
                            .get(line_idx)
                            .cloned()
                            .unwrap_or_default();
                        let value_for_cell = |cell_index: usize| -> Option<String> {
                            cell_results
                                .iter()
                                .find(|(idx, _)| *idx == cell_index)
                                .map(|(_, v)| format_formula_display_value(v))
                                .or_else(|| {
                                    // Fallback: legacy single-result path.
                                    self.calc
                                        .results
                                        .get(line_idx)
                                        .and_then(|r| r.as_deref())
                                        .map(format_formula_display_value)
                                })
                        };

                        let mut out = String::with_capacity(line_text.len() + 16);
                        let mut last_byte = 0usize;
                        let mut char_delta: isize = 0;
                        let mut trailer_parts: Vec<String> = Vec::new();
                        // Char position of the cursor in the rendered line; we
                        // collect this only when the cursor sits inside a
                        // focused (un-masked) formula cell.
                        let mut focused_cursor_col: Option<usize> = None;

                        for (fi, seg) in formula_segments.iter().enumerate() {
                            let marker = formula_marker_token(fi);
                            let value =
                                value_for_cell(seg.cell_index).unwrap_or_else(|| String::from("…"));
                            let source_text =
                                line_text[seg.from_byte..seg.to_byte].trim().to_string();

                            let is_focused = is_cursor_line
                                && self.cursor_col >= seg.cell_from_char
                                && self.cursor_col < seg.cell_to_char;

                            // Ghost trailer: focused cell shows the value
                            // (so the user can see the result while editing),
                            // resting cells show the formula source.
                            let trailer_text = if is_focused {
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
                                    (self.cursor_col as isize + char_delta).max(0) as usize;
                                focused_cursor_col = Some(mapped);
                            } else {
                                let mut replacement = format!("{value}{marker}");
                                let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                let new_chars = replacement.chars().count();
                                if new_chars < old_chars {
                                    replacement.push_str(&" ".repeat(old_chars - new_chars));
                                }
                                let rendered_chars = replacement.chars().count();
                                let marker_char = ((seg.from_char as isize) + char_delta) as usize
                                    + value.chars().count();
                                let marker_end = marker_char + marker.chars().count();
                                ghost_dim_ranges.push((marker_char, marker_end));
                                char_delta += rendered_chars as isize - old_chars as isize;
                                out.push_str(&replacement);
                            }
                            last_byte = seg.to_byte;
                        }
                        out.push_str(&line_text[last_byte..]);
                        rendered_line = out;

                        calc_ghost_override = Some(trailer_parts.join("  "));

                        if is_cursor_line {
                            let mapped_col = focused_cursor_col.unwrap_or_else(|| {
                                // Cursor is outside every formula cell. Walk
                                // the segments that lie entirely before the
                                // cursor and accumulate their rendered-vs-
                                // source char delta.
                                let mut delta: isize = 0;
                                for (fi, seg) in formula_segments.iter().enumerate() {
                                    if seg.cell_to_char <= self.cursor_col {
                                        let value = value_for_cell(seg.cell_index)
                                            .unwrap_or_else(|| String::from("…"));
                                        let marker = formula_marker_token(fi);
                                        let mut rep = format!("{value}{marker}");
                                        let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                        let new_chars = rep.chars().count();
                                        if new_chars < old_chars {
                                            rep.push_str(&" ".repeat(old_chars - new_chars));
                                        }
                                        delta += rep.chars().count() as isize - old_chars as isize;
                                    }
                                }
                                ((self.cursor_col as isize) + delta).max(0) as usize
                            });
                            cursor_line_override = Some((rendered_line.clone(), mapped_col));
                        }
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
                        visual_highlight_ranges.push((0, rendered_line.chars().count().max(1)));
                    }
                } else {
                    self.append_visual_highlights(line_idx, &mut visual_highlight_ranges);
                }

                if is_cursor_line && cursor_line_override.is_none() && !is_fold_placeholder {
                    let (collapsed_line, mapped_col) =
                        render::collapse_markdown_line_for_cursor(&rendered_line, self.cursor_col);
                    if collapsed_line != rendered_line || mapped_col != self.cursor_col {
                        cursor_line_override = Some((collapsed_line, mapped_col));
                    }
                }

                let effective_calc_ghost = calc_ghost_override.as_deref().or(calc_ghost.as_deref());
                let effective_reminder_ghost = reminder_ghost_override.as_deref();
                let render_cursor_col = if is_cursor_line {
                    if !formula_segments.is_empty() {
                        cursor_line_override
                            .as_ref()
                            .map(|(_, mapped_col)| *mapped_col)
                            .or(Some(self.cursor_col))
                    } else {
                        Some(self.cursor_col)
                    }
                } else {
                    None
                };
                let line_scroll_col = self.scroll_col;
                let line_width = line_display_cols(&rendered_line);
                let viewport = compute_line_viewport(line_width, line_scroll_col, available);

                // Highlight the focused table cell's pipe characters in red so
                // the active cell is obvious. Pipe positions are taken from
                // the source `line_text` and translated to rendered char
                // positions using the formula-mask delta accumulated above.
                let mut focused_pipe_ranges: Vec<(usize, usize)> = Vec::new();
                if self.note_table_module_enabled() && is_cursor_line && !is_fold_placeholder {
                    if let Some(info) = table_cell_info_at_char(&line_text, self.cursor_col) {
                        let left_pipe_char = line_text[..info.left_pipe].chars().count();
                        let right_pipe_char = line_text[..info.right_pipe].chars().count();
                        let translate = |src_col: usize| -> usize {
                            let mut delta: isize = 0;
                            for (fi, seg) in formula_segments.iter().enumerate() {
                                if seg.cell_to_char <= src_col {
                                    let value = self
                                        .calc
                                        .cell_results
                                        .get(line_idx)
                                        .and_then(|row| {
                                            row.iter()
                                                .find(|(idx, _)| *idx == seg.cell_index)
                                                .map(|(_, v)| format_formula_display_value(v))
                                        })
                                        .unwrap_or_else(|| String::from("…"));
                                    let marker = formula_marker_token(fi);
                                    let mut rep = format!("{value}{marker}");
                                    let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                    let new_chars = rep.chars().count();
                                    if new_chars < old_chars {
                                        rep.push_str(&" ".repeat(old_chars - new_chars));
                                    }
                                    delta += rep.chars().count() as isize - old_chars as isize;
                                }
                            }
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
                        &rendered_line,
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
                        render_cursor_col,
                    )
                };
                buf.push_str(&goto(row, 1));
                let gutter_style = if is_cursor_line {
                    AnsiStyle {
                        fg: Some(self.render_palette.variable),
                        bold: true,
                        ..Default::default()
                    }
                } else {
                    AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
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
                        dim: true,
                        ..Default::default()
                    };
                    indicator_style.write_to(&mut buf);
                    buf.push(OVERFLOW_LEFT_MARKER);
                    buf.push_str(render::RESET);
                }
                buf.push_str(&rendered_text);
                if viewport.has_right_overflow {
                    let indicator_style = AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
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
                    dim: true,
                    ..Default::default()
                }
                .write_to(&mut buf);
                buf.push_str(&pad_right("~", cols));
                buf.push_str(render::RESET);
            }
        }

        let status_owned = if self.mode == UiMode::Editor {
            self.variable_autocomplete_status_hint()
                .map(|hint| format!("{}  [{}]", self.status, hint))
        } else {
            None
        };
        let status = match self.mode {
            UiMode::Editor => status_owned.as_deref().unwrap_or(&self.status),
            UiMode::Normal
            | UiMode::CommandBar
            | UiMode::Search
            | UiMode::Visual
            | UiMode::VisualLine => &self.status,
            UiMode::Switcher => {
                if self.switcher_open_confirm.is_some() {
                    "Open note: type password, Enter confirm, Esc cancel"
                } else if let Some(confirm) = self.switcher_delete_confirm.as_ref() {
                    if confirm.requires_password {
                        "Confirm delete: type password, Enter confirm, Esc cancel"
                    } else {
                        "Confirm delete: Enter/Y confirm, Esc/N cancel"
                    }
                } else {
                    "Switcher: type to filter, Enter open, Delete/Ctrl+Backspace delete, Esc close"
                }
            }
            UiMode::ContentSearch => {
                if self.switcher_open_confirm.is_some() {
                    "Open note: type password, Enter confirm, Esc cancel"
                } else {
                    "Content search: type to search, Enter open, Tab title search, Esc close"
                }
            }
            UiMode::DatePicker => {
                "Date picker: arrows navigate, Ctrl+arrows months, Enter insert, Esc cancel"
            }
        };
        let status_bg = self.render_palette.search_match;
        if !self.draw_command_completion_status_row(&mut buf, rows, cols, status_bg) {
            draw_row_at_styled(
                &mut buf,
                rows,
                1,
                cols,
                status,
                AnsiStyle {
                    fg: Some(contrast_fg_for_bg(status_bg)),
                    bg: Some(status_bg),
                    ..Default::default()
                },
            );
        }

        if self.mode == UiMode::Switcher {
            switcher::draw_switcher(
                &SwitcherView {
                    query: &self.switcher_query,
                    items: &self.switcher_items,
                    matches: &self.switcher_matches,
                    selected: self.switcher_selected,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(confirm) = self.switcher_delete_confirm.as_ref() {
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
            if let Some(confirm) = self.switcher_open_confirm.as_ref() {
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

        if self.mode == UiMode::ContentSearch {
            switcher::draw_content_search(
                &ContentSearchView {
                    query: &self.content_search_query,
                    results: &self.content_search_results,
                    selected: self.content_search_selected,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(confirm) = self.switcher_open_confirm.as_ref() {
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
                    year: self.date_year,
                    month: self.date_month,
                    day: self.date_day,
                    hour: self.date_hour,
                    minute: self.date_minute,
                    include_time: self.date_include_time,
                    require_time: self.date_require_time,
                    is_notify: self.date_picker_action == DatePickerAction::SetNotify,
                    date_format: &self.date_format,
                    date_time_format: &self.date_time_format,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
        }
        self.draw_variable_autocomplete_popup(&mut buf, rows, cols);
        self.draw_wiki_link_autocomplete_popup(&mut buf, rows, cols);

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
                let display_col = display_cols_for_prefix(&line_text, display_char_col);
                let line_width = line_display_cols(&line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
                    available,
                );
                cursor_col = (gutter_width + visible_col + 1).min(cols.max(1)).max(1);
            }
        }
        buf.push_str(&goto(cursor_row, cursor_col));

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
        let dims_changed = self.last_drawn_rows_dim != (rows, cols)
            || self.last_drawn_rows.len() != row_chunks.len();
        let overlay_active = matches!(
            self.mode,
            UiMode::Switcher | UiMode::ContentSearch | UiMode::DatePicker
        ) || self.switcher_open_confirm.is_some()
            || self.switcher_delete_confirm.is_some();
        // Full-screen repaint only on overlay transitions (open/close) so
        // content search remains stable while typing.
        let overlay_transition = overlay_active != self.last_draw_had_overlay;
        let force_full_redraw = dims_changed || overlay_transition;

        let mut changed_rows = Vec::new();
        if force_full_redraw {
            changed_rows.extend(0..row_chunks.len());
        } else {
            for (row_idx, row_text) in row_chunks.iter().enumerate() {
                if self
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
            || self.last_cursor_row != cursor_row
            || self.last_cursor_col != cursor_col
            || self.last_cursor_block != cursor_block;

        if changed_rows.is_empty() && !cursor_changed {
            self.last_draw_had_overlay = overlay_active;
            self.draw_buf = buf;
            return Ok(());
        }

        let mut out_buf = String::new();
        out_buf.push_str("\x1b[?25l");
        if force_full_redraw {
            out_buf.push_str("\x1b[2J\x1b[H");
        }
        for row_idx in changed_rows {
            if let Some(chunk) = row_chunks.get(row_idx) {
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
            self.last_drawn_rows = row_chunks;
            self.last_drawn_rows_dim = (rows, cols);
            self.last_cursor_row = cursor_row;
            self.last_cursor_col = cursor_col;
            self.last_cursor_block = cursor_block;
            self.last_draw_had_overlay = overlay_active;
        }
        self.draw_buf = buf;
        result
    }

    pub(super) fn cursor_position(&self, rows: usize, cols: usize) -> (usize, usize) {
        match self.mode {
            UiMode::CommandBar => {
                let col = (1 + 1 + self.command_input.chars().count()).min(cols.max(1));
                (rows, col.max(1))
            }
            UiMode::Search => {
                let col = (1 + 1 + self.search_query.chars().count()).min(cols.max(1));
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
                        .saturating_sub(self.scroll_line)
                        .min(rows.saturating_sub(2));
                let line_text = self.current_line();
                let display_char_col = cursor_render_char_col(
                    line_text,
                    self.cursor_col,
                    matches!(
                        self.mode,
                        UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                    ),
                );
                let gutter_width = self.gutter_width();
                let available = cols.saturating_sub(gutter_width);
                let display_col = display_cols_for_prefix(line_text, display_char_col);
                let line_width = line_display_cols(line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
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
                let col = (x + 1 + prompt.chars().count() + self.switcher_query.chars().count())
                    .min(cols.max(1));
                (y + 1, col.max(1))
            }
            UiMode::ContentSearch => {
                let (x, y, _box_w, _box_h) = content_search_box_geometry(rows, cols);
                let prompt = " content: ";
                let col =
                    (x + 1 + prompt.chars().count() + self.content_search_query.chars().count())
                        .min(cols.max(1));
                (y + 1, col.max(1))
            }
        }
    }
}
