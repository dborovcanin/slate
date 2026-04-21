use crate::context::ResolvedContext;
use crate::operations::replace_range;
use crate::types::{CommandExecutionResult, CommandMode, EditorContextSnapshot};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimSubstituteScope {
    CurrentLine,
    WholeDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VimSubstituteFlags {
    pub global: bool,
    pub ignore_case: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimSubstituteCommand {
    pub scope: VimSubstituteScope,
    pub pattern: String,
    pub replacement: String,
    pub flags: VimSubstituteFlags,
}

fn result_with_message(message: impl Into<String>) -> CommandExecutionResult {
    CommandExecutionResult {
        message: message.into(),
        operations: Vec::new(),
        clipboard_text: None,
        quit_requested: false,
    }
}

fn parse_delimited_segment(input: &str, delimiter: char) -> Result<(String, usize), String> {
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if ch == delimiter && !escaped {
            return Ok((input[..idx].to_string(), idx + ch.len_utf8()));
        }
        if ch == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
    }
    Err("unterminated substitute command".to_string())
}

fn unescape_substitute_segment(segment: &str, delimiter: char) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut chars = segment.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.peek().copied() {
                if next == delimiter || next == '\\' {
                    out.push(next);
                    let _ = chars.next();
                    continue;
                }
            }
            out.push(ch);
            continue;
        }
        out.push(ch);
    }
    out
}

pub fn parse_vim_substitute_command(
    raw_input: &str,
) -> Result<Option<VimSubstituteCommand>, String> {
    let trimmed = raw_input.trim();
    let command = trimmed.strip_prefix(':').unwrap_or(trimmed);
    if command.is_empty() {
        return Ok(None);
    }

    let (scope, remainder) = if let Some(rest) = command.strip_prefix("%s") {
        (VimSubstituteScope::WholeDocument, rest)
    } else if let Some(rest) = command.strip_prefix('s') {
        (VimSubstituteScope::CurrentLine, rest)
    } else {
        return Ok(None);
    };

    let mut remainder_chars = remainder.chars();
    let Some(delimiter) = remainder_chars.next() else {
        return Err("missing delimiter".to_string());
    };
    if delimiter.is_ascii_alphanumeric() || delimiter.is_ascii_whitespace() {
        return Ok(None);
    }

    let payload = &remainder[delimiter.len_utf8()..];
    let (raw_pattern, consumed_pattern) = parse_delimited_segment(payload, delimiter)?;
    let after_pattern = &payload[consumed_pattern..];
    let (raw_replacement, consumed_replacement) =
        parse_delimited_segment(after_pattern, delimiter)?;
    let trailing_flags = after_pattern[consumed_replacement..].trim();

    let mut flags = VimSubstituteFlags {
        global: false,
        ignore_case: false,
    };
    for flag in trailing_flags.chars() {
        match flag {
            'g' => flags.global = true,
            'i' => flags.ignore_case = true,
            _ => return Err(format!("unsupported substitute flag: {flag}")),
        }
    }

    let pattern = unescape_substitute_segment(&raw_pattern, delimiter);
    if pattern.is_empty() {
        return Err("empty pattern is not supported".to_string());
    }
    let replacement = unescape_substitute_segment(&raw_replacement, delimiter);

    Ok(Some(VimSubstituteCommand {
        scope,
        pattern,
        replacement,
        flags,
    }))
}

pub fn apply_vim_substitute(
    snapshot: &EditorContextSnapshot,
    mode: CommandMode,
    command: &VimSubstituteCommand,
) -> CommandExecutionResult {
    let pattern = if command.flags.ignore_case {
        format!("(?i:{})", command.pattern)
    } else {
        command.pattern.clone()
    };
    let matcher = match Regex::new(&pattern) {
        Ok(regex) => regex,
        Err(error) => {
            return result_with_message(format!("substitute: invalid pattern ({error})"));
        }
    };

    let ctx = ResolvedContext::new(snapshot.clone());
    let selection = ctx.selection();
    let (start_line, end_line) = match command.scope {
        VimSubstituteScope::WholeDocument => (1, ctx.line_count()),
        VimSubstituteScope::CurrentLine if mode == CommandMode::Vim && !selection.empty => {
            let anchor_line = ctx.line_at(selection.anchor).number;
            let head_line = ctx.line_at(selection.head).number;
            (anchor_line.min(head_line), anchor_line.max(head_line))
        }
        VimSubstituteScope::CurrentLine => {
            let current_line = ctx.line_at(selection.head).number;
            (current_line, current_line)
        }
    };

    let from = ctx.line(start_line).from;
    let to = ctx.line(end_line).to;
    let mut replacements = 0usize;
    let mut changed_lines = 0usize;
    let mut replaced_lines = Vec::with_capacity(end_line.saturating_sub(start_line) + 1);
    for line_no in start_line..=end_line {
        let line_text = ctx.line_text(line_no);
        let count_for_line = if command.flags.global {
            matcher.find_iter(line_text).count()
        } else if matcher.find(line_text).is_some() {
            1
        } else {
            0
        };
        if count_for_line == 0 {
            replaced_lines.push(line_text.to_string());
            continue;
        }

        let replaced = if command.flags.global {
            matcher
                .replace_all(line_text, command.replacement.as_str())
                .to_string()
        } else {
            matcher
                .replacen(line_text, 1, command.replacement.as_str())
                .to_string()
        };
        if replaced != line_text {
            changed_lines += 1;
        }
        replacements += count_for_line;
        replaced_lines.push(replaced);
    }

    if replacements == 0 {
        return result_with_message(format!(
            "substitute: pattern not found: {}",
            command.pattern
        ));
    }

    let op = replace_range(from, to, replaced_lines.join("\n"), None);
    let mut result = result_with_message(format!(
        "{replacements} substitution{} on {changed_lines} line{}",
        if replacements == 1 { "" } else { "s" },
        if changed_lines == 1 { "" } else { "s" }
    ));
    result.operations.push(op);
    result
}

pub fn try_execute_vim_substitute(
    snapshot: &EditorContextSnapshot,
    raw_input: &str,
    mode: CommandMode,
) -> Option<CommandExecutionResult> {
    if mode != CommandMode::Vim {
        return None;
    }
    match parse_vim_substitute_command(raw_input) {
        Ok(Some(command)) => Some(apply_vim_substitute(snapshot, mode, &command)),
        Ok(None) => None,
        Err(error) => Some(result_with_message(format!("substitute: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SelectionSnapshot, TextRange};

    fn snapshot(text: &str, anchor: usize, head: usize) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: Some(TextRange {
                from: anchor,
                to: head,
            }),
        }
    }

    #[test]
    fn parse_returns_none_for_regular_commands() {
        assert_eq!(parse_vim_substitute_command("sum").expect("parse"), None);
    }

    #[test]
    fn parse_supports_flags() {
        let parsed = parse_vim_substitute_command("%s/alpha/beta/gi")
            .expect("parse")
            .expect("command");
        assert_eq!(parsed.scope, VimSubstituteScope::WholeDocument);
        assert!(parsed.flags.global);
        assert!(parsed.flags.ignore_case);
    }

    #[test]
    fn execute_substitute_current_line() {
        let input = snapshot("alpha beta\nalpha", 0, 0);
        let result = try_execute_vim_substitute(&input, "s/alpha/omega/", CommandMode::Vim)
            .expect("substitute result");
        assert!(result.message.contains("substitution"));
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes[0].from, 0);
        assert_eq!(op.changes[0].to, 10);
        assert_eq!(op.changes[0].insert, "omega beta");
    }
}
