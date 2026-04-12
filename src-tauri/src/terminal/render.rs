pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const STRIKETHROUGH: &str = "\x1b[9m";
const REVERSE: &str = "\x1b[7m";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CharStyle {
    bold: bool,
    italic: bool,
    dim: bool,
    strikethrough: bool,
    reverse: bool,
}

impl CharStyle {
    fn write_ansi(&self, buf: &mut String) {
        buf.push_str(RESET);
        if self.bold {
            buf.push_str(BOLD);
        }
        if self.dim {
            buf.push_str(DIM);
        }
        if self.italic {
            buf.push_str(ITALIC);
        }
        if self.strikethrough {
            buf.push_str(STRIKETHROUGH);
        }
        if self.reverse {
            buf.push_str(REVERSE);
        }
    }

    fn is_plain(&self) -> bool {
        !self.bold && !self.italic && !self.dim && !self.strikethrough && !self.reverse
    }
}

pub struct RenderContext {
    in_code_block: bool,
}

impl RenderContext {
    pub fn new() -> Self {
        Self {
            in_code_block: false,
        }
    }

    pub fn advance_line(&mut self, text: &str) {
        if is_code_fence(text) {
            self.in_code_block = !self.in_code_block;
        }
    }

    pub fn reset(&mut self) {
        self.in_code_block = false;
    }

    /// Render a single line with ANSI markdown formatting.
    /// Returns an ANSI string occupying exactly `width` visible characters.
    pub fn render_line(
        &mut self,
        text: &str,
        width: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
    ) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut styles = vec![CharStyle::default(); len];

        let is_fence = is_code_fence(text);

        if is_fence {
            for s in &mut styles {
                s.dim = true;
            }
            self.in_code_block = !self.in_code_block;
        } else if self.in_code_block {
            for s in &mut styles {
                s.dim = true;
            }
        } else {
            apply_line_styles(&chars, &mut styles);
            apply_inline_styles(&chars, &mut styles);
        }

        for &(start, end) in search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.reverse = true;
            }
        }

        build_ansi_output(&chars, &styles, width, calc_ghost)
    }
}

fn is_code_fence(text: &str) -> bool {
    text.trim_start().starts_with("```")
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
    let mut buf = String::with_capacity(width * 3);
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
        buf.push(ch);
        visible += 1;
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
    }

    #[test]
    fn render_context_tracks_fences() {
        let mut ctx = RenderContext::new();
        ctx.advance_line("normal line");
        assert!(!ctx.in_code_block);
        ctx.advance_line("```");
        assert!(ctx.in_code_block);
        ctx.advance_line("code line");
        assert!(ctx.in_code_block);
        ctx.advance_line("```");
        assert!(!ctx.in_code_block);
    }

    #[test]
    fn render_plain_pads_to_width() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("hi", 10, None, &[]);
        // "hi" + 8 spaces = 10 visible chars (plus potential ANSI reset)
        let visible: String = strip_ansi(&out);
        assert_eq!(visible.len(), 10);
        assert!(visible.starts_with("hi"));
    }

    #[test]
    fn render_calc_ghost_appended() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("2+2", 30, Some("4"), &[]);
        let visible = strip_ansi(&out);
        assert!(visible.contains("→ 4"));
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
