use crate::operations::replace_range;
use crate::types::{EditOperation, OperationSelection, SelectionSnapshot};
use crate::vim::VimIntent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimRegisterMode {
    Charwise,
    Linewise,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VimRegisterValue {
    pub text: String,
    pub mode: VimRegisterMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VimActionExecutionResult {
    #[serde(default)]
    pub operations: Vec<EditOperation>,
    #[serde(default)]
    pub register: Option<VimRegisterValue>,
}

impl VimRegisterValue {
    fn is_empty_charwise(&self) -> bool {
        self.mode == VimRegisterMode::Charwise && self.text.is_empty()
    }
}

pub fn execute_vim_action(
    text: &str,
    selection: SelectionSnapshot,
    intent: VimIntent,
    count: usize,
    register: Option<&VimRegisterValue>,
) -> Option<VimActionExecutionResult> {
    execute_vim_action_with_target(text, selection, intent, count, register, None)
}

pub fn execute_vim_action_with_target(
    text: &str,
    selection: SelectionSnapshot,
    intent: VimIntent,
    count: usize,
    register: Option<&VimRegisterValue>,
    target_char: Option<char>,
) -> Option<VimActionExecutionResult> {
    if !supports_intent(intent) {
        return None;
    }

    let repeats = count.max(1);
    match intent {
        VimIntent::DeleteLine => Some(execute_delete_line(text, selection, repeats)),
        VimIntent::YankLine => Some(execute_yank_line(text, selection, repeats)),
        VimIntent::DeleteToLineStart => Some(execute_delete_to_line_start(text, selection)),
        VimIntent::DeleteToLineEnd => Some(execute_delete_to_line_end(text, selection)),
        VimIntent::YankToLineStart => Some(execute_yank_to_line_start(text, selection)),
        VimIntent::YankToLineEnd => Some(execute_yank_to_line_end(text, selection)),
        VimIntent::DeleteWordForward => Some(execute_delete_word_forward(text, selection, repeats)),
        VimIntent::DeleteWordBackward => {
            Some(execute_delete_word_backward(text, selection, repeats))
        }
        VimIntent::DeleteWordEnd => Some(execute_delete_word_end(text, selection, repeats)),
        VimIntent::DeleteInsideWord => Some(execute_word_text_object(
            text, selection, false, true, repeats,
        )),
        VimIntent::DeleteAroundWord => Some(execute_word_text_object(
            text, selection, true, true, repeats,
        )),
        VimIntent::YankInsideWord => Some(execute_word_text_object(
            text, selection, false, false, repeats,
        )),
        VimIntent::YankAroundWord => Some(execute_word_text_object(
            text, selection, true, false, repeats,
        )),
        VimIntent::DeleteInsidePipe => Some(execute_pipe_text_object(
            text, selection, false, true, repeats,
        )),
        VimIntent::DeleteAroundPipe => Some(execute_pipe_text_object(
            text, selection, true, true, repeats,
        )),
        VimIntent::YankInsidePipe => Some(execute_pipe_text_object(
            text, selection, false, false, repeats,
        )),
        VimIntent::YankAroundPipe => Some(execute_pipe_text_object(
            text, selection, true, false, repeats,
        )),
        VimIntent::DeleteInsideParen => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Paren,
            repeats,
        )),
        VimIntent::DeleteInsideBracket => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Bracket,
            repeats,
        )),
        VimIntent::DeleteInsideBrace => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Brace,
            repeats,
        )),
        VimIntent::DeleteInsideDoubleQuote => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::DoubleQuote,
            repeats,
        )),
        VimIntent::DeleteInsideBacktick => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Backtick,
            repeats,
        )),
        VimIntent::DeleteInsideAsterisk => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Asterisk,
            repeats,
        )),
        VimIntent::DeleteInsideTilde => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Tilde,
            repeats,
        )),
        VimIntent::DeleteInsideUnderscore => Some(execute_delimited_inside_text_object(
            text,
            selection,
            DelimitedObject::Underscore,
            repeats,
        )),
        VimIntent::DeleteAroundParen => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Paren,
            repeats,
        )),
        VimIntent::DeleteAroundBracket => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Bracket,
            repeats,
        )),
        VimIntent::DeleteAroundBrace => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Brace,
            repeats,
        )),
        VimIntent::DeleteAroundDoubleQuote => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::DoubleQuote,
            repeats,
        )),
        VimIntent::DeleteAroundBacktick => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Backtick,
            repeats,
        )),
        VimIntent::DeleteAroundAsterisk => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Asterisk,
            repeats,
        )),
        VimIntent::DeleteAroundTilde => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Tilde,
            repeats,
        )),
        VimIntent::DeleteAroundUnderscore => Some(execute_delimited_around_text_object(
            text,
            selection,
            DelimitedObject::Underscore,
            repeats,
        )),
        VimIntent::YankWordForward => Some(execute_yank_word_forward(text, selection, repeats)),
        VimIntent::YankWordBackward => Some(execute_yank_word_backward(text, selection, repeats)),
        VimIntent::PasteAfter => execute_paste_after(text, selection, repeats, register),
        VimIntent::DeleteChar => Some(execute_delete_char(text, selection, repeats)),
        VimIntent::DeleteTillChar => Some(execute_delete_till_char(
            text,
            selection,
            repeats,
            target_char,
        )),
        _ => None,
    }
}

