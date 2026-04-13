use std::fmt::Write as _;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const TAB_WIDTH: usize = 4;
const FG_CODE_KEYWORD: u8 = 81;
const FG_CODE_STRING: u8 = 114;
const FG_CODE_NUMBER: u8 = 215;
const FG_CODE_COMMENT: u8 = 244;
const FG_CODE_FUNCTION: u8 = 74;
const FG_CODE_TYPE: u8 = 183;
const FG_VARIABLE: u8 = 179;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CharStyle {
    bold: bool,
    italic: bool,
    dim: bool,
    strikethrough: bool,
    reverse: bool,
    fg: Option<u8>,
}

impl CharStyle {
    fn write_ansi(&self, buf: &mut String) {
        // Emit a single combined SGR sequence: \x1b[0;1;2;...m
        buf.push_str("\x1b[0");
        if self.bold {
            buf.push_str(";1");
        }
        if self.dim {
            buf.push_str(";2");
        }
        if self.italic {
            buf.push_str(";3");
        }
        if self.reverse {
            buf.push_str(";7");
        }
        if self.strikethrough {
            buf.push_str(";9");
        }
        if let Some(color) = self.fg {
            let _ = write!(buf, ";38;5;{color}");
        }
        buf.push('m');
    }

    fn is_plain(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.dim
            && !self.strikethrough
            && !self.reverse
            && self.fg.is_none()
    }
}

pub struct RenderContext {
    in_code_block: bool,
    code_fence_lang: Option<String>,
}

impl RenderContext {
    pub fn new() -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
        }
    }

    /// Skip ahead through `lines` without rendering — just track code fence state.
    pub fn advance_lines(&mut self, lines: &[String]) {
        for line in lines {
            if is_code_fence(line) {
                if self.in_code_block {
                    self.in_code_block = false;
                    self.code_fence_lang = None;
                } else {
                    self.in_code_block = true;
                    self.code_fence_lang = fence_language(line);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn advance_line(&mut self, text: &str) {
        if is_code_fence(text) {
            if self.in_code_block {
                self.in_code_block = false;
                self.code_fence_lang = None;
            } else {
                self.in_code_block = true;
                self.code_fence_lang = fence_language(text);
            }
        }
    }

    /// Render a single line with ANSI markdown formatting.
    /// Returns an ANSI string occupying exactly `width` visible characters.
    pub fn render_line(
        &mut self,
        text: &str,
        width: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        variable_names: &[String],
    ) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut styles = vec![CharStyle::default(); len];

        let is_fence = is_code_fence(text);

        if is_fence {
            for s in &mut styles {
                s.dim = true;
            }
            if self.in_code_block {
                self.in_code_block = false;
                self.code_fence_lang = None;
            } else {
                self.in_code_block = true;
                self.code_fence_lang = fence_language(text);
            }
        } else if self.in_code_block {
            apply_code_block_styles(&chars, &mut styles, self.code_fence_lang.as_deref());
        } else {
            apply_line_styles(&chars, &mut styles);
            apply_inline_styles(&chars, &mut styles);
            apply_variable_styles(&chars, &mut styles, variable_names);
        }

        for &(start, end) in search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.reverse = true;
            }
        }

        build_ansi_output(&chars, &styles, width, calc_ghost)
    }
}

fn is_variable_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn has_variable_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let left_ok = start == 0 || !is_variable_word_byte(bytes[start - 1]);
    let right_ok = end == bytes.len() || !is_variable_word_byte(bytes[end]);
    left_ok && right_ok
}

fn find_variable_ranges(text: &str, variable_names: &[String]) -> Vec<(usize, usize)> {
    if text.is_empty() || variable_names.is_empty() {
        return Vec::new();
    }

    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for raw in variable_names {
        let needle_text = raw.trim().to_ascii_lowercase();
        if needle_text.is_empty() {
            continue;
        }
        let needle = needle_text.as_bytes();
        if needle.len() > bytes.len() {
            continue;
        }

        let mut idx = 0usize;
        while idx + needle.len() <= bytes.len() {
            let end = idx + needle.len();
            if &bytes[idx..end] == needle && has_variable_word_boundaries(bytes, idx, end) {
                matches.push((idx, end));
            }
            idx += 1;
        }
    }

    if matches.len() <= 1 {
        return matches;
    }

    // Prefer left-most ranges; for overlaps at same start, keep longer match.
    matches.sort_by(|a, b| a.0.cmp(&b.0).then((b.1 - b.0).cmp(&(a.1 - a.0))));

    let mut deduped = Vec::new();
    for candidate in matches {
        let Some(last) = deduped.last() else {
            deduped.push(candidate);
            continue;
        };
        if candidate.0 < last.1 {
            continue;
        }
        deduped.push(candidate);
    }

    deduped
}

