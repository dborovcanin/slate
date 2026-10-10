//! Framework-independent styles. Character ranges use Unicode scalar columns.
use super::markdown::{
    hidden_line_prefix_marker_ranges, image_hidden_token_ranges, markdown_link_hidden_token_ranges,
    normalize_hidden_ranges, should_reveal_inline_marker, wiki_link_hidden_token_ranges,
};
use crate::variables::VariableNames;
use editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType, MarkdownLineInfo};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRole {
    Text,
    Heading,
    HiddenMarker,
    CalcResult,
    Surface,
    CodeBlock,
    Variable,
    CodeKeyword,
    CodeString,
    CodeNumber,
    CodeComment,
    CodeFunction,
    CodeType,
    SearchMatch,
    SearchCurrent,
    Primary,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SemanticStyle {
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub reverse: bool,
    pub fg: Option<SemanticRole>,
    pub bg: Option<SemanticRole>,
}
pub fn apply_variable_styles(
    chars: &[char],
    styles: &mut [SemanticStyle],
    variable_names: Option<&VariableNames>,
    variable_color: SemanticRole,
) {
    let Some(variable_names) = variable_names else {
        return;
    };
    if chars.is_empty() || variable_names.is_empty() {
        return;
    }

    let text: String = chars.iter().collect();
    let ascii = text.is_ascii();
    // `styles` is indexed by char; matches are byte ranges into `text`.
    let char_idx = |byte: usize| {
        if ascii {
            byte
        } else {
            text[..byte].chars().count()
        }
    };
    for (start, end) in variable_names.find_ranges(&text) {
        let (start, end) = (char_idx(start), char_idx(end));
        for style in styles.iter_mut().take(end).skip(start) {
            style.fg = Some(variable_color);
            style.bold = true;
            style.dim = false;
        }
    }
}

pub fn apply_line_styles_from_info(info: &MarkdownLineInfo, styles: &mut [SemanticStyle]) {
    let len = styles.len();

    if let (Some(level), Some(marker_end)) = (info.heading_level, info.heading_marker_end) {
        let _ = level;
        for (idx, style) in styles.iter_mut().enumerate() {
            if idx < marker_end.min(len) {
                style.dim = true;
            } else {
                style.bold = true;
                style.fg = Some(SemanticRole::Heading);
            }
        }
        return;
    }

    if let Some(marker_end) = info.quote_marker_end {
        for (idx, style) in styles.iter_mut().enumerate() {
            if idx < marker_end.min(len) {
                style.dim = true;
            } else {
                style.italic = true;
            }
        }
        return;
    }

    if info.is_horizontal_rule {
        for style in styles.iter_mut() {
            style.dim = true;
        }
        return;
    }

    if let Some(marker_end) = info.checklist_marker_end {
        for style in styles.iter_mut().take(marker_end.min(len)) {
            style.dim = true;
        }
        if info.checklist_checked {
            for style in styles.iter_mut().skip(marker_end.min(len)) {
                style.strikethrough = true;
                style.dim = true;
            }
        }
        return;
    }

    if let Some(marker_end) = info.list_marker_end {
        for style in styles.iter_mut().take(marker_end.min(len)) {
            style.dim = true;
        }
    }
}

pub fn apply_inline_token_styles(
    tokens: &[markdown_tokens::InlineToken],
    styles: &mut [SemanticStyle],
    hidden_ranges: &mut Vec<(usize, usize)>,
    active_cursor_col: Option<usize>,
) {
    let len = styles.len();
    let component_ranges = markdown_tokens::inline_marker_component_ranges_from_tokens(tokens);
    hidden_ranges.extend(image_hidden_token_ranges(tokens, active_cursor_col));
    hidden_ranges.extend(markdown_link_hidden_token_ranges(tokens, active_cursor_col));
    hidden_ranges.extend(wiki_link_hidden_token_ranges(tokens, active_cursor_col));
    for (index, token) in tokens.iter().enumerate() {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }

        if markdown_tokens::is_inline_marker_token_kind(token.kind)
            && !should_reveal_inline_marker(tokens, &component_ranges, index, active_cursor_col)
        {
            hidden_ranges.push((from, to));
            continue;
        }

        for style in styles.iter_mut().take(to).skip(from) {
            match token.kind {
                InlineTokenType::Strong => style.bold = true,
                InlineTokenType::Emphasis => style.italic = true,
                InlineTokenType::Strikethrough => style.strikethrough = true,
                InlineTokenType::Code | InlineTokenType::CodeMarker => style.dim = true,
                InlineTokenType::ImageAlt => {
                    style.italic = true;
                    style.bold = true;
                }
                InlineTokenType::ImageSrc | InlineTokenType::ImageMarker => style.dim = true,
                InlineTokenType::LinkText => {
                    style.bold = true;
                    style.underline = true;
                }
                InlineTokenType::LinkUrl | InlineTokenType::LinkMarker => style.dim = true,
                InlineTokenType::WikiLinkTitle => {
                    style.bold = true;
                    style.underline = true;
                }
                InlineTokenType::WikiLinkMarker
                | InlineTokenType::WikiLinkId
                | InlineTokenType::WikiLinkSep
                | InlineTokenType::WikiLinkAnchor => style.dim = true,
            }
        }
    }
}

