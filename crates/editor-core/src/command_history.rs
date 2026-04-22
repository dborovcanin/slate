use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandHistoryStep {
    pub index: usize,
    pub command: String,
}

pub fn sanitize_command(raw_command: &str) -> String {
    let trimmed = raw_command.trim();
    let base = trimmed.strip_prefix(':').unwrap_or(trimmed);
    redact_command_arguments(base)
}

fn normalize_note_security_action(token: &str) -> Option<&'static str> {
    let normalized = token.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "lock" | "note-lock" | "lock-note" => Some("lock"),
        "unlock" | "note-unlock" | "unlock-note" => Some("unlock"),
        "encrypt" | "note-encrypt" | "encrypt-note" => Some("encrypt"),
        "decrypt" | "note-decrypt" | "decrypt-note" => Some("decrypt"),
        "unprotect"
        | "unencrypt"
        | "note-unprotect"
        | "unprotect-note"
        | "note-unencrypt"
        | "unencrypt-note" => Some("unprotect"),
        _ => None,
    }
}

fn redact_command_arguments(command: &str) -> String {
    let mut tokens = command.split_whitespace();
    let Some(first) = tokens.next() else {
        return String::new();
    };
    let second = tokens.next();

    if first.eq_ignore_ascii_case("note") {
        let Some(action_token) = second else {
            return command.to_string();
        };
        if normalize_note_security_action(action_token).is_some() && tokens.next().is_some() {
            return format!("{first} {action_token}");
        }
        return command.to_string();
    }

    if normalize_note_security_action(first).is_some() && second.is_some() {
        return first.to_string();
    }
    command.to_string()
}

pub fn remember_command(history: &mut Vec<String>, raw_command: &str, max_entries: usize) {
    let command = sanitize_command(raw_command);
    if command.is_empty() {
        return;
    }

    if let Some(existing_idx) = history.iter().rposition(|entry| entry == &command) {
        history.remove(existing_idx);
    }
    history.push(command);

    if history.len() > max_entries {
        let drain_count = history.len() - max_entries;
        history.drain(0..drain_count);
    }
}

pub fn cycle_prev(history: &[String], current_index: Option<usize>) -> Option<CommandHistoryStep> {
    let history_len = history.len();
    if history_len == 0 {
        return None;
    }

    let next_idx = match current_index {
        Some(current) => (current + history_len - 1) % history_len,
        None => history_len - 1,
    };
    Some(CommandHistoryStep {
        index: next_idx,
        command: history[next_idx].clone(),
    })
}

pub fn cycle_next(history: &[String], current_index: Option<usize>) -> Option<CommandHistoryStep> {
    let history_len = history.len();
    if history_len == 0 {
        return None;
    }

    let next_idx = match current_index {
        Some(current) => (current + 1) % history_len,
        None => 0,
    };
    Some(CommandHistoryStep {
        index: next_idx,
        command: history[next_idx].clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_whitespace_and_colon() {
        assert_eq!(sanitize_command(" :sum "), "sum");
        assert_eq!(sanitize_command("format"), "format");
    }

    #[test]
    fn sanitize_redacts_note_password_arguments() {
        assert_eq!(sanitize_command(":note lock hunter2"), "note lock");
        assert_eq!(
            sanitize_command("note encrypt super secret"),
            "note encrypt"
        );
        assert_eq!(sanitize_command("note unprotect pass123"), "note unprotect");
        assert_eq!(sanitize_command(":lock-note hunter2"), "lock-note");
        assert_eq!(sanitize_command("encrypt-note super secret"), "encrypt-note");
        assert_eq!(sanitize_command("note-encrypt pass123"), "note-encrypt");
        assert_eq!(sanitize_command("note unlock"), "note unlock");
        assert_eq!(sanitize_command("sum"), "sum");
    }

    #[test]
    fn remember_keeps_latest_unique_entries() {
        let mut history = vec!["sum".to_string(), "format".to_string()];
        remember_command(&mut history, ":sum", 100);
        assert_eq!(history, vec!["format", "sum"]);
    }

    #[test]
    fn remember_respects_max_entries() {
        let mut history = Vec::new();
        remember_command(&mut history, "one", 2);
        remember_command(&mut history, "two", 2);
        remember_command(&mut history, "three", 2);
        assert_eq!(history, vec!["two", "three"]);
    }

    #[test]
    fn cycle_prev_and_next_wrap() {
        let history = vec!["one".to_string(), "two".to_string(), "three".to_string()];

        let a = cycle_prev(&history, None).expect("prev");
        assert_eq!(a.index, 2);
        assert_eq!(a.command, "three");

        let b = cycle_prev(&history, Some(a.index)).expect("prev");
        assert_eq!(b.index, 1);
        assert_eq!(b.command, "two");

        let c = cycle_next(&history, Some(b.index)).expect("next");
        assert_eq!(c.index, 2);
        assert_eq!(c.command, "three");

        let d = cycle_next(&history, Some(c.index)).expect("next");
        assert_eq!(d.index, 0);
        assert_eq!(d.command, "one");
    }
}
