use serde::{Deserialize, Serialize};
use rustc_hash::FxHashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownLineInfo {
    pub heading_level: Option<usize>,
    pub heading_marker_end: Option<usize>,
    pub quote_marker_end: Option<usize>,
    pub list_marker_end: Option<usize>,
    pub checklist_marker_start: Option<usize>,
    pub checklist_marker_end: Option<usize>,
    pub checklist_content_start: Option<usize>,
    pub checklist_checked: bool,
    pub is_horizontal_rule: bool,
    pub is_code_fence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InlineTokenType {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    CodeMarker,
    ImageAlt,
    ImageSrc,
    ImageMarker,
    LinkText,
    LinkUrl,
    LinkMarker,
    WikiLinkMarker,
    WikiLinkId,
    WikiLinkSep,
    WikiLinkTitle,
    WikiLinkAnchor,
}

impl InlineTokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            InlineTokenType::Strong => "strong",
            InlineTokenType::Emphasis => "emphasis",
            InlineTokenType::Strikethrough => "strikethrough",
            InlineTokenType::Code => "code",
            InlineTokenType::CodeMarker => "code-marker",
            InlineTokenType::ImageAlt => "image-alt",
            InlineTokenType::ImageSrc => "image-src",
            InlineTokenType::ImageMarker => "image-marker",
            InlineTokenType::LinkText => "link-text",
            InlineTokenType::LinkUrl => "link-url",
            InlineTokenType::LinkMarker => "link-marker",
            InlineTokenType::WikiLinkMarker => "wiki-link-marker",
            InlineTokenType::WikiLinkId => "wiki-link-id",
            InlineTokenType::WikiLinkSep => "wiki-link-sep",
            InlineTokenType::WikiLinkTitle => "wiki-link-title",
            InlineTokenType::WikiLinkAnchor => "wiki-link-anchor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineToken {
    pub from: usize,
    pub to: usize,
    #[serde(rename = "type")]
    pub kind: InlineTokenType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineMarkerComponentRange {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiLinkMatch {
    pub from: usize,
    pub to: usize,
    pub short_id: String,
    pub heading: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownImageMatch {
    pub from: usize,
    pub to: usize,
    pub alt: String,
    pub src: String,
    pub width: Option<usize>,
    pub height: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeTokenType {
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
}

impl CodeTokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeTokenType::Keyword => "keyword",
            CodeTokenType::String => "string",
            CodeTokenType::Number => "number",
            CodeTokenType::Comment => "comment",
            CodeTokenType::Function => "function",
            CodeTokenType::Type => "type",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeToken {
    pub from: usize,
    pub to: usize,
    #[serde(rename = "type")]
    pub kind: CodeTokenType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FenceState {
    pub in_code_block: bool,
    pub code_fence_lang: Option<String>,
}

impl Default for FenceState {
    fn default() -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownAnalyzedLine {
    pub info: MarkdownLineInfo,
    pub in_code_block: bool,
    pub code_fence_lang: Option<String>,
    pub inline_tokens: Vec<InlineToken>,
    pub code_tokens: Vec<CodeToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownAnalyzeResult {
    pub lines: Vec<MarkdownAnalyzedLine>,
    pub final_in_code_block: bool,
    pub final_code_fence_lang: Option<String>,
}

#[derive(Clone, Copy)]
enum CommentMode {
    None,
    Hash,
    Slash,
}

fn count_chars(text: &str, upto_byte: usize) -> usize {
    text[..upto_byte.min(text.len())].chars().count()
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn consume_ascii_whitespace(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn is_digits_bytes(segment: &[u8]) -> bool {
    !segment.is_empty() && segment.iter().all(|b| b.is_ascii_digit())
}

fn looks_like_month_name(content: &str) -> bool {
    let first = content
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|ch: char| !ch.is_ascii_alphabetic())
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "jan"
            | "january"
            | "feb"
            | "february"
            | "mar"
            | "march"
            | "apr"
            | "april"
            | "may"
            | "jun"
            | "june"
            | "jul"
            | "july"
            | "aug"
            | "august"
            | "sep"
            | "sept"
            | "september"
            | "oct"
            | "october"
            | "nov"
            | "november"
            | "dec"
            | "december"
    )
}

fn is_probable_numeric_date_marker(marker: &str) -> bool {
    let base = marker.strip_suffix('.').unwrap_or(marker);
    let mut iter = base.split('.');
    let (Some(day_s), Some(month_s), Some(year_s), None) =
        (iter.next(), iter.next(), iter.next(), iter.next())
    else {
        return false;
    };
    if !(day_s.as_bytes().iter().all(|b| b.is_ascii_digit())
        && month_s.as_bytes().iter().all(|b| b.is_ascii_digit())
        && year_s.as_bytes().iter().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let day = day_s.parse::<u32>().ok().unwrap_or(0);
    let month = month_s.parse::<u32>().ok().unwrap_or(0);
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return false;
    }
    marker.ends_with('.') || year_s.len() >= 4
}

fn heading_marker_end(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = consume_ascii_whitespace(bytes, 0);
    let mut level = 0;
    while i < bytes.len() && level < 6 && bytes[i] == b'#' {
        level += 1;
        i += 1;
    }
    if level == 0 || i >= bytes.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    i = consume_ascii_whitespace(bytes, i);
    Some((level, count_chars(text, i)))
}

fn quote_marker_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = consume_ascii_whitespace(bytes, 0);
    let mut gt = 0;
    while i < bytes.len() && bytes[i] == b'>' {
        gt += 1;
        i += 1;
    }
    if gt == 0 {
        return None;
    }
    i = consume_ascii_whitespace(bytes, i);
    Some(count_chars(text, i))
}

pub fn list_marker_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let i = consume_ascii_whitespace(bytes, 0);
    if i >= len {
        return None;
    }

    let marker_end = if i + 2 <= len && &bytes[i..i + 2] == b"->" {
        let after = i + 2;
        if after >= len || !bytes[after].is_ascii_whitespace() {
            return None;
        }
        consume_ascii_whitespace(bytes, after)
    } else if matches!(bytes[i], b'-' | b'*' | b'+') {
        let after = i + 1;
        if after >= len || !bytes[after].is_ascii_whitespace() {
            return None;
        }
        consume_ascii_whitespace(bytes, after)
    } else {
        let mut j = i;
        while j < len && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == i || j >= len || bytes[j] != b'.' {
            return None;
        }

        j += 1;
        if j < len && bytes[j].is_ascii_digit() {
            while j < len {
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < len && bytes[j] == b'.' && j + 1 < len && bytes[j + 1].is_ascii_digit() {
                    j += 1;
                    continue;
                }
                break;
            }
        }

        if j >= len || !bytes[j].is_ascii_whitespace() {
            return None;
        }
        let marker = &text[i..j];
        let rest = &text[j..];
        if is_probable_numeric_date_marker(marker) {
            return None;
        }
        if marker
            .strip_suffix('.')
            .is_some_and(|base| is_digits_bytes(base.as_bytes()))
            && looks_like_month_name(rest)
        {
            return None;
        }
        consume_ascii_whitespace(bytes, j)
    };

    Some(count_chars(text, marker_end))
}

fn checklist_marker_details(text: &str) -> Option<(usize, usize, usize, bool)> {
    let list_end = list_marker_end(text)?;
    let list_end_byte = char_to_byte_idx(text, list_end);
    let bytes = text.as_bytes();
    if list_end_byte + 3 > bytes.len() {
        return None;
    }
    if bytes[list_end_byte] != b'[' || bytes[list_end_byte + 2] != b']' {
        return None;
    }

    let mark = bytes[list_end_byte + 1];
    if !matches!(mark, b' ' | b'x' | b'X') {
        return None;
    }

    let mut content_byte = list_end_byte + 3;
    if content_byte >= bytes.len() || !bytes[content_byte].is_ascii_whitespace() {
        return None;
    }
    content_byte = consume_ascii_whitespace(bytes, content_byte);

    Some((
        list_end,
        count_chars(text, list_end_byte + 3),
        count_chars(text, content_byte),
        matches!(mark, b'x' | b'X'),
    ))
}

pub fn checklist_marker_end(text: &str) -> Option<(usize, bool)> {
    let (_start, end, _content, checked) = checklist_marker_details(text)?;
    Some((end, checked))
}

pub fn is_horizontal_rule(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }

    let mut chars = trimmed.chars().filter(|ch| !ch.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }

    let mut count = 1usize;
    for ch in chars {
        if ch != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

pub fn is_code_fence(text: &str) -> bool {
    text.trim_start().starts_with("```")
}

pub fn normalize_fence_lang(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    let normalized = match value.as_str() {
        "typescript" | "tsx" => "ts",
        "javascript" | "jsx" => "js",
        "shell" | "bash" | "zsh" => "sh",
        "py" => "python",
        "rs" => "rust",
        other => other,
    };
    Some(normalized.to_string())
}

pub fn parse_fence_language(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with("```") {
        return None;
    }
    let rest = trimmed[3..].trim_start();
    if rest.is_empty() {
        return None;
    }
    let lang: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '-'))
        .collect();
    if lang.is_empty() {
        return None;
    }
    normalize_fence_lang(&lang)
}

pub fn advance_fence_state(state: &mut FenceState, text: &str) {
    if !is_code_fence(text) {
        return;
    }

    if state.in_code_block {
        state.in_code_block = false;
        state.code_fence_lang = None;
    } else {
        state.in_code_block = true;
        state.code_fence_lang = parse_fence_language(text);
    }
}

pub fn classify_markdown_line(text: &str) -> MarkdownLineInfo {
    let heading = heading_marker_end(text);
    let quote = quote_marker_end(text);
    let list_end = list_marker_end(text);
    let checklist = checklist_marker_details(text);

    MarkdownLineInfo {
        heading_level: heading.map(|(level, _)| level),
        heading_marker_end: heading.map(|(_, end)| end),
        quote_marker_end: quote,
        list_marker_end: list_end,
        checklist_marker_start: checklist.map(|(start, _, _, _)| start),
        checklist_marker_end: checklist.map(|(_, end, _, _)| end),
        checklist_content_start: checklist.map(|(_, _, content, _)| content),
        checklist_checked: checklist.map(|(_, _, _, checked)| checked).unwrap_or(false),
        is_horizontal_rule: is_horizontal_rule(text),
        is_code_fence: is_code_fence(text),
    }
}

pub fn extract_markdown_headings(text: &str) -> Vec<String> {
    let mut headings = Vec::new();
    let mut seen = FxHashSet::default();

    for line in text.lines() {
        let trimmed = line.trim_start();
        let hash_count = trimmed.chars().take_while(|ch| *ch == '#').count();
        if hash_count == 0 || hash_count > 6 {
            continue;
        }
        let rest = &trimmed[hash_count..];
        if !rest.chars().next().is_some_and(|ch| ch.is_whitespace()) {
            continue;
        }

        let mut heading = rest.trim();
        heading = heading.trim_end_matches('#').trim_end();
        if heading.is_empty() {
            continue;
        }
        let normalized = heading.to_lowercase();
        if seen.insert(normalized) {
            headings.push(heading.to_string());
        }
    }

    headings
}

fn overlaps(ranges: &[(usize, usize)], from: usize, to: usize) -> bool {
    ranges.iter().any(|(a, b)| from < *b && to > *a)
}

fn protect(ranges: &mut Vec<(usize, usize)>, from: usize, to: usize) {
    if to > from {
        ranges.push((from, to));
    }
}

fn push_inline_token(tokens: &mut Vec<InlineToken>, from: usize, to: usize, kind: InlineTokenType) {
    if to > from {
        tokens.push(InlineToken { from, to, kind });
    }
}

fn is_run(chars: &[char], pos: usize, marker: char, count: usize) -> bool {
    (0..count).all(|idx| pos + idx < chars.len() && chars[pos + idx] == marker)
}

pub fn is_inline_marker_token_kind(kind: InlineTokenType) -> bool {
    matches!(
        kind,
        InlineTokenType::CodeMarker
            | InlineTokenType::ImageMarker
            | InlineTokenType::LinkMarker
            | InlineTokenType::WikiLinkMarker
    )
}

fn is_marker_left_boundary(chars: &[char], marker_start: usize) -> bool {
    marker_start == 0 || chars[marker_start - 1].is_whitespace()
}

fn is_marker_right_boundary(chars: &[char], marker_end: usize) -> bool {
    marker_end >= chars.len() || chars[marker_end].is_whitespace()
}

fn find_backtick_close(chars: &[char], from: usize, count: usize) -> Option<usize> {
    let mut i = from;
    while i + count <= chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let start = i;
        let mut run = 0;
        while i < chars.len() && chars[i] == '`' {
            run += 1;
            i += 1;
        }
        if run == count {
            return Some(start);
        }
    }
    None
}

fn find_paired_close(
    chars: &[char],
    protected: &[(usize, usize)],
    from: usize,
    marker: char,
    marker_len: usize,
) -> Option<usize> {
    let mut i = from;
    while i + marker_len <= chars.len() {
        if is_run(chars, i, marker, marker_len)
            && !overlaps(protected, i, i + marker_len)
            && i > 0
            && !chars[i - 1].is_whitespace()
            && is_marker_right_boundary(chars, i + marker_len)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_single_close(
    chars: &[char],
    protected: &[(usize, usize)],
    from: usize,
    marker: char,
) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == marker && !overlaps(protected, i, i + 1) {
            if i + 1 < chars.len() && chars[i + 1] == marker {
                i += 2;
                continue;
            }
            if i > 0 && !chars[i - 1].is_whitespace() && is_marker_right_boundary(chars, i + 1) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

pub fn tokenize_inline_markdown(text: &str) -> Vec<InlineToken> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut tokens = Vec::new();
    let mut protected: Vec<(usize, usize)> = Vec::new();

    // `code`
    let mut i = 0;
    while i < len {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let open = i;
        let mut tick_count = 0;
        while i < len && chars[i] == '`' {
            tick_count += 1;
            i += 1;
        }
        if let Some(close) = find_backtick_close(&chars, i, tick_count) {
            push_inline_token(
                &mut tokens,
                open,
                open + tick_count,
                InlineTokenType::CodeMarker,
            );
            push_inline_token(&mut tokens, open + tick_count, close, InlineTokenType::Code);
            push_inline_token(
                &mut tokens,
                close,
                close + tick_count,
                InlineTokenType::CodeMarker,
            );
            protect(&mut protected, open, close + tick_count);
            i = close + tick_count;
        }
    }

    // [[shortid]] | [[shortid#heading]] | [[shortid|alt-text]] | [[shortid#heading|alt-text]]
    // short ID must be exactly 8 alphanumeric chars (ULID prefix); inserted only via autocomplete picker.
    // The #heading and |alt-text parts are both optional.
    i = 0;
    while i + 5 < len {
        if chars[i] != '[' || chars[i + 1] != '[' || overlaps(&protected, i, i + 2) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 2;
        while j + 1 < len && chars[j] != '\n' && !(chars[j] == ']' && chars[j + 1] == ']') {
            j += 1;
        }
        if j + 1 >= len || chars[j] != ']' || chars[j + 1] != ']' {
            i += 1;
            continue;
        }
        let end = j + 2;
        let inner = &chars[i + 2..j];

        // Split on optional '|' to separate id_part from alt-text.
        let pipe_rel = inner.iter().position(|&c| c == '|');
        let id_part = if let Some(p) = pipe_rel {
            &inner[..p]
        } else {
            &inner[..]
        };

        // Split id_part on optional '#' to separate short_id from heading anchor.
        let (short_id_chars, has_anchor) =
            if let Some(hash_rel) = id_part.iter().position(|&c| c == '#') {
                (&id_part[..hash_rel], true)
            } else {
                (id_part, false)
            };

        if short_id_chars.len() != 8 || !short_id_chars.iter().all(|c| c.is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }

        let id_end = i + 2 + short_id_chars.len();
        let anchor_end = i + 2 + id_part.len();

        push_inline_token(
            &mut tokens,
            start,
            start + 2,
            InlineTokenType::WikiLinkMarker,
        );
        push_inline_token(&mut tokens, start + 2, id_end, InlineTokenType::WikiLinkId);
        if has_anchor {
            push_inline_token(
                &mut tokens,
                id_end,
                anchor_end,
                InlineTokenType::WikiLinkAnchor,
            );
        }
        if let Some(pipe_rel) = pipe_rel {
            let pipe_abs = i + 2 + pipe_rel;
            let title_start = pipe_abs + 1;
            push_inline_token(
                &mut tokens,
                pipe_abs,
                title_start,
                InlineTokenType::WikiLinkSep,
            );
            push_inline_token(&mut tokens, title_start, j, InlineTokenType::WikiLinkTitle);
        }
        push_inline_token(&mut tokens, j, end, InlineTokenType::WikiLinkMarker);
        protect(&mut protected, start, end);
        i = end;
    }

    // ![alt](src)
    i = 0;
    while i + 4 < len {
        if chars[i] != '!' || chars[i + 1] != '[' {
            i += 1;
            continue;
        }
        let start = i;
        let mut close_bracket = i + 2;
        while close_bracket < len && !matches!(chars[close_bracket], ']' | '\n') {
            close_bracket += 1;
        }
        if close_bracket >= len
            || chars[close_bracket] != ']'
            || close_bracket + 1 >= len
            || chars[close_bracket + 1] != '('
        {
            i += 1;
            continue;
        }

        let mut close_paren = close_bracket + 2;
        while close_paren < len && !matches!(chars[close_paren], ')' | '\n') {
            close_paren += 1;
        }
        if close_paren >= len || chars[close_paren] != ')' || close_paren == close_bracket + 2 {
            i += 1;
            continue;
        }

        let end = close_paren + 1;
        if overlaps(&protected, start, end) {
            i += 1;
            continue;
        }

        push_inline_token(&mut tokens, start, start + 2, InlineTokenType::ImageMarker);
        push_inline_token(
            &mut tokens,
            start + 2,
            close_bracket,
            InlineTokenType::ImageAlt,
        );
        push_inline_token(
            &mut tokens,
            close_bracket,
            close_bracket + 2,
            InlineTokenType::ImageMarker,
        );
        push_inline_token(
            &mut tokens,
            close_bracket + 2,
            close_paren,
            InlineTokenType::ImageSrc,
        );
        push_inline_token(
            &mut tokens,
            close_paren,
            close_paren + 1,
            InlineTokenType::ImageMarker,
        );
        protect(&mut protected, start, end);
        i = end;
    }

    // [text](url)
    i = 0;
    while i < len {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        let start = i;
        let mut close_bracket = i + 1;
        while close_bracket < len && !matches!(chars[close_bracket], ']' | '\n') {
            close_bracket += 1;
        }
        if close_bracket >= len
            || chars[close_bracket] != ']'
            || close_bracket == start + 1
            || close_bracket + 1 >= len
            || chars[close_bracket + 1] != '('
        {
            i += 1;
            continue;
        }

        let mut close_paren = close_bracket + 2;
        while close_paren < len && !matches!(chars[close_paren], ')' | '\n') {
            close_paren += 1;
        }
        if close_paren >= len || chars[close_paren] != ')' || close_paren == close_bracket + 2 {
            i += 1;
            continue;
        }

        let end = close_paren + 1;
        if overlaps(&protected, start, end) {
            i += 1;
            continue;
        }

        push_inline_token(&mut tokens, start, start + 1, InlineTokenType::LinkMarker);
        push_inline_token(
            &mut tokens,
            start + 1,
            close_bracket,
            InlineTokenType::LinkText,
        );
        push_inline_token(
            &mut tokens,
            close_bracket,
            close_bracket + 2,
            InlineTokenType::LinkMarker,
        );
        push_inline_token(
            &mut tokens,
            close_bracket + 2,
            close_paren,
            InlineTokenType::LinkUrl,
        );
        push_inline_token(
            &mut tokens,
            close_paren,
            close_paren + 1,
            InlineTokenType::LinkMarker,
        );
        protect(&mut protected, start, end);
        i = end;
    }

    // **strong** and __strong__
    for marker in ['*', '_'] {
        let mut pos = 0;
        while pos + 4 <= len {
            if !is_run(&chars, pos, marker, 2) || overlaps(&protected, pos, pos + 2) {
                pos += 1;
                continue;
            }
            if !is_marker_left_boundary(&chars, pos) {
                pos += 1;
                continue;
            }
            let after = pos + 2;
            if after >= len || chars[after].is_whitespace() {
                pos += 1;
                continue;
            }
            let Some(close) = find_paired_close(&chars, &protected, after, marker, 2) else {
                pos += 1;
                continue;
            };

            push_inline_token(&mut tokens, pos, pos + 2, InlineTokenType::CodeMarker);
            push_inline_token(&mut tokens, after, close, InlineTokenType::Strong);
            push_inline_token(&mut tokens, close, close + 2, InlineTokenType::CodeMarker);
            protect(&mut protected, pos, close + 2);
            pos = close + 2;
        }
    }

    // ~~strike~~
    i = 0;
    while i + 4 <= len {
        if !is_run(&chars, i, '~', 2) || overlaps(&protected, i, i + 2) {
            i += 1;
            continue;
        }
        if !is_marker_left_boundary(&chars, i) {
            i += 1;
            continue;
        }
        let after = i + 2;
        if after >= len || chars[after].is_whitespace() {
            i += 1;
            continue;
        }
        let Some(close) = find_paired_close(&chars, &protected, after, '~', 2) else {
            i += 1;
            continue;
        };

        push_inline_token(&mut tokens, i, i + 2, InlineTokenType::CodeMarker);
        push_inline_token(&mut tokens, after, close, InlineTokenType::Strikethrough);
        push_inline_token(&mut tokens, close, close + 2, InlineTokenType::CodeMarker);
        protect(&mut protected, i, close + 2);
        i = close + 2;
    }

    // *em* and _em_
    for marker in ['*', '_'] {
        let mut pos = 0;
        while pos + 2 < len {
            if chars[pos] != marker || overlaps(&protected, pos, pos + 1) {
                pos += 1;
                continue;
            }
            if !is_marker_left_boundary(&chars, pos) {
                pos += 1;
                continue;
            }
            if pos + 1 < len && chars[pos + 1] == marker {
                pos += 2;
                continue;
            }
            if chars[pos + 1].is_whitespace() {
                pos += 1;
                continue;
            }
            let Some(close) = find_single_close(&chars, &protected, pos + 1, marker) else {
                pos += 1;
                continue;
            };
            push_inline_token(&mut tokens, pos, pos + 1, InlineTokenType::CodeMarker);
            push_inline_token(&mut tokens, pos + 1, close, InlineTokenType::Emphasis);
            push_inline_token(&mut tokens, close, close + 1, InlineTokenType::CodeMarker);
            pos = close + 1;
        }
    }

    tokens.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    tokens
}

pub fn wiki_link_matches_from_tokens(text: &str, tokens: &[InlineToken]) -> Vec<WikiLinkMatch> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut open_idx: Option<usize> = None;
    let mut short_id: Option<String> = None;
    let mut heading: Option<String> = None;
    let mut title: Option<String> = None;

    for (idx, token) in tokens.iter().enumerate() {
        match token.kind {
            InlineTokenType::WikiLinkMarker => {
                if open_idx.is_none() {
                    open_idx = Some(idx);
                    short_id = None;
                    heading = None;
                    title = None;
                    continue;
                }
                let Some(start_token_idx) = open_idx.take() else {
                    continue;
                };
                let start_token = &tokens[start_token_idx];
                let Some(id) = short_id.take() else {
                    continue;
                };
                out.push(WikiLinkMatch {
                    from: start_token.from,
                    to: token.to,
                    short_id: id,
                    heading: heading.take(),
                    title: title.take(),
                });
            }
            InlineTokenType::WikiLinkId => {
                short_id = Some(chars[token.from..token.to].iter().collect());
            }
            InlineTokenType::WikiLinkAnchor => {
                let anchor: String = chars[token.from..token.to].iter().collect();
                heading = anchor.strip_prefix('#').map(|value| value.to_string());
            }
            InlineTokenType::WikiLinkTitle => {
                title = Some(chars[token.from..token.to].iter().collect());
            }
            _ => {}
        }
    }

    out
}

pub fn find_wiki_link_matches(text: &str) -> Vec<WikiLinkMatch> {
    let tokens = tokenize_inline_markdown(text);
    wiki_link_matches_from_tokens(text, &tokens)
}

pub fn wiki_link_at_cursor(text: &str, cursor_col: usize) -> Option<WikiLinkMatch> {
    let links = find_wiki_link_matches(text);
    links
        .into_iter()
        .find(|entry| cursor_col >= entry.from && cursor_col <= entry.to)
}

pub fn find_markdown_image_matches(text: &str) -> Vec<MarkdownImageMatch> {
    let tokens = tokenize_inline_markdown(text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut open_idx: Option<usize> = None;
    let mut marker_count = 0usize;
    let mut alt = String::new();
    let mut src = String::new();

    for (idx, token) in tokens.iter().enumerate() {
        match token.kind {
            InlineTokenType::ImageMarker => {
                if open_idx.is_none() {
                    open_idx = Some(idx);
                    marker_count = 1;
                    alt.clear();
                    src.clear();
                    continue;
                }
                marker_count += 1;
                if marker_count >= 3 {
                    let Some(start_idx) = open_idx.take() else {
                        continue;
                    };
                    let start = tokens[start_idx].from;
                    let (end, width, height) = parse_image_attribute_suffix(&chars, token.to);
                    if !src.is_empty() {
                        out.push(MarkdownImageMatch {
                            from: start,
                            to: end,
                            alt: alt.clone(),
                            src: src.clone(),
                            width,
                            height,
                        });
                    }
                    marker_count = 0;
                }
            }
            InlineTokenType::ImageAlt => {
                if open_idx.is_some() {
                    alt = chars[token.from..token.to].iter().collect();
                }
            }
            InlineTokenType::ImageSrc => {
                if open_idx.is_some() {
                    src = chars[token.from..token.to].iter().collect();
                }
            }
            _ => {}
        }
    }

    out
}

fn parse_image_attribute_suffix(
    chars: &[char],
    image_end: usize,
) -> (usize, Option<usize>, Option<usize>) {
    let mut i = image_end;
    while i < chars.len() && chars[i].is_whitespace() {
        if chars[i] == '\n' || chars[i] == '\r' {
            return (image_end, None, None);
        }
        i += 1;
    }
    if i >= chars.len() || chars[i] != '{' {
        return (image_end, None, None);
    }
    let attr_start = i;
    i += 1;
    while i < chars.len() && chars[i] != '}' && chars[i] != '\n' && chars[i] != '\r' {
        i += 1;
    }
    if i >= chars.len() || chars[i] != '}' {
        return (image_end, None, None);
    }
    let attr_end = i + 1;
    let body: String = chars[attr_start + 1..i].iter().collect();
    let (width, height) = parse_image_dimensions(&body);
    (attr_end, width, height)
}

fn parse_image_dimensions(attrs: &str) -> (Option<usize>, Option<usize>) {
    let mut width = None;
    let mut height = None;
    for part in attrs.split_whitespace() {
        let (key, value) = if let Some((k, v)) = part.split_once('=') {
            (k.trim().to_ascii_lowercase(), v.trim())
        } else if let Some((k, v)) = part.split_once(':') {
            (k.trim().to_ascii_lowercase(), v.trim())
        } else {
            continue;
        };
        let parsed = value
            .trim_matches('"')
            .trim_matches('\'')
            .trim_end_matches("px")
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0);
        match key.as_str() {
            "width" | "w" => {
                if parsed.is_some() {
                    width = parsed;
                }
            }
            "height" | "h" => {
                if parsed.is_some() {
                    height = parsed;
                }
            }
            _ => {}
        }
    }
    (width, height)
}

fn marker_component_range_for_token_index(
    tokens: &[InlineToken],
    marker_index: usize,
) -> Option<InlineMarkerComponentRange> {
    let marker = tokens.get(marker_index)?;
    if !is_inline_marker_token_kind(marker.kind) {
        return None;
    }

    let mut start_index = marker_index;
    while start_index > 0 {
        let prev = &tokens[start_index - 1];
        let current = &tokens[start_index];
        if prev.to != current.from {
            break;
        }
        start_index -= 1;
    }

    let mut end_index = marker_index;
    while end_index + 1 < tokens.len() {
        let current = &tokens[end_index];
        let next = &tokens[end_index + 1];
        if current.to != next.from {
            break;
        }
        end_index += 1;
    }

    let has_non_marker = (start_index..=end_index).any(|idx| {
        tokens
            .get(idx)
            .map(|token| !is_inline_marker_token_kind(token.kind))
            .unwrap_or(false)
    });
    if !has_non_marker {
        return None;
    }

    Some(InlineMarkerComponentRange {
        from: tokens[start_index].from,
        to: tokens[end_index].to,
    })
}

pub fn inline_marker_component_ranges_from_tokens(
    tokens: &[InlineToken],
) -> Vec<InlineMarkerComponentRange> {
    let mut ranges: Vec<InlineMarkerComponentRange> = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        if !is_inline_marker_token_kind(token.kind) {
            continue;
        }
        let Some(range) = marker_component_range_for_token_index(tokens, idx) else {
            continue;
        };
        if ranges
            .iter()
            .any(|existing| existing.from == range.from && existing.to == range.to)
        {
            continue;
        }
        ranges.push(range);
    }
    ranges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    ranges
}

pub fn inline_marker_component_ranges(text: &str) -> Vec<InlineMarkerComponentRange> {
    let tokens = tokenize_inline_markdown(text);
    inline_marker_component_ranges_from_tokens(&tokens)
}

fn keyword_list(lang: Option<&str>) -> &'static [&'static str] {
    const JS: &[&str] = &[
        "const",
        "let",
        "var",
        "function",
        "return",
        "if",
        "else",
        "for",
        "while",
        "switch",
        "case",
        "break",
        "continue",
        "import",
        "export",
        "from",
        "class",
        "extends",
        "new",
        "async",
        "await",
        "try",
        "catch",
        "finally",
        "throw",
        "true",
        "false",
        "null",
        "undefined",
    ];
    const RUST: &[&str] = &[
        "fn", "let", "mut", "pub", "struct", "enum", "impl", "trait", "use", "mod", "match", "if",
        "else", "for", "while", "loop", "return", "self", "Self", "crate", "super", "as", "where",
        "const", "static", "true", "false",
    ];
    const PY: &[&str] = &[
        "def", "class", "return", "if", "elif", "else", "for", "while", "try", "except", "finally",
        "with", "import", "from", "as", "break", "continue", "yield", "lambda", "True", "False",
        "None",
    ];
    const SH: &[&str] = &[
        "if", "then", "else", "fi", "for", "in", "do", "done", "case", "esac", "while", "function",
        "export", "local",
    ];

    match lang {
        Some("rust") => RUST,
        Some("python") => PY,
        Some("sh") => SH,
        Some("ts") | Some("js") | Some("go") | Some("java") | Some("c") => JS,
        _ => JS,
    }
}

fn comment_mode(lang: Option<&str>) -> CommentMode {
    match lang {
        Some("json") => CommentMode::None,
        Some("python") | Some("sh") | Some("yaml") | Some("yml") | Some("toml") => {
            CommentMode::Hash
        }
        _ => CommentMode::Slash,
    }
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn next_non_whitespace_char(chars: &[char], from: usize) -> Option<char> {
    let mut idx = from;
    while idx < chars.len() && chars[idx].is_ascii_whitespace() {
        idx += 1;
    }
    chars.get(idx).copied()
}

fn push_code_token(tokens: &mut Vec<CodeToken>, from: usize, to: usize, kind: CodeTokenType) {
    if to > from {
        tokens.push(CodeToken { from, to, kind });
    }
}

pub fn tokenize_code_line(text: &str, lang: Option<&str>) -> Vec<CodeToken> {
    let normalized_lang = lang.and_then(normalize_fence_lang);
    let keywords = keyword_list(normalized_lang.as_deref());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut protected = vec![false; len];

    // strings
    let mut i = 0;
    while i < len {
        let quote = chars[i];
        if !matches!(quote, '\'' | '"' | '`') {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut escaped = false;
        while i < len {
            let ch = chars[i];
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                i += 1;
                continue;
            }
            if ch == quote {
                i += 1;
                break;
            }
            i += 1;
        }
        push_code_token(&mut tokens, start, i, CodeTokenType::String);
        for flag in protected.iter_mut().take(i.min(len)).skip(start) {
            *flag = true;
        }
    }

    // comment
    let comment_start = match comment_mode(normalized_lang.as_deref()) {
        CommentMode::None => None,
        CommentMode::Hash => {
            let mut idx = None;
            for i in 0..len {
                if chars[i] == '#' && !protected[i] {
                    idx = Some(i);
                    break;
                }
            }
            idx
        }
        CommentMode::Slash => {
            let mut idx = None;
            for i in 0..len.saturating_sub(1) {
                if chars[i] == '/' && chars[i + 1] == '/' && !protected[i] && !protected[i + 1] {
                    idx = Some(i);
                    break;
                }
            }
            idx
        }
    };
    if let Some(start) = comment_start {
        push_code_token(&mut tokens, start, len, CodeTokenType::Comment);
        for flag in protected.iter_mut().take(len).skip(start) {
            *flag = true;
        }
    }

    // identifiers/numbers
    let mut pos = 0;
    while pos < len {
        if protected[pos] {
            pos += 1;
            continue;
        }

        let ch = chars[pos];
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = pos;
            pos += 1;
            while pos < len && is_ident_char(chars[pos]) {
                pos += 1;
            }

            if !protected[start..pos].iter().any(|flag| *flag) {
                let word: String = chars[start..pos].iter().collect();
                let kind = if keywords.contains(&word.as_str()) {
                    Some(CodeTokenType::Keyword)
                } else if next_non_whitespace_char(&chars, pos)
                    .is_some_and(|next| next == '(' || next == '!')
                {
                    Some(CodeTokenType::Function)
                } else if word
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_uppercase())
                {
                    Some(CodeTokenType::Type)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    push_code_token(&mut tokens, start, pos, kind);
                }
            }
            continue;
        }

        if ch.is_ascii_digit() {
            let prev_is_ident = pos > 0 && is_ident_char(chars[pos - 1]);
            if prev_is_ident {
                pos += 1;
                continue;
            }

            let start = pos;
            pos += 1;
            while pos < len && (chars[pos].is_ascii_digit() || chars[pos] == '_') {
                pos += 1;
            }
            if pos + 1 < len && chars[pos] == '.' && chars[pos + 1].is_ascii_digit() {
                pos += 1;
                while pos < len && (chars[pos].is_ascii_digit() || chars[pos] == '_') {
                    pos += 1;
                }
            }
            if !protected[start..pos].iter().any(|flag| *flag) {
                push_code_token(&mut tokens, start, pos, CodeTokenType::Number);
            }
            continue;
        }

        pos += 1;
    }

    tokens.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    tokens
}

pub fn analyze_lines(
    lines: &[String],
    start_in_code_block: bool,
    start_code_fence_lang: Option<&str>,
) -> MarkdownAnalyzeResult {
    let mut state = FenceState {
        in_code_block: start_in_code_block,
        code_fence_lang: start_code_fence_lang.and_then(normalize_fence_lang),
    };

    let mut analyzed = Vec::with_capacity(lines.len());
    for line in lines {
        let info = classify_markdown_line(line);
        let in_code_block = state.in_code_block;
        let code_fence_lang = state.code_fence_lang.clone();

        let mut inline_tokens = Vec::new();
        let mut code_tokens = Vec::new();

        if info.is_code_fence {
            advance_fence_state(&mut state, line);
        } else if in_code_block {
            code_tokens = tokenize_code_line(line, code_fence_lang.as_deref());
        } else {
            inline_tokens = tokenize_inline_markdown(line);
        }

        analyzed.push(MarkdownAnalyzedLine {
            info,
            in_code_block,
            code_fence_lang,
            inline_tokens,
            code_tokens,
        });
    }

    MarkdownAnalyzeResult {
        lines: analyzed,
        final_in_code_block: state.in_code_block,
        final_code_fence_lang: state.code_fence_lang,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_line_detects_markers() {
        let heading = classify_markdown_line("### Title");
        assert_eq!(heading.heading_level, Some(3));
        assert_eq!(heading.heading_marker_end, Some(4));

        let quote = classify_markdown_line(">quoted");
        assert_eq!(quote.quote_marker_end, Some(1));

        let list = classify_markdown_line("  -> item");
        assert_eq!(list.list_marker_end, Some(5));

        let ordered = classify_markdown_line("22. task");
        assert_eq!(ordered.list_marker_end, Some(4));

        let numeric_date = classify_markdown_line("22.04.2026. 14:34");
        assert_eq!(numeric_date.list_marker_end, None);

        let textual_date = classify_markdown_line("22. April 2026.");
        assert_eq!(textual_date.list_marker_end, None);

        let checklist = classify_markdown_line("- [x] done");
        assert_eq!(checklist.checklist_marker_start, Some(2));
        assert_eq!(checklist.checklist_marker_end, Some(5));
        assert_eq!(checklist.checklist_content_start, Some(6));
        assert!(checklist.checklist_checked);

        let fence = classify_markdown_line("```ts");
        assert!(fence.is_code_fence);

        let rule = classify_markdown_line("---");
        assert!(rule.is_horizontal_rule);
    }

    #[test]
    fn inline_tokenizer_finds_rich_tokens() {
        let tokens =
            tokenize_inline_markdown("**bold** *em* ~~gone~~ `code` ![pic](img.png) [txt](url)");
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"strong"));
        assert!(kinds.contains(&"emphasis"));
        assert!(kinds.contains(&"strikethrough"));
        assert!(kinds.contains(&"code"));
        assert!(kinds.contains(&"image-alt"));
        assert!(kinds.contains(&"image-src"));
        assert!(kinds.contains(&"link-text"));
        assert!(kinds.contains(&"link-url"));
    }

    #[test]
    fn image_tokens_do_not_interfere_with_markdown_links() {
        let text = "![shot](./shot.png) and [site](https://example.com)";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"image-marker"));
        assert!(kinds.contains(&"image-alt"));
        assert!(kinds.contains(&"image-src"));
        assert!(kinds.contains(&"link-text"));
        assert!(kinds.contains(&"link-url"));
    }

    #[test]
    fn find_markdown_image_matches_extracts_alt_and_src() {
        let text = "x ![diagram](./assets/plan.png){width=320 height=180} y ![](./empty-alt.jpg)";
        let matches = find_markdown_image_matches(text);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].alt, "diagram");
        assert_eq!(matches[0].src, "./assets/plan.png");
        assert_eq!(matches[0].width, Some(320));
        assert_eq!(matches[0].height, Some(180));
        assert_eq!(matches[1].alt, "");
        assert_eq!(matches[1].src, "./empty-alt.jpg");
        assert_eq!(matches[1].width, None);
        assert_eq!(matches[1].height, None);
    }

    #[test]
    fn inline_tokenizer_does_not_treat_mid_word_markers_as_formatting() {
        let tokens = tokenize_inline_markdown("this_Is_my_Word this*is*mid this~~is~~mid");
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(!kinds.contains(&"emphasis"));
        assert!(!kinds.contains(&"strong"));
        assert!(!kinds.contains(&"strikethrough"));
    }

    #[test]
    fn inline_marker_component_ranges_identify_contiguous_marker_components() {
        let ranges = inline_marker_component_ranges("**bold** and `code`");
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], InlineMarkerComponentRange { from: 0, to: 8 });
        assert_eq!(ranges[1], InlineMarkerComponentRange { from: 13, to: 19 });
    }

    #[test]
    fn inline_marker_component_ranges_skip_marker_only_runs() {
        let ranges = inline_marker_component_ranges("****");
        assert!(ranges.is_empty());
    }

    #[test]
    fn code_tokenizer_marks_expected_kinds() {
        let rust = tokenize_code_line(
            "let total: Result = parse_value(42); let s = \"ok\" // note",
            Some("rust"),
        );
        let rust_kinds: Vec<&str> = rust.iter().map(|t| t.kind.as_str()).collect();
        assert!(rust_kinds.contains(&"keyword"));
        assert!(rust_kinds.contains(&"string"));
        assert!(rust_kinds.contains(&"comment"));
        assert!(rust_kinds.contains(&"number"));
        assert!(rust_kinds.contains(&"function"));
        assert!(rust_kinds.contains(&"type"));

        let sh = tokenize_code_line("if [ $x -eq 1 ]; then # done", Some("sh"));
        let sh_kinds: Vec<&str> = sh.iter().map(|t| t.kind.as_str()).collect();
        assert!(sh_kinds.contains(&"keyword"));
        assert!(sh_kinds.contains(&"comment"));
    }

    #[test]
    fn analyze_lines_tracks_fence_state_across_lines() {
        let lines = vec![
            "```rust".to_string(),
            "let x = 1;".to_string(),
            "```".to_string(),
            "plain".to_string(),
        ];
        let analyzed = analyze_lines(&lines, false, None);
        assert_eq!(analyzed.lines.len(), 4);
        assert!(analyzed.lines[0].info.is_code_fence);
        assert!(analyzed.lines[1].in_code_block);
        assert!(!analyzed.lines[3].in_code_block);
        assert!(!analyzed.final_in_code_block);
    }

    #[test]
    fn wiki_link_basic() {
        let text = "see [[01HX4VHR|My Note]] here";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"wiki-link-marker"));
        assert!(kinds.contains(&"wiki-link-id"));
        assert!(kinds.contains(&"wiki-link-sep"));
        assert!(kinds.contains(&"wiki-link-title"));
        let id_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-id")
            .unwrap();
        let title_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-title")
            .unwrap();
        assert_eq!(&text[id_tok.from..id_tok.to], "01HX4VHR");
        assert_eq!(&text[title_tok.from..title_tok.to], "My Note");
    }

    #[test]
    fn wiki_link_with_anchor() {
        let text = "[[01HX4VHR#The Beginning|Intro]]";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"wiki-link-title"));
        assert!(kinds.contains(&"wiki-link-anchor"));
        let title_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-title")
            .unwrap();
        let anchor_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-anchor")
            .unwrap();
        assert_eq!(&text[title_tok.from..title_tok.to], "Intro");
        assert_eq!(&text[anchor_tok.from..anchor_tok.to], "#The Beginning");
    }

    #[test]
    fn wiki_link_invalid_id_not_parsed() {
        // ID with spaces (not alphanumeric)
        let tokens = tokenize_inline_markdown("[[not vali|Title]]");
        assert!(!tokens.iter().any(|t| t.kind.as_str() == "wiki-link-id"));
        // wrong length (too short)
        let tokens = tokenize_inline_markdown("[[ABC|Title]]");
        assert!(!tokens.iter().any(|t| t.kind.as_str() == "wiki-link-id"));
        // wrong length (too long)
        let tokens = tokenize_inline_markdown("[[01HX4VHRXX|Title]]");
        assert!(!tokens.iter().any(|t| t.kind.as_str() == "wiki-link-id"));
    }

    #[test]
    fn wiki_link_no_pipe() {
        let text = "[[01HX4VHR]]";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"wiki-link-id"));
        assert!(!kinds.contains(&"wiki-link-sep"));
        assert!(!kinds.contains(&"wiki-link-title"));
        assert!(!kinds.contains(&"wiki-link-anchor"));
        let id_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-id")
            .unwrap();
        assert_eq!(&text[id_tok.from..id_tok.to], "01HX4VHR");
    }

    #[test]
    fn wiki_link_heading_no_pipe() {
        let text = "[[01HX4VHR#My Section]]";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"wiki-link-id"));
        assert!(kinds.contains(&"wiki-link-anchor"));
        assert!(!kinds.contains(&"wiki-link-sep"));
        assert!(!kinds.contains(&"wiki-link-title"));
        let id_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-id")
            .unwrap();
        let anchor_tok = tokens
            .iter()
            .find(|t| t.kind.as_str() == "wiki-link-anchor")
            .unwrap();
        assert_eq!(&text[id_tok.from..id_tok.to], "01HX4VHR");
        assert_eq!(&text[anchor_tok.from..anchor_tok.to], "#My Section");
    }

    #[test]
    fn wiki_link_does_not_interfere_with_markdown_link() {
        let text = "[normal](http://example.com) [[01HX4VHR|My Note]]";
        let tokens = tokenize_inline_markdown(text);
        let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
        assert!(kinds.contains(&"link-text"));
        assert!(kinds.contains(&"link-url"));
        assert!(kinds.contains(&"wiki-link-title"));
    }

    #[test]
    fn wiki_link_protected_from_inner_formatting() {
        // bold markers inside a wiki link should not produce strong tokens
        let text = "[[01HX4VHR|**Note**]]";
        let tokens = tokenize_inline_markdown(text);
        assert!(!tokens.iter().any(|t| t.kind.as_str() == "strong"));
        assert!(tokens.iter().any(|t| t.kind.as_str() == "wiki-link-title"));
    }

    #[test]
    fn find_wiki_link_matches_extracts_parts() {
        let text = "A [[01HX4VHR#Intro|Alt]] and [[01HX4VHS]]";
        let links = find_wiki_link_matches(text);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].short_id, "01HX4VHR");
        assert_eq!(links[0].heading.as_deref(), Some("Intro"));
        assert_eq!(links[0].title.as_deref(), Some("Alt"));
        assert_eq!(links[1].short_id, "01HX4VHS");
        assert_eq!(links[1].heading, None);
        assert_eq!(links[1].title, None);
    }

    #[test]
    fn wiki_link_at_cursor_finds_link_at_boundaries() {
        let text = "[[01HX4VHR#Intro]] tail";
        let start = 0usize;
        let end = "[[01HX4VHR#Intro]]".chars().count();
        assert!(wiki_link_at_cursor(text, start).is_some());
        assert!(wiki_link_at_cursor(text, 6).is_some());
        assert!(wiki_link_at_cursor(text, end).is_some());
        assert!(wiki_link_at_cursor(text, end + 1).is_none());
    }

    #[test]
    fn extract_markdown_headings_collects_unique_headings() {
        let text = "# Intro\n## Setup ##\ntext\n### Intro\n#### Deep Dive";
        let headings = extract_markdown_headings(text);
        assert_eq!(
            headings,
            vec![
                "Intro".to_string(),
                "Setup".to_string(),
                "Deep Dive".to_string()
            ]
        );
    }
}