pub fn supports_intent(intent: VimIntent) -> bool {
    matches!(
        intent,
        VimIntent::DeleteLine
            | VimIntent::YankLine
            | VimIntent::DeleteToLineStart
            | VimIntent::DeleteToLineEnd
            | VimIntent::YankToLineStart
            | VimIntent::YankToLineEnd
            | VimIntent::DeleteWordForward
            | VimIntent::DeleteWordBackward
            | VimIntent::DeleteWordEnd
            | VimIntent::DeleteInsideWord
            | VimIntent::DeleteAroundWord
            | VimIntent::YankInsideWord
            | VimIntent::YankAroundWord
            | VimIntent::DeleteInsidePipe
            | VimIntent::DeleteAroundPipe
            | VimIntent::YankInsidePipe
            | VimIntent::YankAroundPipe
            | VimIntent::DeleteInsideParen
            | VimIntent::DeleteInsideBracket
            | VimIntent::DeleteInsideBrace
            | VimIntent::DeleteInsideDoubleQuote
            | VimIntent::DeleteInsideBacktick
            | VimIntent::DeleteInsideAsterisk
            | VimIntent::DeleteInsideTilde
            | VimIntent::DeleteInsideUnderscore
            | VimIntent::DeleteAroundParen
            | VimIntent::DeleteAroundBracket
            | VimIntent::DeleteAroundBrace
            | VimIntent::DeleteAroundDoubleQuote
            | VimIntent::DeleteAroundBacktick
            | VimIntent::DeleteAroundAsterisk
            | VimIntent::DeleteAroundTilde
            | VimIntent::DeleteAroundUnderscore
            | VimIntent::YankWordForward
            | VimIntent::YankWordBackward
            | VimIntent::PasteAfter
            | VimIntent::DeleteChar
            | VimIntent::DeleteTillChar
    )
}

fn execute_delete_line(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let cursor = clamp_offset(text, selection.head);
    let start_idx = line_index_for_offset(&spans, cursor);
    let end_idx = (start_idx + count.saturating_sub(1)).min(spans.len().saturating_sub(1));

    let mut from = spans[start_idx].0;
    let mut to = spans[end_idx].1;
    if end_idx + 1 < spans.len() {
        to += 1;
    } else if start_idx > 0 {
        from = spans[start_idx - 1].1;
    }

    let mut deleted = Vec::with_capacity(end_idx - start_idx + 1);
    for (line_from, line_to) in spans
        .iter()
        .skip(start_idx)
        .take(end_idx - start_idx + 1)
        .copied()
    {
        deleted.push(text[line_from..line_to].to_string());
    }

    let operations = if from < to {
        vec![replace_range(
            from,
            to,
            "",
            Some(OperationSelection {
                anchor: from,
                head: None,
            }),
        )]
    } else {
        Vec::new()
    };

    VimActionExecutionResult {
        operations,
        register: Some(VimRegisterValue {
            text: deleted.join("\n"),
            mode: VimRegisterMode::Linewise,
        }),
    }
}

fn execute_yank_line(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let cursor = clamp_offset(text, selection.head);
    let start_idx = line_index_for_offset(&spans, cursor);
    let end_idx = (start_idx + count.saturating_sub(1)).min(spans.len().saturating_sub(1));

    let mut yanked = Vec::with_capacity(end_idx - start_idx + 1);
    for (line_from, line_to) in spans
        .iter()
        .skip(start_idx)
        .take(end_idx - start_idx + 1)
        .copied()
    {
        yanked.push(text[line_from..line_to].to_string());
    }

    VimActionExecutionResult {
        operations: Vec::new(),
        register: Some(VimRegisterValue {
            text: yanked.join("\n"),
            mode: VimRegisterMode::Linewise,
        }),
    }
}