fn apply_variable_styles(chars: &[char], styles: &mut [CharStyle], variable_names: &[String]) {
    if chars.is_empty() || variable_names.is_empty() {
        return;
    }

    let text: String = chars.iter().collect();
    for (start, end) in find_variable_ranges(&text, variable_names) {
        for style in styles.iter_mut().take(end).skip(start) {
            style.fg = Some(FG_VARIABLE);
            style.bold = true;
            style.dim = false;
        }
    }
}

fn is_code_fence(text: &str) -> bool {
    text.trim_start().starts_with("```")
}

fn normalize_fence_lang(raw: &str) -> Option<String> {
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

fn fence_language(text: &str) -> Option<String> {
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
        "else", "for", "while", "loop", "return", "self", "Self", "crate", "super", "where",
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

fn comment_mode(lang: Option<&str>) -> &'static str {
    match lang {
        Some("json") => "none",
        Some("python") | Some("sh") | Some("yaml") | Some("yml") | Some("toml") => "hash",
        _ => "slash",
    }
}

fn apply_style_range(
    styles: &mut [CharStyle],
    protected: &mut [bool],
    from: usize,
    to: usize,
    fg: u8,
    dim: bool,
) {
    let end = to.min(styles.len());
    for i in from.min(end)..end {
        styles[i].fg = Some(fg);
        styles[i].dim = dim;
        protected[i] = true;
    }
}

fn apply_code_block_styles(chars: &[char], styles: &mut [CharStyle], lang: Option<&str>) {
    for style in styles.iter_mut() {
        style.dim = true;
        style.fg = None;
    }
    if chars.is_empty() {
        return;
    }

    let mut protected = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if !matches!(ch, '"' | '\'' | '`') {
            i += 1;
            continue;
        }
        let quote = ch;
        let start = i;
        i += 1;
        let mut escaped = false;
        while i < chars.len() {
            let current = chars[i];
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if current == '\\' {
                escaped = true;
                i += 1;
                continue;
            }
            if current == quote {
                i += 1;
                break;
            }
            i += 1;
        }
        apply_style_range(styles, &mut protected, start, i, FG_CODE_STRING, false);
    }

    match comment_mode(lang) {
        "hash" => {
            for idx in 0..chars.len() {
                if chars[idx] == '#' && !protected[idx] {
                    apply_style_range(
                        styles,
                        &mut protected,
                        idx,
                        chars.len(),
                        FG_CODE_COMMENT,
                        true,
                    );
                    break;
                }
            }
        }
        "slash" => {
            for idx in 0..chars.len().saturating_sub(1) {
                if chars[idx] == '/'
                    && chars[idx + 1] == '/'
                    && !protected[idx]
                    && !protected[idx + 1]
                {
                    apply_style_range(
                        styles,
                        &mut protected,
                        idx,
                        chars.len(),
                        FG_CODE_COMMENT,
                        true,
                    );
                    break;
                }
            }
        }
        _ => {}
    }

    let keywords = keyword_list(lang);
    let mut pos = 0;
    while pos < chars.len() {
        if protected[pos] {
            pos += 1;
            continue;
        }

        let ch = chars[pos];
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = pos;
            pos += 1;
            while pos < chars.len() && is_ident_char(chars[pos]) {
                pos += 1;
            }
            if !protected[start..pos].iter().any(|&v| v) {
                let word: String = chars[start..pos].iter().collect();
                let color = if keywords.contains(&word.as_str()) {
                    Some(FG_CODE_KEYWORD)
                } else if next_non_whitespace_char(chars, pos)
                    .is_some_and(|ch| ch == '(' || ch == '!')
                {
                    Some(FG_CODE_FUNCTION)
                } else if word
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                {
                    Some(FG_CODE_TYPE)
                } else {
                    None
                };
                if let Some(color) = color {
                    for idx in start..pos {
                        styles[idx].fg = Some(color);
                        styles[idx].dim = false;
                    }
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
            while pos < chars.len() && (chars[pos].is_ascii_digit() || chars[pos] == '_') {
                pos += 1;
            }
            if pos + 1 < chars.len() && chars[pos] == '.' && chars[pos + 1].is_ascii_digit() {
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_digit() || chars[pos] == '_') {
                    pos += 1;
                }
            }
            if !protected[start..pos].iter().any(|&v| v) {
                for idx in start..pos {
                    styles[idx].fg = Some(FG_CODE_NUMBER);
                    styles[idx].dim = false;
                }
            }
            continue;
        }

        pos += 1;
    }
}

