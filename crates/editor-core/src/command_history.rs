use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandHistoryStep {
    pub index: usize,
    pub command: String,
}

pub fn sanitize_command(raw_command: &str) -> String {
    let trimmed = raw_command.trim();
    trimmed.strip_prefix(':').unwrap_or(trimmed).to_string()
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