fn execute_delete_to_line_start(
    text: &str,
    selection: SelectionSnapshot,
) -> VimActionExecutionResult {
    let cursor = clamp_offset(text, selection.head);
    let spans = line_spans(text);
    let idx = line_index_for_offset(&spans, cursor);
    let line_from = spans[idx].0;
    if cursor <= line_from {
        return VimActionExecutionResult::default();
    }

    let deleted = text[line_from..cursor].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            line_from,
            cursor,
            "",
            Some(OperationSelection {
                anchor: line_from,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_delete_to_line_end(
    text: &str,
    selection: SelectionSnapshot,
) -> VimActionExecutionResult {
    let cursor = clamp_offset(text, selection.head);
    let spans = line_spans(text);
    let idx = line_index_for_offset(&spans, cursor);
    let line_to = spans[idx].1;
    if cursor >= line_to {
        return VimActionExecutionResult::default();
    }

    let deleted = text[cursor..line_to].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            cursor,
            line_to,
            "",
            Some(OperationSelection {
                anchor: cursor,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_yank_to_line_start(
    text: &str,
    selection: SelectionSnapshot,
) -> VimActionExecutionResult {
    let cursor = clamp_offset(text, selection.head);
    let spans = line_spans(text);
    let idx = line_index_for_offset(&spans, cursor);
    let line_from = spans[idx].0;
    if cursor <= line_from {
        return VimActionExecutionResult::default();
    }

    VimActionExecutionResult {
        operations: Vec::new(),
        register: Some(VimRegisterValue {
            text: text[line_from..cursor].to_string(),
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_yank_to_line_end(text: &str, selection: SelectionSnapshot) -> VimActionExecutionResult {
    let cursor = clamp_offset(text, selection.head);
    let spans = line_spans(text);
    let idx = line_index_for_offset(&spans, cursor);
    let line_to = spans[idx].1;
    if cursor >= line_to {
        return VimActionExecutionResult::default();
    }

    VimActionExecutionResult {
        operations: Vec::new(),
        register: Some(VimRegisterValue {
            text: text[cursor..line_to].to_string(),
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_yank_word_forward(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let origin = clamp_offset(text, selection.head);
    let mut cursor = origin;
    let mut yanked = Vec::new();
    for _ in 0..count {
        let next = move_word_forward(text, &spans, cursor);
        if next == cursor {
            break;
        }
        let from = cursor.min(next);
        let to = cursor.max(next);
        yanked.push(text[from..to].to_string());
        cursor = next;
    }

    if yanked.is_empty() {
        return VimActionExecutionResult::default();
    }

    VimActionExecutionResult {
        operations: Vec::new(),
        register: Some(VimRegisterValue {
            text: yanked.join(""),
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_yank_word_backward(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let origin = clamp_offset(text, selection.head);
    let mut cursor = origin;
    for _ in 0..count {
        let next = move_word_backward(text, &spans, cursor);
        if next == cursor {
            break;
        }
        cursor = next;
    }

    if cursor == origin {
        return VimActionExecutionResult::default();
    }

    let from = cursor.min(origin);
    let to = cursor.max(origin);
    VimActionExecutionResult {
        operations: Vec::new(),
        register: Some(VimRegisterValue {
            text: text[from..to].to_string(),
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_delete_word_forward(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let origin = clamp_offset(text, selection.head);
    let mut cursor = origin;
    for _ in 0..count {
        let next = move_word_forward(text, &spans, cursor);
        if next == cursor {
            break;
        }
        cursor = next;
    }

    if cursor == origin {
        return VimActionExecutionResult::default();
    }

    let from = origin.min(cursor);
    let to = origin.max(cursor);
    let deleted = text[from..to].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            from,
            to,
            "",
            Some(OperationSelection {
                anchor: from,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_delete_word_backward(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let spans = line_spans(text);
    let origin = clamp_offset(text, selection.head);
    let mut cursor = origin;
    for _ in 0..count {
        let next = move_word_backward(text, &spans, cursor);
        if next == cursor {
            break;
        }
        cursor = next;
    }

    if cursor == origin {
        return VimActionExecutionResult::default();
    }

    let from = cursor.min(origin);
    let to = cursor.max(origin);
    let deleted = text[from..to].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            from,
            to,
            "",
            Some(OperationSelection {
                anchor: from,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_delete_word_end(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let origin = clamp_offset(text, selection.head);
    let mut cursor = origin;
    let mut resolved = false;
    for _ in 0..count {
        let Some(next) = move_word_end(text, cursor) else {
            break;
        };
        cursor = next;
        resolved = true;
    }

    if !resolved {
        return VimActionExecutionResult::default();
    }

    let delete_to = next_char_boundary(text, cursor);
    if delete_to <= origin {
        return VimActionExecutionResult::default();
    }

    let deleted = text[origin..delete_to].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            origin,
            delete_to,
            "",
            Some(OperationSelection {
                anchor: origin,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_delete_till_char(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
    target_char: Option<char>,
) -> VimActionExecutionResult {
    let Some(target_char) = target_char else {
        return VimActionExecutionResult::default();
    };
    let cursor = clamp_offset(text, selection.head);
    let spans = line_spans(text);
    let idx = line_index_for_offset(&spans, cursor);
    let (_line_from, line_to) = spans[idx];
    if cursor >= line_to {
        return VimActionExecutionResult::default();
    }

    let mut search = cursor;
    let mut match_at: Option<usize> = None;
    for _ in 0..count.max(1) {
        let Some(next) = find_next_char_in_line(text, search, line_to, target_char) else {
            return VimActionExecutionResult::default();
        };
        match_at = Some(next);
        let after = next_char_boundary(text, next);
        if after <= next {
            break;
        }
        search = after.min(line_to);
    }

    let delete_to = match_at.unwrap_or(cursor);
    if delete_to <= cursor {
        return VimActionExecutionResult::default();
    }
    let deleted = text[cursor..delete_to].to_string();
    VimActionExecutionResult {
        operations: vec![replace_range(
            cursor,
            delete_to,
            "",
            Some(OperationSelection {
                anchor: cursor,
                head: None,
            }),
        )],
        register: Some(VimRegisterValue {
            text: deleted,
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_word_text_object(
    text: &str,
    selection: SelectionSnapshot,
    around: bool,
    delete: bool,
    count: usize,
) -> VimActionExecutionResult {
    execute_text_object(text, selection, count, delete, |line, cursor_col| {
        find_word_object_bounds(line, cursor_col, around)
    })
}

fn execute_pipe_text_object(
    text: &str,
    selection: SelectionSnapshot,
    around: bool,
    delete: bool,
    count: usize,
) -> VimActionExecutionResult {
    execute_text_object(text, selection, count, delete, |line, cursor_col| {
        find_pipe_object_bounds(line, cursor_col, around)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelimitedObject {
    Paren,
    Bracket,
    Brace,
    DoubleQuote,
    Backtick,
    Asterisk,
    Tilde,
    Underscore,
}

fn execute_delimited_inside_text_object(
    text: &str,
    selection: SelectionSnapshot,
    kind: DelimitedObject,
    count: usize,
) -> VimActionExecutionResult {
    execute_text_object(text, selection, count, true, |line, cursor_col| {
        find_delimited_object_bounds(line, cursor_col, kind, false)
    })
}

fn execute_delimited_around_text_object(
    text: &str,
    selection: SelectionSnapshot,
    kind: DelimitedObject,
    count: usize,
) -> VimActionExecutionResult {
    execute_text_object(text, selection, count, true, |line, cursor_col| {
        find_delimited_object_bounds(line, cursor_col, kind, true)
    })
}

fn execute_text_object<F>(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
    delete: bool,
    mut resolve_bounds: F,
) -> VimActionExecutionResult
where
    F: FnMut(&str, usize) -> Option<(usize, usize)>,
{
    let mut text = text.to_string();
    let mut cursor = clamp_offset(&text, selection.head);
    let mut chunks = Vec::new();
    let mut operations = Vec::new();

    for _ in 0..count.max(1) {
        let spans = line_spans(&text);
        let idx = line_index_for_offset(&spans, cursor);
        let (line_from, line_to) = spans[idx];
        let line = &text[line_from..line_to];

        let cursor_in_line = cursor.clamp(line_from, line_to) - line_from;
        let cursor_col = line[..cursor_in_line].chars().count();
        let Some((start_col, end_col)) = resolve_bounds(line, cursor_col) else {
            break;
        };
        let from = line_from + char_to_byte_offset(line, start_col);
        let to = line_from + char_to_byte_offset(line, end_col);
        if from >= to || to > text.len() {
            break;
        }

        let chunk = text[from..to].to_string();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);

        if delete {
            operations.push(replace_range(
                from,
                to,
                "",
                Some(OperationSelection {
                    anchor: from,
                    head: None,
                }),
            ));
            text.replace_range(from..to, "");
            cursor = from.min(text.len());
        } else {
            cursor = to.min(text.len());
        }
    }

    if chunks.is_empty() {
        return VimActionExecutionResult::default();
    }

    VimActionExecutionResult {
        operations,
        register: Some(VimRegisterValue {
            text: chunks.join("\n"),
            mode: VimRegisterMode::Charwise,
        }),
    }
}

fn execute_paste_after(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
    register: Option<&VimRegisterValue>,
) -> Option<VimActionExecutionResult> {
    let register = register?;
    if register.is_empty_charwise() {
        return None;
    }

    match register.mode {
        VimRegisterMode::Linewise => {
            let spans = line_spans(text);
            let cursor = clamp_offset(text, selection.head);
            let idx = line_index_for_offset(&spans, cursor);
            let insert_at = spans[idx].1;
            let normalized = register
                .text
                .strip_suffix('\n')
                .unwrap_or(register.text.as_str());
            let lines = if normalized.is_empty() {
                vec![String::new()]
            } else {
                normalized
                    .split('\n')
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
            };
            let mut repeated = Vec::with_capacity(lines.len() * count);
            for _ in 0..count {
                repeated.extend(lines.iter().cloned());
            }
            if repeated.is_empty() {
                return Some(VimActionExecutionResult::default());
            }
            let insert = format!("\n{}", repeated.join("\n"));
            Some(VimActionExecutionResult {
                operations: vec![replace_range(
                    insert_at,
                    insert_at,
                    insert,
                    Some(OperationSelection {
                        anchor: insert_at + 1,
                        head: None,
                    }),
                )],
                register: None,
            })
        }
        VimRegisterMode::Charwise => {
            let mut text = text.to_string();
            let mut cursor = clamp_offset(&text, selection.head);
            let mut operations = Vec::new();
            for _ in 0..count {
                let spans = line_spans(&text);
                let idx = line_index_for_offset(&spans, cursor);
                let line_to = spans[idx].1;
                let insert_at = if cursor < line_to {
                    next_char_boundary(&text, cursor)
                } else {
                    line_to
                };
                let anchor = insert_at + register.text.len();
                let operation = replace_range(
                    insert_at,
                    insert_at,
                    register.text.clone(),
                    Some(OperationSelection { anchor, head: None }),
                );
                text.insert_str(insert_at, register.text.as_str());
                cursor = anchor.min(text.len());
                operations.push(operation);
            }
            Some(VimActionExecutionResult {
                operations,
                register: None,
            })
        }
    }
}

fn execute_delete_char(
    text: &str,
    selection: SelectionSnapshot,
    count: usize,
) -> VimActionExecutionResult {
    let mut text = text.to_string();
    let mut cursor = clamp_offset(&text, selection.head);
    let mut operations = Vec::new();

    for _ in 0..count {
        let spans = line_spans(&text);
        let idx = line_index_for_offset(&spans, cursor);
        let line_to = spans[idx].1;
        if cursor < line_to {
            let next = next_char_boundary(&text, cursor);
            if next <= cursor {
                break;
            }
            operations.push(replace_range(
                cursor,
                next,
                "",
                Some(OperationSelection {
                    anchor: cursor,
                    head: None,
                }),
            ));
            text.replace_range(cursor..next, "");
            continue;
        }

        if idx + 1 >= spans.len() || line_to >= text.len() {
            break;
        }

        operations.push(replace_range(
            line_to,
            line_to + 1,
            "",
            Some(OperationSelection {
                anchor: cursor,
                head: None,
            }),
        ));
        text.replace_range(line_to..line_to + 1, "");
        cursor = cursor.min(text.len());
    }

    VimActionExecutionResult {
        operations,
        register: None,
    }
}

fn clamp_offset(text: &str, offset: usize) -> usize {
    offset.min(text.len())
}

fn line_spans(text: &str) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }

    let mut spans = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            spans.push((start, idx));
            start = idx + 1;
        }
    }
    spans.push((start, text.len()));
    spans
}

fn line_index_for_offset(spans: &[(usize, usize)], offset: usize) -> usize {
    if spans.is_empty() {
        return 0;
    }
    let mut lo = 0usize;
    let mut hi = spans.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if spans[mid].0 <= offset {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo.min(spans.len().saturating_sub(1))
}

fn next_char_boundary(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    let mut iter = text[offset..].chars();
    if let Some(ch) = iter.next() {
        offset + ch.len_utf8()
    } else {
        text.len()
    }
}

fn prev_char_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 || text.is_empty() {
        return 0;
    }
    let mut cursor = offset.min(text.len());
    while cursor > 0 {
        cursor -= 1;
        if text.is_char_boundary(cursor) {
            return cursor;
        }
    }
    0
}

fn char_at(text: &str, offset: usize) -> Option<char> {
    if offset >= text.len() {
        None
    } else {
        text[offset..].chars().next()
    }
}

fn find_next_char_in_line(text: &str, from: usize, line_to: usize, target: char) -> Option<usize> {
    let mut cursor = from.min(text.len()).min(line_to);
    while cursor < line_to {
        let ch = char_at(text, cursor)?;
        if ch == target {
            return Some(cursor);
        }
        let next = next_char_boundary(text, cursor);
        if next <= cursor {
            break;
        }
        cursor = next.min(line_to);
    }
    None
}

fn char_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

fn move_word_forward(text: &str, spans: &[(usize, usize)], offset: usize) -> usize {
    if spans.is_empty() {
        return 0;
    }
    let cursor = clamp_offset(text, offset);
    let idx = line_index_for_offset(spans, cursor);
    let (line_from, line_to) = spans[idx];

    if cursor >= line_to {
        if idx + 1 < spans.len() {
            return spans[idx + 1].0;
        }
        return line_to;
    }

    let mut col = cursor.max(line_from).min(line_to);
    let start_class = char_at(text, col).map(char_class).unwrap_or(0);
    while col < line_to {
        let current_class = char_at(text, col).map(char_class).unwrap_or(0);
        if current_class != start_class {
            break;
        }
        let next = next_char_boundary(text, col);
        if next <= col {
            break;
        }
        col = next.min(line_to);
    }
    if start_class != 0 {
        while col < line_to {
            let current = char_at(text, col);
            if !current.is_some_and(char::is_whitespace) {
                break;
            }
            let next = next_char_boundary(text, col);
            if next <= col {
                break;
            }
            col = next.min(line_to);
        }
    }
    col
}

fn move_word_backward(text: &str, spans: &[(usize, usize)], offset: usize) -> usize {
    if spans.is_empty() {
        return 0;
    }
    let cursor = clamp_offset(text, offset);
    let idx = line_index_for_offset(spans, cursor);
    let (line_from, line_to) = spans[idx];

    if cursor <= line_from {
        if idx > 0 {
            return spans[idx - 1].1;
        }
        return line_from;
    }

    let mut col = cursor.min(line_to);
    col = prev_char_boundary(text, col);
    while col > line_from {
        let current = char_at(text, col);
        if !current.is_some_and(char::is_whitespace) {
            break;
        }
        let prev = prev_char_boundary(text, col);
        if prev == col {
            break;
        }
        col = prev;
    }

    let target_class = char_at(text, col).map(char_class).unwrap_or(0);
    while col > line_from {
        let prev = prev_char_boundary(text, col);
        if char_at(text, prev).map(char_class).unwrap_or(0) == target_class {
            col = prev;
        } else {
            break;
        }
    }
    col
}

fn run_end_by_class(text: &str, start: usize, class: u8) -> usize {
    let mut cursor = start;
    while cursor < text.len() {
        let next = next_char_boundary(text, cursor);
        if next >= text.len() {
            break;
        }
        if char_at(text, next).map(char_class).unwrap_or(255) != class {
            break;
        }
        cursor = next;
    }
    cursor
}

fn find_next_non_whitespace(text: &str, start: usize) -> Option<usize> {
    let mut cursor = start.min(text.len());
    while cursor < text.len() {
        if char_at(text, cursor).is_some_and(|ch| !ch.is_whitespace()) {
            return Some(cursor);
        }
        let next = next_char_boundary(text, cursor);
        if next <= cursor {
            break;
        }
        cursor = next;
    }
    None
}

fn move_word_end(text: &str, offset: usize) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    let cursor = clamp_offset(text, offset);
    if cursor >= text.len() {
        return None;
    }
    let current = char_at(text, cursor)?;
    let class = char_class(current);

    if class != 0 {
        let next = next_char_boundary(text, cursor);
        if next < text.len() && char_at(text, next).map(char_class) == Some(class) {
            return Some(run_end_by_class(text, cursor, class));
        }
        if let Some(start) = find_next_non_whitespace(text, next) {
            let next_class = char_at(text, start).map(char_class).unwrap_or(0);
            return Some(run_end_by_class(text, start, next_class));
        }
        return Some(cursor);
    }

    if let Some(start) = find_next_non_whitespace(text, cursor) {
        let next_class = char_at(text, start).map(char_class).unwrap_or(0);
        return Some(run_end_by_class(text, start, next_class));
    }
    Some(run_end_by_class(text, cursor, 0))
}

fn char_to_byte_offset(line: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    line.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len())
}

fn find_word_object_bounds(line: &str, cursor_col: usize, around: bool) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if len == 0 {
        return None;
    }
    let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';

    let mut rel = cursor_col.min(len);
    if rel >= len {
        rel = len - 1;
    }

    if !is_word_char(chars[rel]) {
        if rel > 0 && is_word_char(chars[rel - 1]) {
            rel -= 1;
        } else {
            while rel < len && !is_word_char(chars[rel]) {
                rel += 1;
            }
            if rel >= len {
                return None;
            }
        }
    }

    let mut start = rel;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = rel + 1;
    while end < len && is_word_char(chars[end]) {
        end += 1;
    }

    if around {
        let mut around_start = start;
        let mut around_end = end;
        while around_end < len && chars[around_end].is_whitespace() {
            around_end += 1;
        }
        if around_end == end {
            while around_start > 0 && chars[around_start - 1].is_whitespace() {
                around_start -= 1;
            }
        }
        start = around_start;
        end = around_end;
    }

    if start >= end {
        None
    } else {
        Some((start, end))
    }
}

fn find_pipe_object_bounds(line: &str, cursor_col: usize, around: bool) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let pipes: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter_map(|(idx, ch)| if *ch == '|' { Some(idx) } else { None })
        .collect();
    if pipes.len() < 2 {
        return None;
    }

    let cursor = cursor_col.min(chars.len());
    let mut pair = None;
    for window in pipes.windows(2) {
        let left = window[0];
        let right = window[1];
        if cursor == left || (cursor > left && cursor <= right) {
            pair = Some((left, right));
            break;
        }
    }
    let (left, right) = pair?;
    let (start, end) = if around {
        (left, right + 1)
    } else {
        let mut inner_start = (left + 1).min(right);
        let mut inner_end = right;
        while inner_start < right && chars[inner_start].is_whitespace() {
            inner_start += 1;
        }
        while inner_end > inner_start && chars[inner_end - 1].is_whitespace() {
            inner_end -= 1;
        }
        (inner_start, inner_end)
    };
    if start >= end {
        None
    } else {
        Some((start, end))
    }
}

fn find_delimited_object_bounds(
    line: &str,
    cursor_col: usize,
    kind: DelimitedObject,
    around: bool,
) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let cursor = cursor_col.min(chars.len().saturating_sub(1));

    let pair_opt = match kind {
        DelimitedObject::Paren => find_balanced_pair_around_cursor(&chars, cursor, '(', ')'),
        DelimitedObject::Bracket => find_balanced_pair_around_cursor(&chars, cursor, '[', ']'),
        DelimitedObject::Brace => find_balanced_pair_around_cursor(&chars, cursor, '{', '}'),
        DelimitedObject::DoubleQuote => {
            find_same_delimiter_pair_around_cursor(&chars, cursor, '"')
        }
        DelimitedObject::Backtick => {
            find_same_delimiter_pair_around_cursor(&chars, cursor, '`')
        }
        DelimitedObject::Asterisk => {
            find_same_delimiter_pair_around_cursor(&chars, cursor, '*')
        }
        DelimitedObject::Tilde => find_same_delimiter_pair_around_cursor(&chars, cursor, '~'),
        DelimitedObject::Underscore => {
            find_same_delimiter_pair_around_cursor(&chars, cursor, '_')
        }
    };
    let (start, end) = pair_opt?;
    let is_run = matches!(
        kind,
        DelimitedObject::Asterisk | DelimitedObject::Tilde | DelimitedObject::Underscore
    );
    let (open_start, open_end_exclusive, close_start, close_end_exclusive) = if is_run {
        let (open_run_start, open_run_end_exclusive) =
            delimiter_run_bounds(chars.as_slice(), start);
        let (close_run_start, close_run_end_exclusive) =
            delimiter_run_bounds(chars.as_slice(), end);
        (
            open_run_start,
            open_run_end_exclusive,
            close_run_start,
            close_run_end_exclusive,
        )
    } else {
        (start, start + 1, end, end + 1)
    };
    if around {
        (open_start < close_end_exclusive).then_some((open_start, close_end_exclusive))
    } else {
        (open_end_exclusive < close_start).then_some((open_end_exclusive, close_start))
    }
}

fn find_balanced_pair_around_cursor(
    chars: &[char],
    cursor: usize,
    open: char,
    close: char,
) -> Option<(usize, usize)> {
    for start in (0..=cursor).rev() {
        if chars[start] != open {
            continue;
        }
        let mut depth = 1usize;
        let mut end = None;
        for (idx, ch) in chars.iter().enumerate().skip(start + 1) {
            if *ch == open {
                depth += 1;
            } else if *ch == close {
                depth -= 1;
                if depth == 0 {
                    end = Some(idx);
                    break;
                }
            }
        }
        if let Some(end) = end {
            if cursor == start || (cursor > start && cursor <= end) {
                return Some((start, end));
            }
        }
    }
    None
}

fn find_same_delimiter_pair_around_cursor(
    chars: &[char],
    cursor: usize,
    delimiter: char,
) -> Option<(usize, usize)> {
    for start in (0..=cursor).rev() {
        if chars[start] != delimiter {
            continue;
        }
        let (_, start_run_end_exclusive) = delimiter_run_bounds(chars, start);
        let mut end = None;
        for idx in start_run_end_exclusive..chars.len() {
            if chars[idx] == delimiter {
                end = Some(idx);
                break;
            }
        }
        if let Some(end) = end {
            let (_, end_run_end_exclusive) = delimiter_run_bounds(chars, end);
            if cursor == start || (cursor > start && cursor < end_run_end_exclusive) {
                return Some((start, end));
            }
        }
    }
    None
}

fn delimiter_run_bounds(chars: &[char], idx: usize) -> (usize, usize) {
    let delimiter = chars[idx];
    let mut start = idx;
    while start > 0 && chars[start - 1] == delimiter {
        start -= 1;
    }
    let mut end_exclusive = idx + 1;
    while end_exclusive < chars.len() && chars[end_exclusive] == delimiter {
        end_exclusive += 1;
    }
    (start, end_exclusive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SelectionSnapshot;
    use crate::vim::{self, VimIntent, VimMode, VimState};

    fn sel(cursor: usize) -> SelectionSnapshot {
        SelectionSnapshot {
            anchor: cursor,
            head: cursor,
        }
    }

    fn apply_operations(mut text: String, operations: &[EditOperation]) -> String {
        for operation in operations {
            let mut changes = operation.changes.clone();
            changes.sort_by(|a, b| b.from.cmp(&a.from).then_with(|| b.to.cmp(&a.to)));
            for change in changes {
                text.replace_range(change.from..change.to, &change.insert);
            }
        }
        text
    }

    fn charwise_register(result: &VimActionExecutionResult) -> Option<&str> {
        result
            .register
            .as_ref()
            .filter(|reg| reg.mode == VimRegisterMode::Charwise)
            .map(|reg| reg.text.as_str())
    }

    fn linewise_register(result: &VimActionExecutionResult) -> Option<&str> {
        result
            .register
            .as_ref()
            .filter(|reg| reg.mode == VimRegisterMode::Linewise)
            .map(|reg| reg.text.as_str())
    }

    #[test]
    fn supports_intent_only_for_shared_edit_operations() {
        assert!(supports_intent(VimIntent::DeleteLine));
        assert!(supports_intent(VimIntent::PasteAfter));
        assert!(!supports_intent(VimIntent::MoveDown));
        assert!(!supports_intent(VimIntent::MoveUp));
        assert!(!supports_intent(VimIntent::MoveWordForward));
    }

    #[test]
    fn delete_line_removes_selected_line_and_sets_linewise_register() {
        let text = "alpha\nbeta\ngamma";
        let cursor = text.find("beta").expect("beta");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteLine, 1, None).expect("handled");
        assert_eq!(linewise_register(&result), Some("beta"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "alpha\ngamma");
    }

    #[test]
    fn yank_line_sets_linewise_register_without_edit() {
        let result = execute_vim_action("alpha\nbeta\ngamma", sel(0), VimIntent::YankLine, 2, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(linewise_register(&result), Some("alpha\nbeta"));
    }

    #[test]
    fn delete_to_line_end_sets_register_and_deletes_text() {
        let text = "hello world";
        let cursor = text.find("world").expect("cursor");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteToLineEnd, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("world"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "hello ");
    }

    #[test]
    fn yank_to_line_start_sets_register_without_edit() {
        let text = "hello world";
        let cursor = text.find("world").expect("cursor");
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankToLineStart, 1, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("hello "));
    }

    #[test]
    fn yank_to_line_end_sets_register_without_edit() {
        let text = "hello world";
        let cursor = text.find("world").expect("cursor");
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankToLineEnd, 1, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("world"));
    }

    #[test]
    fn yank_word_forward_collects_next_word_chunk() {
        let text = "alpha   beta";
        let cursor = text.find("alpha").expect("cursor");
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankWordForward, 1, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("alpha   "));
    }

    #[test]
    fn paste_after_charwise_inserts_after_cursor() {
        let register = VimRegisterValue {
            text: "Z".to_string(),
            mode: VimRegisterMode::Charwise,
        };
        let result = execute_vim_action("abc", sel(0), VimIntent::PasteAfter, 1, Some(&register))
            .expect("handled");
        let next = apply_operations("abc".to_string(), &result.operations);
        assert_eq!(next, "aZbc");
    }

    #[test]
    fn paste_after_linewise_inserts_lines_below_current_line() {
        let register = VimRegisterValue {
            text: "A\nB".to_string(),
            mode: VimRegisterMode::Linewise,
        };
        let result = execute_vim_action(
            "one\ntwo",
            sel(0),
            VimIntent::PasteAfter,
            1,
            Some(&register),
        )
        .expect("handled");
        let next = apply_operations("one\ntwo".to_string(), &result.operations);
        assert_eq!(next, "one\nA\nB\ntwo");
    }

    #[test]
    fn delete_char_removes_char_or_joins_next_line() {
        let result =
            execute_vim_action("ab\ncd", sel(1), VimIntent::DeleteChar, 2, None).expect("handled");
        let next = apply_operations("ab\ncd".to_string(), &result.operations);
        assert_eq!(next, "acd");
    }

    #[test]
    fn delete_word_forward_matches_vim_spacing() {
        let result =
            execute_vim_action("foo bar baz", sel(0), VimIntent::DeleteWordForward, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("foo "));
        let next = apply_operations("foo bar baz".to_string(), &result.operations);
        assert_eq!(next, "bar baz");
    }

    #[test]
    fn delete_word_backward_with_count_deletes_previous_words() {
        let text = "foo bar baz";
        let cursor = text.find("baz").expect("baz");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteWordBackward, 2, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("foo bar "));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "baz");
    }

    #[test]
    fn delete_word_end_is_distinct_from_delete_word_forward() {
        let result = execute_vim_action("foo bar baz", sel(0), VimIntent::DeleteWordEnd, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("foo"));
        let next = apply_operations("foo bar baz".to_string(), &result.operations);
        assert_eq!(next, " bar baz");
    }

    #[test]
    fn delete_word_end_from_word_end_crosses_to_next_word_end() {
        let result = execute_vim_action("foo bar baz", sel(2), VimIntent::DeleteWordEnd, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("o bar"));
        let next = apply_operations("foo bar baz".to_string(), &result.operations);
        assert_eq!(next, "fo baz");
    }

    #[test]
    fn delete_word_end_deletes_trailing_whitespace_when_no_next_word() {
        let result = execute_vim_action("foo   ", sel(3), VimIntent::DeleteWordEnd, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("   "));
        let next = apply_operations("foo   ".to_string(), &result.operations);
        assert_eq!(next, "foo");
    }

    #[test]
    fn delete_till_char_deletes_up_to_but_not_including_target() {
        let text = "alpha beta gamma";
        let cursor = text.find("alpha").expect("alpha");
        let result = execute_vim_action_with_target(
            text,
            sel(cursor),
            VimIntent::DeleteTillChar,
            1,
            None,
            Some('b'),
        )
        .expect("handled");
        assert_eq!(charwise_register(&result), Some("alpha "));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "beta gamma");
    }

    #[test]
    fn delete_till_char_count_targets_nth_match() {
        let text = "a x b x c";
        let result = execute_vim_action_with_target(
            text,
            sel(0),
            VimIntent::DeleteTillChar,
            2,
            None,
            Some('x'),
        )
        .expect("handled");
        assert_eq!(charwise_register(&result), Some("a x b "));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "x c");
    }

    #[test]
    fn delete_till_char_noops_when_target_missing() {
        let result = execute_vim_action_with_target(
            "alpha beta",
            sel(0),
            VimIntent::DeleteTillChar,
            1,
            None,
            Some('z'),
        )
        .expect("handled");
        assert!(result.operations.is_empty());
        assert!(result.register.is_none());
    }

    #[test]
    fn yank_word_backward_collects_previous_word_chunk() {
        let text = "foo bar baz";
        let cursor = text.find("baz").expect("baz");
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankWordBackward, 1, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("bar "));
    }

    #[test]
    fn delete_inside_word_removes_word_without_padding() {
        let text = "foo bar baz";
        let cursor = text.find("bar").expect("bar") + 1;
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideWord, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("bar"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "foo  baz");
    }

    #[test]
    fn delete_around_word_removes_word_with_trailing_space() {
        let text = "foo bar baz";
        let cursor = text.find("bar").expect("bar");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundWord, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("bar "));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "foo baz");
    }

    #[test]
    fn yank_inside_word_updates_register_without_edit() {
        let text = "foo bar baz";
        let cursor = text.find("bar").expect("bar") + 2;
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankInsideWord, 1, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("bar"));
    }

    #[test]
    fn delete_inside_pipe_removes_pipe_cell_contents_only() {
        let text = "| left | right |";
        let cursor = text.find("left").expect("left") + 1;
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsidePipe, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("left"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "|  | right |");
    }

    #[test]
    fn delete_around_pipe_removes_wrapping_pipes_too() {
        let text = "| left | right |";
        let cursor = text.find("left").expect("left") + 1;
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundPipe, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("| left |"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, " right |");
    }

    #[test]
    fn delete_inside_paren_removes_text_between_parens() {
        let text = "fn call(alpha + beta)";
        let cursor = text.find("alpha").expect("alpha");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideParen, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("alpha + beta"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "fn call()");
    }

    #[test]
    fn delete_inside_bracket_removes_text_between_brackets() {
        let text = "list[keep, remove]";
        let cursor = text.find("keep").expect("keep");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideBracket, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("keep, remove"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "list[]");
    }

    #[test]
    fn delete_inside_brace_removes_text_between_braces() {
        let text = "obj{keep: value}";
        let cursor = text.find("keep").expect("keep");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideBrace, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("keep: value"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "obj{}");
    }

    #[test]
    fn delete_inside_double_quote_removes_quoted_content() {
        let text = "say \"hello world\" now";
        let cursor = text.find("hello").expect("hello");
        let result = execute_vim_action(
            text,
            sel(cursor),
            VimIntent::DeleteInsideDoubleQuote,
            1,
            None,
        )
        .expect("handled");
        assert_eq!(charwise_register(&result), Some("hello world"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "say \"\" now");
    }

    #[test]
    fn delete_inside_backtick_removes_code_span_content() {
        let text = "run `cargo test` soon";
        let cursor = text.find("cargo").expect("cargo");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideBacktick, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("cargo test"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "run `` soon");
    }

    #[test]
    fn delete_inside_asterisk_removes_emphasis_content() {
        let text = "prefix *inside* suffix";
        let cursor = text.find("inside").expect("inside");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideAsterisk, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("inside"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix ** suffix");
    }

    #[test]
    fn delete_inside_tilde_removes_tilde_wrapped_content() {
        let text = "prefix ~inside~ suffix";
        let cursor = text.find("inside").expect("inside");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideTilde, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("inside"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix ~~ suffix");
    }

    #[test]
    fn delete_inside_underscore_removes_underscore_wrapped_content() {
        let text = "prefix _inside_ suffix";
        let cursor = text.find("inside").expect("inside");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideUnderscore, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("inside"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix __ suffix");
    }

    #[test]
    fn delete_around_paren_removes_delimiters_too() {
        let text = "fn call(alpha + beta)";
        let cursor = text.find("alpha").expect("alpha");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundParen, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("(alpha + beta)"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "fn call");
    }

    #[test]
    fn delete_around_bracket_removes_delimiters_too() {
        let text = "list[keep, remove]";
        let cursor = text.find("keep").expect("keep");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundBracket, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("[keep, remove]"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "list");
    }

    #[test]
    fn delete_around_brace_removes_delimiters_too() {
        let text = "obj{keep: value}";
        let cursor = text.find("keep").expect("keep");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundBrace, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("{keep: value}"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "obj");
    }

    #[test]
    fn delete_around_double_quote_removes_delimiters_too() {
        let text = "say \"hello world\" now";
        let cursor = text.find("hello").expect("hello");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundDoubleQuote, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("\"hello world\""));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "say  now");
    }

    #[test]
    fn delete_around_backtick_removes_delimiters_too() {
        let text = "run `cargo test` soon";
        let cursor = text.find("cargo").expect("cargo");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundBacktick, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("`cargo test`"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "run  soon");
    }

    #[test]
    fn delete_around_asterisk_removes_delimiters_too() {
        let text = "prefix *inside* suffix";
        let cursor = text.find("inside").expect("inside");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundAsterisk, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("*inside*"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix  suffix");
    }

    #[test]
    fn delete_around_tilde_removes_delimiters_too() {
        let text = "prefix ~inside~ suffix";
        let cursor = text.find("inside").expect("inside");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundTilde, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("~inside~"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix  suffix");
    }

    #[test]
    fn delete_around_underscore_removes_delimiters_too() {
        let text = "prefix _inside_ suffix";
        let cursor = text.find("inside").expect("inside");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundUnderscore, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("_inside_"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "prefix  suffix");
    }

    #[test]
    fn delete_inside_asterisk_handles_double_markers() {
        let text = "Capability is **an action** linked";
        let cursor = text.find("action").expect("action");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideAsterisk, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("an action"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "Capability is **** linked");
    }

    #[test]
    fn delete_inside_asterisk_handles_double_marker_boundary_cursor() {
        let text = "Capability is **an action** linked";
        let cursor = text.find("** linked").expect("closing marker boundary");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideAsterisk, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("an action"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "Capability is **** linked");
    }

    #[test]
    fn delete_around_asterisk_handles_double_markers() {
        let text = "Capability is **an action** linked";
        let cursor = text.find("action").expect("action");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteAroundAsterisk, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("**an action**"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "Capability is  linked");
    }

    #[test]
    fn delete_inside_underscore_handles_double_markers() {
        let text = "Capability is __an action__ linked";
        let cursor = text.find("action").expect("action");
        let result =
            execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideUnderscore, 1, None)
                .expect("handled");
        assert_eq!(charwise_register(&result), Some("an action"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "Capability is ____ linked");
    }

    #[test]
    fn delete_inside_tilde_handles_double_markers() {
        let text = "Capability is ~~an action~~ linked";
        let cursor = text.find("action").expect("action");
        let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideTilde, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("an action"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "Capability is ~~~~ linked");
    }

    #[test]
    fn delete_inside_tilde_keeps_boundaries_for_all_boundary_and_inner_cursor_positions() {
        let text = "~~test~~";
        for cursor in 0..text.len() {
            let result = execute_vim_action(text, sel(cursor), VimIntent::DeleteInsideTilde, 1, None)
                .expect("handled");
            assert_eq!(
                charwise_register(&result),
                Some("test"),
                "unexpected register at cursor {}",
                cursor
            );
            let next = apply_operations(text.to_string(), &result.operations);
            assert_eq!(next, "~~~~", "unexpected result at cursor {}", cursor);
        }
    }

    #[test]
    fn ci_tilde_on_double_marker_text_deletes_only_inner_content() {
        let one = vim::step(
            &VimState::default(),
            vim::parse_key_token("char:c").expect("c"),
            &vim::VimContext::default(),
        );
        let two = vim::step(
            &one.state,
            vim::parse_key_token("char:i").expect("i"),
            &vim::VimContext::default(),
        );
        let three = vim::step(
            &two.state,
            vim::parse_key_token("char:~").expect("~"),
            &vim::VimContext::default(),
        );
        assert_eq!(three.state.mode, VimMode::Insert);
        assert_eq!(three.actions.len(), 2);
        assert_eq!(three.actions[0].intent, VimIntent::DeleteInsideTilde);
        assert_eq!(three.actions[1].intent, VimIntent::EnterInsert);

        let text = "~~test~~";
        let cursor = text.find("test").expect("test");
        let result = execute_vim_action(text, sel(cursor), three.actions[0].intent, 1, None)
            .expect("handled");
        assert_eq!(charwise_register(&result), Some("test"));
        let next = apply_operations(text.to_string(), &result.operations);
        assert_eq!(next, "~~~~");
    }

    #[test]
    fn counted_yank_around_word_walks_forward_by_object_end() {
        let text = "foo bar baz";
        let cursor = text.find("foo").expect("foo");
        let result = execute_vim_action(text, sel(cursor), VimIntent::YankAroundWord, 2, None)
            .expect("handled");
        assert!(result.operations.is_empty());
        assert_eq!(charwise_register(&result), Some("foo \nbar "));
    }
}
