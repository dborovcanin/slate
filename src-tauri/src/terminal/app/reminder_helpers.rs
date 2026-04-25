use super::LineReminderGhost;
use crate::storage::Db;
use std::collections::{HashMap, HashSet};

pub(super) fn load_note_reminder_ghosts(
    db: &Db,
    note_id: &str,
    lines: &[String],
) -> Result<HashMap<usize, LineReminderGhost>, String> {
    #[derive(Debug, Clone)]
    struct PlannedReminder {
        source_line: i64,
        source_text: String,
        target_line: i64,
        target_text: String,
        remind_at_ms: i64,
        display_at: String,
        notified_at_ms: Option<i64>,
    }

    fn nearest_available_line(
        candidates: &[usize],
        preferred: usize,
        used_lines: &HashSet<usize>,
    ) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_distance = usize::MAX;
        for &line_number in candidates {
            if used_lines.contains(&line_number) {
                continue;
            }
            let distance = line_number.abs_diff(preferred);
            if distance < best_distance
                || (distance == best_distance
                    && best.map(|current| line_number < current).unwrap_or(true))
            {
                best = Some(line_number);
                best_distance = distance;
            }
        }
        best
    }

    fn nearest_free_line(
        preferred: usize,
        line_count: usize,
        used_lines: &HashSet<usize>,
    ) -> Option<usize> {
        if line_count == 0 {
            return None;
        }
        let clamped = preferred.clamp(1, line_count);
        if !used_lines.contains(&clamped) {
            return Some(clamped);
        }
        for distance in 1..=line_count {
            let down = clamped.saturating_add(distance);
            if down <= line_count && !used_lines.contains(&down) {
                return Some(down);
            }
            let up = clamped.saturating_sub(distance);
            if up >= 1 && !used_lines.contains(&up) {
                return Some(up);
            }
        }
        None
    }

    let mut reminders = db.list_reminders(note_id)?;
    reminders.sort_by_key(|reminder| reminder.line_number);

    // Fast path for large notes: avoid building line-text indexes when there
    // are no reminders at all.
    if reminders.is_empty() {
        return Ok(HashMap::new());
    }

    let mut text_to_lines: Option<HashMap<String, Vec<usize>>> = None;

    let mut used_lines: HashSet<usize> = HashSet::new();
    let mut planned = Vec::with_capacity(reminders.len());

    for reminder in reminders {
        let old_line = reminder.line_number;
        let old_line_usize = usize::try_from(old_line).ok().filter(|line| *line >= 1);
        let old_line_idx = old_line_usize
            .and_then(|line| line.checked_sub(1))
            .filter(|idx| *idx < lines.len());
        let old_text_matches = old_line_idx
            .and_then(|idx| lines.get(idx))
            .map(|line| line == &reminder.line_text)
            .unwrap_or(false);

        let preferred_line = old_line_usize.unwrap_or(1);
        let mut target_line = None;

        if old_text_matches {
            if let Some(old_line_num) = old_line_usize {
                if !used_lines.contains(&old_line_num) {
                    target_line = Some(old_line_num);
                }
            }
        }

        if target_line.is_none() {
            if text_to_lines.is_none() {
                let mut index: HashMap<String, Vec<usize>> = HashMap::new();
                for (idx, line) in lines.iter().enumerate() {
                    index.entry(line.clone()).or_default().push(idx + 1);
                }
                text_to_lines = Some(index);
            }
            if let Some(candidates) = text_to_lines
                .as_ref()
                .and_then(|index| index.get(&reminder.line_text))
            {
                target_line = nearest_available_line(candidates, preferred_line, &used_lines);
            }
        }

        if target_line.is_none() {
            if let Some(old_line_num) = old_line_usize {
                if old_line_num >= 1
                    && old_line_num <= lines.len()
                    && !used_lines.contains(&old_line_num)
                {
                    target_line = Some(old_line_num);
                }
            }
        }

        if target_line.is_none() {
            target_line = nearest_free_line(preferred_line, lines.len(), &used_lines);
        }

        let Some(target_line) = target_line else {
            continue;
        };
        used_lines.insert(target_line);

        let Some(target_text) = lines.get(target_line.saturating_sub(1)).cloned() else {
            continue;
        };

        planned.push(PlannedReminder {
            source_line: old_line,
            source_text: reminder.line_text,
            target_line: i64::try_from(target_line).unwrap_or(old_line),
            target_text,
            remind_at_ms: reminder.remind_at_ms,
            display_at: reminder.display_at,
            notified_at_ms: reminder.notified_at_ms,
        });
    }

    if !planned.is_empty() {
        let max_existing_line = planned
            .iter()
            .flat_map(|entry| [entry.source_line, entry.target_line])
            .filter(|line| *line > 0)
            .max()
            .unwrap_or_else(|| i64::try_from(lines.len()).unwrap_or(1));
        let temp_base = max_existing_line.saturating_add(10);
        let mut temp_moves = Vec::new();

        for (idx, entry) in planned.iter().enumerate() {
            if entry.source_line == entry.target_line {
                continue;
            }
            let temp_line = temp_base.saturating_add(i64::try_from(idx).unwrap_or(0) + 1);
            if db.move_reminder_line(note_id, entry.source_line, temp_line, &entry.source_text)? {
                temp_moves.push((idx, temp_line));
            }
        }

        for (idx, temp_line) in temp_moves {
            let entry = &planned[idx];
            let _ =
                db.move_reminder_line(note_id, temp_line, entry.target_line, &entry.target_text)?;
        }

        for entry in &planned {
            if entry.source_line == entry.target_line && entry.source_text != entry.target_text {
                let _ = db.move_reminder_line(
                    note_id,
                    entry.source_line,
                    entry.target_line,
                    &entry.target_text,
                )?;
            }
        }
    }

    let mut by_line = HashMap::with_capacity(planned.len());
    for entry in planned {
        let Some(line_idx) = entry
            .target_line
            .checked_sub(1)
            .and_then(|line| usize::try_from(line).ok())
        else {
            continue;
        };
        by_line.insert(
            line_idx,
            LineReminderGhost {
                remind_at_ms: entry.remind_at_ms,
                display_at: entry.display_at,
                line_text: entry.target_text,
                notified_at_ms: entry.notified_at_ms,
            },
        );
    }

    Ok(by_line)
}