pub fn apply_code_token_styles(
    tokens: &[markdown_tokens::CodeToken],
    styles: &mut [SemanticStyle],
) {
    let len = styles.len();
    for token in tokens {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }

        for style in styles.iter_mut().take(to).skip(from) {
            match token.kind {
                CodeTokenType::Keyword => {
                    style.fg = Some(SemanticRole::CodeKeyword);
                    style.dim = false;
                }
                CodeTokenType::String => {
                    style.fg = Some(SemanticRole::CodeString);
                    style.dim = false;
                }
                CodeTokenType::Number => {
                    style.fg = Some(SemanticRole::CodeNumber);
                    style.dim = false;
                }
                CodeTokenType::Comment => {
                    style.fg = Some(SemanticRole::CodeComment);
                    style.dim = true;
                }
                CodeTokenType::Function => {
                    style.fg = Some(SemanticRole::CodeFunction);
                    style.dim = false;
                }
                CodeTokenType::Type => {
                    style.fg = Some(SemanticRole::CodeType);
                    style.dim = false;
                }
            }
        }
    }
}

const INLINE_TOKEN_CACHE_MAX_ENTRIES: usize = 512;

struct InlineTokenCache {
    entries: FxHashMap<String, Arc<Vec<markdown_tokens::InlineToken>>>,
    order: VecDeque<String>,
}

impl InlineTokenCache {
    fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, text: &str) -> Arc<Vec<markdown_tokens::InlineToken>> {
        if let Some(tokens) = self.entries.get(text) {
            return Arc::clone(tokens);
        }
        let tokens = Arc::new(markdown_tokens::tokenize_inline_markdown(text));
        self.entries.insert(text.to_string(), Arc::clone(&tokens));
        self.order.push_back(text.to_string());
        while self.entries.len() > INLINE_TOKEN_CACHE_MAX_ENTRIES {
            let Some(evict_key) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(evict_key.as_str());
        }
        tokens
    }
}

thread_local! {
    static INLINE_TOKEN_CACHE: RefCell<InlineTokenCache> = RefCell::new(InlineTokenCache::new());
}

fn cached_inline_tokens(text: &str) -> Arc<Vec<markdown_tokens::InlineToken>> {
    INLINE_TOKEN_CACHE.with(|cache| cache.borrow_mut().get(text))
}

pub struct SemanticContext {
    pub fence: markdown_tokens::FenceState,
    pub render_as_plain_code: bool,
    pub forced_code_lang: Option<String>,
}