fn apply_line_styles(chars: &[char], styles: &mut [CharStyle]) {
    let text: String = chars.iter().collect();

    if let Some((_, marker_end)) = heading_marker_end(&text) {
        for (i, s) in styles.iter_mut().enumerate() {
            if i < marker_end.min(chars.len()) {
                s.dim = true;
            } else {
                s.bold = true;
            }
        }
        return;
    }

    if let Some(marker_end) = quote_marker_end(&text) {
        for (i, s) in styles.iter_mut().enumerate() {
            if i < marker_end {
                s.dim = true;
            } else {
                s.italic = true;
            }
        }
        return;
    }

    if is_horizontal_rule(&text) {
        for s in styles.iter_mut() {
            s.dim = true;
        }
        return;
    }

    if let Some((marker_end, checked)) = checklist_marker_end(&text) {
        for s in styles.iter_mut().take(marker_end.min(chars.len())) {
            s.dim = true;
        }
        if checked {
            for s in styles.iter_mut().skip(marker_end) {
                s.strikethrough = true;
                s.dim = true;
            }
        }
        return;
    }

    if let Some(marker_end) = list_marker_end(&text) {
        for s in styles.iter_mut().take(marker_end.min(chars.len())) {
            s.dim = true;
        }
    }
}

fn heading_marker_end(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let mut level = 0;
    while i < bytes.len() && level < 6 && bytes[i] == b'#' {
        level += 1;
        i += 1;
    }
    if level > 0 && i < bytes.len() && bytes[i] == b' ' {
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        Some((level, i))
    } else {
        None
    }
}

fn quote_marker_end(text: &str) -> Option<usize> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('>') {
        return None;
    }
    let leading = text.len() - trimmed.len();
    let bytes = text.as_bytes();
    let mut i = leading;
    while i < bytes.len() && bytes[i] == b'>' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b' ' {
        Some(i + 1)
    } else {
        Some(i)
    }
}

fn is_horizontal_rule(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let first = match trimmed.chars().find(|c| !c.is_whitespace()) {
        Some(c @ ('-' | '*' | '_')) => c,
        _ => return false,
    };
    let count = trimmed.chars().filter(|&c| c == first).count();
    let all_valid = trimmed.chars().all(|c| c == first || c.is_whitespace());
    count >= 3 && all_valid
}

pub fn list_marker_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }

    let mut marker_end = i;
    while marker_end < bytes.len() && !bytes[marker_end].is_ascii_whitespace() {
        marker_end += 1;
    }
    if marker_end == i || marker_end >= bytes.len() || !bytes[marker_end].is_ascii_whitespace() {
        return None;
    }

    let marker = &text[i..marker_end];
    let is_digits = |segment: &str| {
        !segment.is_empty() && segment.as_bytes().iter().all(|b| b.is_ascii_digit())
    };
    let is_ordered = if let Some(stripped) = marker.strip_suffix('.') {
        is_digits(stripped) || (stripped.contains('.') && stripped.split('.').all(is_digits))
    } else {
        marker.contains('.') && marker.split('.').all(is_digits)
    };
    if !matches!(marker, "-" | "*" | "+" | "->") && !is_ordered {
        return None;
    }

    while marker_end < bytes.len() && bytes[marker_end].is_ascii_whitespace() {
        marker_end += 1;
    }
    Some(marker_end)
}

pub fn checklist_marker_end(text: &str) -> Option<(usize, bool)> {
    if let Some(list_end) = list_marker_end(text) {
        let rest = &text[list_end..];
        let bytes = rest.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'[' && bytes[2] == b']' {
            let ch = bytes[1];
            if ch == b' ' || ch == b'x' || ch == b'X' {
                let checked = ch == b'x' || ch == b'X';
                let mut end = list_end + 3;
                while end < text.len() && text.as_bytes()[end] == b' ' {
                    end += 1;
                }
                return Some((end, checked));
            }
        }
    }
    None
}

// --- Inline markdown scanning ---

fn apply_inline_styles(chars: &[char], styles: &mut [CharStyle]) {
    let len = chars.len();
    if len == 0 {
        return;
    }
    let mut claimed = vec![false; len];

    scan_code_spans(chars, styles, &mut claimed);
    scan_paired(chars, styles, &mut claimed, '*', 2, |s| s.bold = true);
    scan_paired(chars, styles, &mut claimed, '_', 2, |s| s.bold = true);
    scan_paired(chars, styles, &mut claimed, '~', 2, |s| {
        s.strikethrough = true
    });
    scan_single_em(chars, styles, &claimed, '*');
    scan_single_em(chars, styles, &claimed, '_');
}

