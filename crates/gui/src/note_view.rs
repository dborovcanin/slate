//! The open note and the lines the window paints, without any GPUI types.
//!
//! Everything here comes from the shared crates: `NoteSession` owns the note,
//! calc runs through the session's `NoteCalcProvider`, and line styling comes
//! from `note_session::display`. This module only groups that output into
//! runs and table cells the window can lay out.
use app_core::cross_note::CrossNoteVarIndex;
use app_core::storage::{Db, Note, NoteModules, NoteSummary};
use editor_core::calc_plan::CalcFeatureMask;
use editor_core::markdown_tokens::{self, FenceState};
use note_session::calc::CalcInputs;
use note_session::calc_provider::{CrossNoteSource, NoteCalcProvider};
use note_session::display::semantic::{
    LineDecorations, SemanticContext, SemanticLine, SemanticStyle,
};
use note_session::display::table::{format_formula_display_value, is_markdown_table_line};
use note_session::{Document, NoteSession};
use std::sync::{Arc, Condvar, Mutex};

/// A stretch of one line with a single style.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: SemanticStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    /// The cell holds a formula and `text` is its computed value.
    pub formula: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineKind {
    Text,
    /// Markdown heading, level 1–6.
    Heading(usize),
    /// Checklist item; runs start after the `- [ ]` marker.
    Checklist { checked: bool },
    TableRow { cells: Vec<TableCell>, header: bool },
    /// The `| --- |` row under a table header; drawn as the header rule.
    TableDelimiter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineView {
    pub index: usize,
    pub kind: LineKind,
    pub runs: Vec<Run>,
    /// Calc result shown after the text, with its ` = ` or ` → ` prefix.
    pub ghost: Option<String>,
}

pub struct NoteHost {
    db: Db,
    index: Arc<Mutex<CrossNoteVarIndex>>,
    loaded: Condvar,
    pub doc: Document,
    pub session: NoteSession,
    pub title: String,
    pub modules: NoteModules,
    pub notes: Vec<NoteSummary>,
}

impl NoteHost {
    /// Open `note_id`, or the most recently edited note when none is given.
    pub fn open(db: Db, note_id: Option<&str>) -> Result<Self, String> {
        let notes = db.list_notes_meta()?;
        let id = match note_id {
            Some(id) => id.to_string(),
            None => notes
                .first()
                .map(|n| n.id.clone())
                .ok_or_else(|| "no notes yet; create one with `slate --new`".to_string())?,
        };
        let mut doc = Document::default();
        let history =
            note_session::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let session = NoteSession::new(history, Default::default(), Default::default());
        let mut host = Self {
            db,
            index: Arc::new(Mutex::new(Default::default())),
            loaded: Condvar::new(),
            doc,
            session,
            title: String::new(),
            modules: NoteModules::default(),
            notes,
        };
        host.switch_to(&id)?;
        Ok(host)
    }

    pub fn note_id(&self) -> &str {
        self.session.note_id()
    }

    pub fn switch_to(&mut self, id: &str) -> Result<(), String> {
        let note: Note = self
            .db
            .get_note(id)?
            .ok_or_else(|| format!("note {id} not found"))?;
        self.session.open(&note, &mut self.doc);
        self.modules = note.modules;
        self.title = self
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .map(|n| n.title.clone())
            .unwrap_or_default();
        self.recompute_calc();
        Ok(())
    }

    /// Full recompute through the provider the terminal also uses.
    pub fn recompute_calc(&mut self) {
        let modules = self.modules;
        let note_id = self.session.note_id().to_string();
        let provider = NoteCalcProvider {
            base: CalcInputs {
                mask: CalcFeatureMask {
                    math_enabled: modules.math,
                    table_enabled: modules.table,
                    variables_enabled: modules.variables,
                },
                math_enabled: modules.math,
                viewport_only: false,
            },
            cross_note_enabled: modules.cross_note,
            table_enabled: modules.table,
            cross_note: CrossNoteSource {
                note_id: &note_id,
                index: &self.index,
                db: &self.db,
                loaded: &self.loaded,
            },
            selection_range: None,
        };
        let lines = self.doc.lines().len();
        self.session.evaluate_calc_range(&self.doc, 0, lines, &provider);
    }

    /// Move the cursor to the start of `line`, clamped to the note.
    pub fn set_cursor_line(&mut self, line: usize) {
        let last = self.doc.lines().len().saturating_sub(1);
        self.doc.cursor_line = line.min(last);
        self.doc.cursor_col = 0;
    }

    /// Lines `from..to` ready to paint. Code fences are tracked from the top
    /// of the note so a range that starts inside a block is styled as code.
    pub fn lines(&self, from: usize, to: usize) -> Vec<LineView> {
        let lines = self.doc.lines();
        let to = to.min(lines.len());
        let calc = self.session.calc();
        let variables = (!calc.variable_names.is_empty()).then_some(&calc.variable_names);
        let mut ctx = SemanticContext {
            fence: FenceState::default(),
            render_as_plain_code: false,
            forced_code_lang: None,
        };
        for text in &lines[..from.min(to)] {
            markdown_tokens::advance_fence_state(&mut ctx.fence, text);
        }
        let mut out = Vec::with_capacity(to.saturating_sub(from));
        for index in from..to {
            let text = lines[index].as_str();
            let is_cursor = index == self.doc.cursor_line;
            let result = calc.results.get(index).and_then(|r| r.as_deref());
            let in_code = ctx.fence.in_code_block;
            let deco = LineDecorations {
                calc_ghost: result,
                variable_names: variables,
                active_cursor_col: is_cursor.then_some(self.doc.cursor_col),
                ..Default::default()
            };
            let styled = ctx.style_line(text, &deco);
            if !in_code && is_markdown_table_line(text) {
                out.push(self.table_line(index, text));
                continue;
            }
            let info = (!in_code).then(|| markdown_tokens::classify_markdown_line(text));
            let mut kind = LineKind::Text;
            let mut start = 0;
            if let Some(info) = &info {
                if let Some(level) = info.heading_level {
                    kind = LineKind::Heading(level);
                } else if let (Some(content), false) = (info.checklist_content_start, is_cursor) {
                    kind = LineKind::Checklist {
                        checked: info.checklist_checked,
                    };
                    start = content;
                }
            }
            out.push(LineView {
                index,
                kind,
                runs: runs(&styled, start),
                ghost: result.map(|r| format!("{}{}", styled.calc_prefix, r)),
            });
        }
        out
    }

    fn table_line(&self, index: usize, text: &str) -> LineView {
        let lines = self.doc.lines();
        if table_syntax::is_delimiter_line_in(lines, index) {
            return LineView {
                index,
                kind: LineKind::TableDelimiter,
                runs: Vec::new(),
                ghost: None,
            };
        }
        let header = table_syntax::is_delimiter_line_in(lines, index + 1);
        let results = self.session.calc().cell_results.get(index);
        let is_cursor = index == self.doc.cursor_line;
        let cells = table_syntax::split_table_cells(text)
            .into_iter()
            .enumerate()
            .map(|(cell_index, raw)| {
                let value = results.and_then(|r| r.iter().find(|e| e.cell_index == cell_index));
                match value {
                    // The cursor row keeps its formulas visible for editing.
                    Some(eval) if !is_cursor => TableCell {
                        text: format_formula_display_value(&eval.value),
                        formula: true,
                    },
                    _ => TableCell {
                        formula: raw.starts_with(":="),
                        text: raw,
                    },
                }
            })
            .collect();
        LineView {
            index,
            kind: LineKind::TableRow { cells, header },
            runs: Vec::new(),
            ghost: None,
        }
    }
}

/// Visible characters from `start`, grouped by style; hidden markers are dropped.
fn runs(line: &SemanticLine, start: usize) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut hidden = line.hidden_ranges.iter().peekable();
    for (i, (&ch, &style)) in line.chars.iter().zip(&line.styles).enumerate().skip(start) {
        while hidden.peek().is_some_and(|&&(_, end)| end <= i) {
            hidden.next();
        }
        if hidden.peek().is_some_and(|&&(from, _)| from <= i) {
            continue;
        }
        match out.last_mut() {
            Some(run) if run.style == style => run.text.push(ch),
            _ => out.push(Run {
                text: ch.to_string(),
                style,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    const LISBON: &str = "# Lisbon trip\n\
        ## Budget\n\
        flights := 2 * 189 EUR\n\
        hotel := 4 * 96 EUR\n\
        flights + hotel\n\
        - [x] Book flights\n\
        - [ ] Renew passport\n\
        \n\
        | Item | Qty | Price | Total |\n\
        | --- | --- | --- | --- |\n\
        | Tram pass | 2 | 6.60 | :=(1,2)*(1,3) |\n\
        | Museum | 2 | 12 | :=(2,2)*(2,3) |";

    struct Fixture {
        host: NoteHost,
        dir: std::path::PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn fixture(body: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "slate-gui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("notes.db")).unwrap();
        db.create_note_with_context("lisbon", Default::default(), None, None)
            .unwrap();
        db.save_note("lisbon", body).unwrap();
        let host = NoteHost::open(db, Some("lisbon")).unwrap();
        Fixture { host, dir }
    }
    fn text(line: &LineView) -> String {
        line.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn headings_hide_their_marker_away_from_the_cursor() {
        let mut f = fixture(LISBON);
        f.host.set_cursor_line(2);
        let lines = f.host.lines(0, 2);
        assert_eq!(lines[0].kind, LineKind::Heading(1));
        assert_eq!(text(&lines[0]), "Lisbon trip");
        assert_eq!(lines[1].kind, LineKind::Heading(2));
        assert_eq!(text(&lines[1]), "Budget");
    }

    #[test]
    fn expressions_carry_their_calc_result() {
        let f = fixture(LISBON);
        let line = &f.host.lines(4, 5)[0];
        let ghost = line.ghost.as_deref().expect("calc result");
        assert!(ghost.contains("762"), "{ghost}");
        assert_eq!(text(line), "flights + hotel");
    }

    #[test]
    fn checklists_report_state_and_drop_the_marker() {
        let f = fixture(LISBON);
        let lines = f.host.lines(5, 7);
        assert_eq!(lines[0].kind, LineKind::Checklist { checked: true });
        assert_eq!(text(&lines[0]), "Book flights");
        assert_eq!(lines[1].kind, LineKind::Checklist { checked: false });
    }

    #[test]
    fn tables_show_formula_values_except_on_the_cursor_row() {
        let mut f = fixture(LISBON);
        let lines = f.host.lines(8, 12);
        let LineKind::TableRow { cells, header } = &lines[0].kind else {
            panic!("header row")
        };
        assert!(*header);
        assert_eq!(cells[0].text, "Item");
        assert_eq!(lines[1].kind, LineKind::TableDelimiter);
        let LineKind::TableRow { cells, header } = &lines[2].kind else {
            panic!("body row")
        };
        assert!(!*header);
        assert!(cells[3].formula);
        assert_eq!(cells[3].text, "13.2");

        f.host.set_cursor_line(10);
        let LineKind::TableRow { cells, .. } = &f.host.lines(10, 11)[0].kind else {
            panic!("cursor row")
        };
        assert_eq!(cells[3].text, ":=(1,2)*(1,3)");
    }

    #[test]
    fn switching_notes_reloads_text_and_calc() {
        let mut f = fixture(LISBON);
        f.host
            .db
            .create_note_with_context("other", Default::default(), None, None)
            .unwrap();
        f.host.db.save_note("other", "2 + 3").unwrap();
        f.host.switch_to("other").unwrap();
        assert_eq!(f.host.note_id(), "other");
        let line = &f.host.lines(0, 1)[0];
        assert_eq!(line.ghost.as_deref(), Some(" → 5"));
    }
}