#[derive(Clone, Copy, Default)]
pub struct LineDecorations<'a> {
    pub calc_ghost: Option<&'a str>,
    pub reminder_ghost: Option<&'a str>,
    pub reminder_strikethrough: bool,
    pub search_ranges: &'a [(usize, usize)],
    pub current_search_ranges: &'a [(usize, usize)],
    pub variable_names: Option<&'a VariableNames>,
    pub dim_ranges: &'a [(usize, usize)],
    pub selection_ranges: &'a [(usize, usize)],
    pub accent_ranges: &'a [(usize, usize)],
    pub underline_ranges: &'a [(usize, usize)],
    /// Cursor column when this is the cursor line; reveals markers near it.
    pub active_cursor_col: Option<usize>,
}
#[derive(Clone)]
pub struct SemanticLine {
    pub chars: Vec<char>,
    pub styles: Vec<SemanticStyle>,
    pub hidden_ranges: Vec<(usize, usize)>,
    pub base_style: SemanticStyle,
    pub calc_prefix: &'static str,
    /// Raw scalar columns to visible columns after marker hiding. Cached with styling.
    pub source_map: Arc<super::mapping::SourceDisplayMap>,
}

impl SemanticLine {
    /// Build scalar source/display coordinates from explicit hidden spans.
    /// Callers that already transformed text compose this with their provenance map.
    pub fn source_mapping(&self) -> super::mapping::SourceDisplayMap {
        self.source_map.as_ref().clone()
    }
}

// Bounded by visible-line working sets. Hashes include every semantic input;
// entries retain full keys so hash collisions cannot return another line.
#[derive(Clone, PartialEq, Eq)]
struct StyleKey {
    text: String,
    fence: markdown_tokens::FenceState,
    plain: bool,
    language: Option<String>,
    variables: Option<u64>,
    cursor: Option<usize>,
    assignment_ghost: bool,
    ranges: [Vec<(usize, usize)>; 5],
}
struct StyleEntry {
    key: StyleKey,
    line: SemanticLine,
    fence_after: markdown_tokens::FenceState,
}
thread_local! {
    static STYLE_CACHE: RefCell<FxHashMap<u64, StyleEntry>> = RefCell::new(FxHashMap::default());
    static STYLE_ORDER: RefCell<VecDeque<u64>> = const { RefCell::new(VecDeque::new()) };
}
impl SemanticContext {
    pub fn style_line(&mut self, text: &str, deco: &LineDecorations<'_>) -> SemanticLine {
        use std::hash::{Hash, Hasher};
        let variables = deco.variable_names.map(VariableNames::identity);
        let assignment_ghost = deco
            .calc_ghost
            .is_some_and(|g| g.trim_start().starts_with('*'));
        let ranges = [
            deco.search_ranges,
            deco.current_search_ranges,
            deco.dim_ranges,
            deco.accent_ranges,
            deco.underline_ranges,
        ];
        let mut hasher = rustc_hash::FxHasher::default();
        text.hash(&mut hasher);
        self.fence.in_code_block.hash(&mut hasher);
        self.fence.code_fence_lang.hash(&mut hasher);
        self.fence.fence.map(|f| (f.ch, f.len)).hash(&mut hasher);
        self.render_as_plain_code.hash(&mut hasher);
        self.forced_code_lang.hash(&mut hasher);
        variables.hash(&mut hasher);
        deco.active_cursor_col.hash(&mut hasher);
        assignment_ghost.hash(&mut hasher);
        ranges.hash(&mut hasher);
        let hash = hasher.finish();
        let found = STYLE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let entry = cache.get(&hash)?;
            let key = &entry.key;
            (key.text == text
                && key.fence == self.fence
                && key.plain == self.render_as_plain_code
                && key.language == self.forced_code_lang
                && key.variables == variables
                && key.cursor == deco.active_cursor_col
                && key.assignment_ghost == assignment_ghost
                && key.ranges.iter().zip(ranges).all(|(a, b)| a == b))
            .then(|| (entry.line.clone(), entry.fence_after.clone()))
        });
        if let Some((line, fence)) = found {
            self.fence = fence;
            return line;
        }
        let key = StyleKey {
            text: text.to_string(),
            fence: self.fence.clone(),
            plain: self.render_as_plain_code,
            language: self.forced_code_lang.clone(),
            variables,
            cursor: deco.active_cursor_col,
            assignment_ghost,
            ranges: ranges.map(<[(usize, usize)]>::to_vec),
        };
        let line = self.style_line_uncached(text, deco);
        if line.chars.len() > 1024 {
            return line;
        }
        STYLE_CACHE.with(|cache| {
            STYLE_ORDER.with(|order| {
                let mut cache = cache.borrow_mut();
                let mut order = order.borrow_mut();
                if !cache.contains_key(&hash) {
                    order.push_back(hash);
                }
                cache.insert(
                    hash,
                    StyleEntry {
                        key,
                        line: line.clone(),
                        fence_after: self.fence.clone(),
                    },
                );
                while order.len() > 512 {
                    if let Some(old) = order.pop_front() {
                        cache.remove(&old);
                    }
                }
            });
        });
        line
    }
}

