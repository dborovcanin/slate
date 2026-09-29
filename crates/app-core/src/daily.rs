//! Daily notes and quick capture. Callers pass the local date/time (the
//! host knows the local timezone reliably); everything here is pure apart
//! from the `Db` calls in `ensure_daily_note` / `capture_to_daily_note`.

use crate::config::DailyNotesConfig;
use crate::storage::{Db, Note};

/// Local calendar date and wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Note id of the daily note for `stamp`'s date: `<prefix>-YYYY-MM-DD`.
pub fn daily_note_id(prefix: &str, stamp: LocalStamp) -> String {
    format!(
        "{prefix}-{:04}-{:02}-{:02}",
        stamp.year, stamp.month, stamp.day
    )
}

/// Body of a new daily note: the template with `{date}` replaced.
pub fn daily_note_body(template: &str, date_label: &str) -> String {
    template.replace("{date}", date_label)
}

/// A captured entry as a timestamped bullet. Continuation lines of
/// multi-line text are indented under the bullet; blank edges are trimmed.
pub fn capture_entry(stamp: LocalStamp, text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().trim_end();
    let mut entry = format!("- {:02}:{:02} {first}", stamp.hour, stamp.minute);
    for line in lines {
        entry.push('\n');
        if !line.trim().is_empty() {
            entry.push_str("  ");
            entry.push_str(line.trim_end());
        }
    }
    entry
}

/// Returns today's daily note, creating it from the template when missing.
pub fn ensure_daily_note(
    db: &Db,
    config: &DailyNotesConfig,
    stamp: LocalStamp,
    date_label: &str,
) -> Result<Note, String> {
    let id = daily_note_id(&config.note_prefix, stamp);
    if let Some(note) = db.get_note(&id)? {
        return Ok(note);
    }
    db.save_note(&id, &daily_note_body(&config.template, date_label))
}

/// Appends `text` as a timestamped entry to today's daily note.
pub fn capture_to_daily_note(
    db: &Db,
    config: &DailyNotesConfig,
    stamp: LocalStamp,
    date_label: &str,
    text: &str,
) -> Result<Note, String> {
    if text.trim().is_empty() {
        return Err("nothing to capture".to_string());
    }
    let note = ensure_daily_note(db, config, stamp, date_label)?;
    db.append_note_body(&note.id, &format!("{}\n", capture_entry(stamp, text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: LocalStamp = LocalStamp {
        year: 2026,
        month: 9,
        day: 7,
        hour: 8,
        minute: 5,
    };

    fn temp_db() -> (Db, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("slate-daily-{}.db", ulid::Ulid::new()));
        (Db::open(path.clone()).expect("db opens"), path)
    }

    #[test]
    fn daily_note_id_zero_pads_the_date() {
        assert_eq!(daily_note_id("daily", STAMP), "daily-2026-09-07");
    }

    #[test]
    fn template_substitutes_the_date_label() {
        assert_eq!(
            daily_note_body("# {date}\n\n", "07.09.2026"),
            "# 07.09.2026\n\n"
        );
    }

    #[test]
    fn capture_entry_formats_a_timestamped_bullet() {
        assert_eq!(capture_entry(STAMP, "  call Ana  "), "- 08:05 call Ana");
        assert_eq!(
            capture_entry(STAMP, "first\nsecond\n\nthird\n"),
            "- 08:05 first\n  second\n\n  third"
        );
    }

    #[test]
    fn ensure_creates_once_and_capture_appends() {
        let (db, path) = temp_db();
        let config = DailyNotesConfig::default();

        let created = ensure_daily_note(&db, &config, STAMP, "2026-09-07").expect("created");
        assert_eq!(created.id, "daily-2026-09-07");
        assert_eq!(created.body, "# 2026-09-07\n\n");

        capture_to_daily_note(&db, &config, STAMP, "2026-09-07", "buy milk").expect("capture");
        let later = LocalStamp {
            hour: 9,
            minute: 30,
            ..STAMP
        };
        let note =
            capture_to_daily_note(&db, &config, later, "2026-09-07", "ship it").expect("capture");
        assert_eq!(
            note.body,
            "# 2026-09-07\n\n- 08:05 buy milk\n- 09:30 ship it\n"
        );

        let again = ensure_daily_note(&db, &config, STAMP, "ignored").expect("existing");
        assert_eq!(
            again.body, note.body,
            "an existing daily note is not rewritten"
        );

        assert!(capture_to_daily_note(&db, &config, STAMP, "x", "  ").is_err());

        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