fn scan_code_spans(chars: &[char], styles: &mut [CharStyle], claimed: &mut [bool]) {
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if claimed[i] || chars[i] != '`' {
            i += 1;
            continue;
        }
        let open = i;
        let mut bt = 0;
        while i < len && chars[i] == '`' {
            bt += 1;
            i += 1;
        }
        if let Some(close) = find_backtick_close(chars, i, bt) {
            for k in open..close + bt {
                if k < len {
                    styles[k].dim = true;
                    claimed[k] = true;
                }
            }
            i = close + bt;
        }
    }
}

fn find_backtick_close(chars: &[char], from: usize, count: usize) -> Option<usize> {
    let len = chars.len();
    let mut i = from;
    while i + count <= len {
        if chars[i] == '`' {
            let start = i;
            let mut c = 0;
            while i < len && chars[i] == '`' {
                c += 1;
                i += 1;
            }
            if c == count {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn scan_paired(
    chars: &[char],
    styles: &mut [CharStyle],
    claimed: &mut [bool],
    delim: char,
    delim_len: usize,
    apply: fn(&mut CharStyle),
) {
    let len = chars.len();
    if len < delim_len * 2 + 1 {
        return;
    }
    let mut i = 0;
    while i + delim_len * 2 < len {
        if claimed[i] {
            i += 1;
            continue;
        }
        if !is_run(chars, i, delim, delim_len) {
            i += 1;
            continue;
        }
        let after = i + delim_len;
        if after < len && chars[after].is_whitespace() {
            i += 1;
            continue;
        }
        if let Some(close) = find_paired_close(chars, claimed, after, delim, delim_len) {
            for k in i..i + delim_len {
                styles[k].dim = true;
                claimed[k] = true;
            }
            for k in after..close {
                if !claimed[k] {
                    apply(&mut styles[k]);
                }
            }
            for k in close..close + delim_len {
                styles[k].dim = true;
                claimed[k] = true;
            }
            i = close + delim_len;
        } else {
            i += 1;
        }
    }
}

fn is_run(chars: &[char], pos: usize, ch: char, count: usize) -> bool {
    for j in 0..count {
        if pos + j >= chars.len() || chars[pos + j] != ch {
            return false;
        }
    }
    true
}

fn find_paired_close(
    chars: &[char],
    claimed: &[bool],
    from: usize,
    delim: char,
    delim_len: usize,
) -> Option<usize> {
    let len = chars.len();
    let mut i = from;
    while i + delim_len <= len {
        if !claimed[i] && is_run(chars, i, delim, delim_len) {
            if i > 0 && !chars[i - 1].is_whitespace() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn scan_single_em(chars: &[char], styles: &mut [CharStyle], claimed: &[bool], delim: char) {
    let len = chars.len();
    let mut i = 0;
    while i + 2 < len {
        if claimed[i] || chars[i] != delim {
            i += 1;
            continue;
        }
        // Skip if this is a double delimiter
        if i + 1 < len && chars[i + 1] == delim {
            i += 2;
            continue;
        }
        if i + 1 < len && chars[i + 1].is_whitespace() {
            i += 1;
            continue;
        }
        if let Some(close) = find_single_close(chars, claimed, i + 1, delim) {
            styles[i].dim = true;
            for k in i + 1..close {
                if !claimed[k] {
                    styles[k].italic = true;
                }
            }
            styles[close].dim = true;
            i = close + 1;
        } else {
            i += 1;
        }
    }
}

fn find_single_close(chars: &[char], claimed: &[bool], from: usize, delim: char) -> Option<usize> {
    let len = chars.len();
    let mut i = from;
    while i < len {
        if !claimed[i] && chars[i] == delim {
            if i + 1 < len && chars[i + 1] == delim {
                i += 2;
                continue;
            }
            if i > 0 && !chars[i - 1].is_whitespace() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

// --- Output ---

fn build_ansi_output(
    chars: &[char],
    styles: &[CharStyle],
    width: usize,
    calc_ghost: Option<&str>,
) -> String {
    let mut buf = String::with_capacity(width * 4);
    let mut current = CharStyle::default();
    let mut visible = 0;

    for (i, &ch) in chars.iter().enumerate() {
        if visible >= width {
            break;
        }
        let s = styles[i];
        if s != current {
            s.write_ansi(&mut buf);
            current = s;
        }
        if ch == '\t' {
            let tab_spaces = TAB_WIDTH - (visible % TAB_WIDTH);
            for _ in 0..tab_spaces {
                if visible >= width {
                    break;
                }
                buf.push(' ');
                visible += 1;
            }
        } else {
            buf.push(ch);
            visible += 1;
        }
    }

    if let Some(ghost) = calc_ghost {
        if visible < width {
            let ghost_style = CharStyle {
                dim: true,
                italic: true,
                ..Default::default()
            };
            if current != ghost_style {
                ghost_style.write_ansi(&mut buf);
                current = ghost_style;
            }
            for ch in " → ".chars().chain(ghost.chars()) {
                if visible >= width {
                    break;
                }
                buf.push(ch);
                visible += 1;
            }
        }
    }

    if !current.is_plain() {
        buf.push_str(RESET);
    }
    while visible < width {
        buf.push(' ');
        visible += 1;
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_detected() {
        assert_eq!(heading_marker_end("# Hello"), Some((1, 2)));
        assert_eq!(heading_marker_end("  ### Foo"), Some((3, 6)));
        assert_eq!(heading_marker_end("Not a heading"), None);
        assert_eq!(heading_marker_end("#nospace"), None);
    }

    #[test]
    fn list_marker_detected() {
        assert_eq!(list_marker_end("- item"), Some(2));
        assert_eq!(list_marker_end("  * item"), Some(4));
        assert_eq!(list_marker_end("1. item"), Some(3));
        assert_eq!(list_marker_end("1.1 item"), Some(4));
        assert_eq!(list_marker_end("  -> item"), Some(5));
        assert_eq!(list_marker_end("10. item"), Some(4));
        assert_eq!(list_marker_end("plain text"), None);
    }

    #[test]
    fn horizontal_rule_detected() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("- - -"));
        assert!(is_horizontal_rule("***"));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("hello"));
    }

    #[test]
    fn code_fence_detected() {
        assert!(is_code_fence("```"));
        assert!(is_code_fence("  ```rust"));
        assert!(!is_code_fence("hello"));
        assert_eq!(fence_language("```rust"), Some("rust".to_string()));
        assert_eq!(fence_language("```typescript"), Some("ts".to_string()));
    }

    #[test]
    fn render_context_tracks_fences() {
        let mut ctx = RenderContext::new();
        ctx.advance_line("normal line");
        assert!(!ctx.in_code_block);
        ctx.advance_line("```rust");
        assert!(ctx.in_code_block);
        assert_eq!(ctx.code_fence_lang.as_deref(), Some("rust"));
        ctx.advance_line("code line");
        assert!(ctx.in_code_block);
        ctx.advance_line("```");
        assert!(!ctx.in_code_block);
        assert_eq!(ctx.code_fence_lang, None);
    }

    #[test]
    fn render_plain_pads_to_width() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("hi", 10, None, &[], &[]);
        // "hi" + 8 spaces = 10 visible chars (plus potential ANSI reset)
        let visible: String = strip_ansi(&out);
        assert_eq!(visible.len(), 10);
        assert!(visible.starts_with("hi"));
    }

    #[test]
    fn render_calc_ghost_appended() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("2+2", 30, Some("4"), &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.contains("→ 4"));
    }

    #[test]
    fn render_code_block_adds_syntax_color_sequences() {
        let mut ctx = RenderContext::new();
        let _ = ctx.render_line("```rust", 60, None, &[], &[]);
        let out = ctx.render_line("let total = 42 // note", 60, None, &[], &[]);
        assert!(out.contains("38;5;81"));
        assert!(out.contains("38;5;215"));
        assert!(out.contains("38;5;244"));
    }

    #[test]
    fn render_expands_tabs_into_spaces() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("a\tb", 12, None, &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.starts_with("a   b"));
        assert_eq!(visible.len(), 12);
    }

    #[test]
    fn render_highlights_variables_in_bold_with_distinct_color() {
        let mut ctx = RenderContext::new();
        let vars = vec!["subtotal".to_string(), "tax rate".to_string()];
        let out = ctx.render_line("total = subtotal + tax rate", 80, None, &[], &vars);
        assert!(out.contains(&format!("0;1;38;5;{FG_VARIABLE}")));
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_esc = false;
        for ch in s.chars() {
            if ch == '\x1b' {
                in_esc = true;
            } else if in_esc {
                if ch.is_ascii_alphabetic() {
                    in_esc = false;
                }
            } else {
                out.push(ch);
            }
        }
        out
    }
}