impl SemanticContext {
    fn style_line_uncached(&mut self, text: &str, deco: &LineDecorations<'_>) -> SemanticLine {
        let LineDecorations {
            calc_ghost,
            reminder_ghost: _,
            reminder_strikethrough: _,
            search_ranges,
            current_search_ranges,
            variable_names,
            dim_ranges,
            selection_ranges: _,
            accent_ranges: red_ranges,
            underline_ranges,
            active_cursor_col,
        } = *deco;
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let is_table_row = text.trim_start().starts_with('|');
        let is_table_continuation_line = editor_core::table::is_table_continuation_line(text);
        let info = if self.render_as_plain_code {
            None
        } else {
            Some(markdown_tokens::classify_markdown_line(text))
        };
        // A fence line here is one that opens a block or closes the open one.
        let is_fence_line = info.is_some() && markdown_tokens::is_fence_line(&self.fence, text);
        let is_code_block_line =
            !self.render_as_plain_code && (self.fence.in_code_block || is_fence_line);
        let base_style = SemanticStyle {
            fg: Some(SemanticRole::Text),
            bg: Some(if is_code_block_line {
                SemanticRole::CodeBlock
            } else {
                SemanticRole::Surface
            }),
            ..Default::default()
        };
        let mut styles = vec![base_style; len];
        let mut hidden_ranges: Vec<(usize, usize)> = Vec::new();
        if self.render_as_plain_code {
            if let Some(lang) = self.forced_code_lang.as_deref() {
                let code_tokens = markdown_tokens::tokenize_code_line(text, Some(lang));
                apply_code_token_styles(&code_tokens, &mut styles);
            }
            apply_variable_styles(&chars, &mut styles, variable_names, SemanticRole::Variable);
        } else if is_table_continuation_line {
            // `|>` is a structural continuation marker, not editable cell content.
            if let Some(style) = styles.get_mut(1) {
                style.dim = true;
                style.italic = true;
                style.fg = Some(SemanticRole::CodeComment);
            }
            if text.chars().nth(2).is_some_and(|ch| ch == ' ') {
                if let Some(style) = styles.get_mut(2) {
                    style.dim = true;
                    style.italic = true;
                    style.fg = Some(SemanticRole::CodeComment);
                }
            }
        }

        if self.render_as_plain_code {
            // Skip markdown semantic styling when a file has a fixed syntax mode.
        } else if is_fence_line {
            for s in &mut styles {
                s.dim = true;
            }
            markdown_tokens::advance_fence_state(&mut self.fence, text);
        } else if self.fence.in_code_block {
            let code_tokens =
                markdown_tokens::tokenize_code_line(text, self.fence.code_fence_lang.as_deref());
            apply_code_token_styles(&code_tokens, &mut styles);
        } else {
            let info = info.as_ref().expect("markdown info present");
            apply_line_styles_from_info(info, &mut styles);
            hidden_ranges.extend(hidden_line_prefix_marker_ranges(
                info,
                len,
                active_cursor_col.is_some(),
            ));
            let inline_tokens = cached_inline_tokens(text);
            // Inside table rows, single-asterisk Emphasis (`*x*`) is ambiguous
            // with formula markers (`value*`, `value***`) and can leak across
            // cell boundaries — always filter it.
            //
            // Strong (bold, `**x**`) uses double asterisks that wrap content;
            // this is visually unambiguous when the token stays within one cell.
            // Allow it unless the token spans a `|` character (cross-cell).
            let filtered_tokens: Vec<markdown_tokens::InlineToken>;
            let inline_tokens_to_apply: &[markdown_tokens::InlineToken] = if is_table_row {
                filtered_tokens = inline_tokens
                    .iter()
                    .filter(|t| {
                        if matches!(t.kind, markdown_tokens::InlineTokenType::Emphasis) {
                            return false;
                        }
                        if matches!(t.kind, markdown_tokens::InlineTokenType::Strong) {
                            // Drop bold that spans a pipe (cross-cell ambiguity).
                            // Scan the chars slice directly — no Vec allocation.
                            let from = t.from.min(len);
                            let to = t.to.min(len);
                            return !chars[from..to].iter().any(|&c| c == '|');
                        }
                        true
                    })
                    .cloned()
                    .collect();
                &filtered_tokens
            } else {
                inline_tokens.as_ref()
            };
            apply_inline_token_styles(
                inline_tokens_to_apply,
                &mut styles,
                &mut hidden_ranges,
                active_cursor_col,
            );
            apply_variable_styles(&chars, &mut styles, variable_names, SemanticRole::Variable);
        }

        for &(start, end) in dim_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.dim = true;
                s.fg = Some(SemanticRole::CodeComment);
            }
        }
        if !self.render_as_plain_code && is_table_row && text.contains('*') {
            apply_table_formula_marker_styles(&chars, &mut styles, SemanticRole::CalcResult);
        }

        for &(start, end) in search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.fg = Some(SemanticRole::SearchMatch);
            }
        }

        for &(start, end) in current_search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.bold = true;
                s.fg = Some(SemanticRole::SearchCurrent);
            }
        }

        for &(start, end) in red_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.fg = Some(SemanticRole::Primary);
                s.bold = true;
                s.dim = false;
            }
        }

        for &(start, end) in underline_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.underline = true;
            }
        }

        hidden_ranges = normalize_hidden_ranges(hidden_ranges, len);
        for &(start, end) in &hidden_ranges {
            for style in &mut styles[start..end] {
                style.fg = Some(SemanticRole::HiddenMarker);
            }
        }
        let calc_prefix = if calc_ghost
            .map(|ghost| ghost.trim_start().starts_with('*'))
            .unwrap_or(false)
        {
            " "
        } else if contains_assignment_operator(text) {
            " = "
        } else {
            " → "
        };

        let source_map = Arc::new(
            super::mapping::SourceDisplayMap::identity(text)
                .hide(
                    text,
                    &hidden_ranges
                        .iter()
                        .map(|&(from, to)| from..to)
                        .collect::<Vec<_>>(),
                )
                .map,
        );
        SemanticLine {
            source_map,
            chars,
            styles,
            hidden_ranges,
            base_style,
            calc_prefix,
        }
    }
}
fn contains_assignment_operator(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }

    for i in 0..bytes.len() - 1 {
        if bytes[i] != b':' || bytes[i + 1] != b'=' {
            continue;
        }

        if i > 0 && matches!(bytes[i - 1], b':' | b'!' | b'<' | b'>' | b'=') {
            continue;
        }
        if i + 2 < bytes.len() && bytes[i + 2] == b'=' {
            continue;
        }

        return true;
    }

    false
}

fn apply_table_formula_marker_styles(
    chars: &[char],
    styles: &mut [SemanticStyle],
    marker_color: SemanticRole,
) {
    let len = chars.len();
    let mut i = 0usize;
    while i < len {
        if chars[i] != '*' {
            i += 1;
            continue;
        }
        let start = i;
        while i < len && chars[i] == '*' {
            i += 1;
        }
        let end = i;
        let prev = if start > 0 {
            Some(chars[start - 1])
        } else {
            None
        };
        let next = if end < len { Some(chars[end]) } else { None };
        let prev_ok = prev
            .map(|ch| ch.is_ascii_digit() || ch == '.' || ch == ')')
            .unwrap_or(false);
        let next_ok = next
            .map(|ch| ch == '|' || ch.is_whitespace())
            .unwrap_or(true);
        if prev_ok && next_ok {
            for s in styles.iter_mut().take(end).skip(start) {
                s.dim = true;
                s.fg = Some(marker_color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> SemanticContext {
        SemanticContext {
            fence: Default::default(),
            render_as_plain_code: false,
            forced_code_lang: None,
        }
    }
    #[test]
    fn cache_tracks_changed_variable_identity_and_cursor_reveal() {
        let mut ctx = context();
        let mut names = VariableNames::new(vec!["first".into()]);
        let before = ctx.style_line(
            "first second",
            &LineDecorations {
                variable_names: Some(&names),
                ..Default::default()
            },
        );
        assert_eq!(before.styles[0].fg, Some(SemanticRole::Variable));
        names.set(vec!["second".into()]);
        let after = ctx.style_line(
            "first second",
            &LineDecorations {
                variable_names: Some(&names),
                ..Default::default()
            },
        );
        assert_eq!(after.styles[0].fg, Some(SemanticRole::Text));
        assert_eq!(after.styles[6].fg, Some(SemanticRole::Variable));
        let hidden = ctx.style_line("**é**", &LineDecorations::default());
        assert!(!hidden.hidden_ranges.is_empty());
        assert_eq!(hidden.styles[0].fg, Some(SemanticRole::HiddenMarker));
        assert_eq!(hidden.source_mapping().display_len, 1);
        let shown = ctx.style_line(
            "**é**",
            &LineDecorations {
                active_cursor_col: Some(2),
                ..Default::default()
            },
        );
        assert!(shown.hidden_ranges.is_empty());
    }
    #[test]
    fn cache_hits_restore_fence_transitions_and_syntax_modes() {
        // The second context hits cached opening/closing lines but must advance
        // its own fence just as the initial uncached walk did.
        for _ in 0..2 {
            let mut ctx = context();
            ctx.style_line("```rust", &Default::default());
            assert!(ctx.fence.in_code_block);
            let code = ctx.style_line("let x = 1;", &Default::default());
            assert_eq!(code.styles[0].fg, Some(SemanticRole::CodeKeyword));
            ctx.style_line("```", &Default::default());
            assert!(!ctx.fence.in_code_block);
            let heading = ctx.style_line("# heading", &Default::default());
            assert_eq!(heading.styles[2].fg, Some(SemanticRole::Heading));
            ctx.render_as_plain_code = true;
            let plain = ctx.style_line("# heading", &Default::default());
            assert!(plain.hidden_ranges.is_empty());
            assert_eq!(plain.styles[2].fg, Some(SemanticRole::Text));
        }
    }
    #[test]
    fn cache_tracks_search_calc_ranges_and_ghost_prefix() {
        let mut ctx = context();
        let text = "a + b";
        let first = ctx.style_line(
            text,
            &LineDecorations {
                search_ranges: &[(0, 1)],
                dim_ranges: &[(4, 5)],
                ..Default::default()
            },
        );
        assert_eq!(first.styles[0].fg, Some(SemanticRole::SearchMatch));
        assert!(first.styles[4].dim);
        let second = ctx.style_line(
            text,
            &LineDecorations {
                search_ranges: &[(4, 5)],
                dim_ranges: &[(0, 1)],
                calc_ghost: Some("*42"),
                ..Default::default()
            },
        );
        assert_eq!(second.styles[4].fg, Some(SemanticRole::SearchMatch));
        assert!(!second.styles[4].dim);
        assert!(second.styles[0].dim);
        assert_eq!(first.calc_prefix, " → ");
        assert_eq!(second.calc_prefix, " ");
        let formula = ctx.style_line("| 42* |", &Default::default());
        assert_eq!(formula.styles[4].fg, Some(SemanticRole::CalcResult));
    }
}
