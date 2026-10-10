//! Note-wide calc decisions: what to evaluate when a note is first shown,
//! opened, or changed as a whole (calc modules, exchange rates).
use crate::calc::CalcInputs;
use crate::calc_provider::{
    NoteCalcProvider, CALC_VIEWPORT_ONLY_MIN_LINES, LARGE_DOC_CALC_DEFER_LINES,
};
use crate::{Document, NoteSession};
use editor_core::calc_plan::{should_defer_after_edit, CalcSignalFlags};

/// Startup evaluates notes below this size inline, unless calc is skipped.
pub const CALC_ASYNC_MIN_LINES: usize = 2_000;

/// What a front end does after a note-wide calc reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalcReset {
    /// Only visible lines are evaluated; the front end schedules viewport passes.
    pub viewport_only: bool,
    /// Results were cleared; viewport bookkeeping must start over.
    pub cleared: bool,
    /// Evaluate the viewport now (or start preparing it in the background).
    pub refresh_viewport: bool,
}

/// How the first note shown at startup is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialCalcPlan {
    pub viewport_only: bool,
    /// No evaluation at startup: viewport-only notes and notes without calc.
    pub skip: bool,
    /// Large notes with formulas and variables evaluate on the first idle ticks.
    pub defer: bool,
}

/// Signals as evaluation sees them: disabled modules hide their constructs.
fn active_signals(raw: CalcSignalFlags, base: CalcInputs) -> CalcSignalFlags {
    CalcSignalFlags {
        has_builtin_formula: base.math_enabled && raw.has_builtin_formula,
        has_variable_assignment: base.math_enabled
            && base.mask.variables_enabled
            && raw.has_variable_assignment,
        has_expression: base.math_enabled && raw.has_expression,
    }
}

fn viewport_only(line_count: usize, signals: CalcSignalFlags) -> bool {
    line_count >= CALC_VIEWPORT_ONLY_MIN_LINES
        && signals.has_variable_assignment
        && !signals.has_builtin_formula
}

fn has_calc(signals: CalcSignalFlags) -> bool {
    signals.has_builtin_formula || signals.has_variable_assignment || signals.has_expression
}

/// Decide startup evaluation from the note's raw calc signals.
pub fn initial_calc_plan(
    line_count: usize,
    raw: CalcSignalFlags,
    base: CalcInputs,
) -> InitialCalcPlan {
    let signals = active_signals(raw, base);
    let viewport_only = viewport_only(line_count, signals);
    InitialCalcPlan {
        viewport_only,
        skip: viewport_only || !has_calc(signals),
        defer: line_count >= CALC_ASYNC_MIN_LINES
            && signals.has_builtin_formula
            && signals.has_variable_assignment,
    }
}

impl NoteSession {
    /// The flags after-edit planning uses: variables are gated by their
    /// modules; formulas and expressions are the note's cached raw signals.
    fn signals(&self, base: CalcInputs) -> CalcSignalFlags {
        CalcSignalFlags {
            has_builtin_formula: self.calc.cached_has_builtin_formula,
            has_variable_assignment: base.math_enabled
                && base.mask.variables_enabled
                && self.calc.cached_has_variable_assignment,
            has_expression: self.calc.cached_has_expression,
        }
    }

    /// Whether this note evaluates calc for the viewport only.
    pub fn viewport_only_calc(&self, doc: &Document, base: CalcInputs) -> bool {
        viewport_only(doc.lines().len(), self.signals(base))
    }

    /// After opening a note: refresh values from referenced notes, rescan the
    /// note's calc signals, then clear, defer or recompute.
    pub fn reset_calc_after_open(
        &mut self,
        doc: &mut Document,
        provider: &NoteCalcProvider<'_>,
    ) -> CalcReset {
        if provider.cross_note_enabled && doc.lines().iter().any(|line| line.contains("[[")) {
            app_core::cross_note::refresh_referenced_notes(
                provider.cross_note.db,
                provider.cross_note.index,
                doc.lines(),
            );
        }
        self.calc.rescan_calc_flags(doc, provider.base);
        let signals = self.signals(provider.base);
        let viewport_only = viewport_only(doc.lines().len(), signals);
        if viewport_only || !has_calc(signals) {
            self.calc.clear(doc);
            self.calc.stale = false;
        } else if should_defer_after_edit(doc.lines().len(), signals, LARGE_DOC_CALC_DEFER_LINES) {
            self.calc.clear(doc);
            self.calc.stale = true;
        } else {
            self.recompute_calc_with(doc, &with_viewport_only(provider, viewport_only));
        }
        CalcReset {
            viewport_only,
            cleared: true,
            refresh_viewport: viewport_only,
        }
    }

    /// After something note-wide changed (calc modules, exchange rates):
    /// every result is re-evaluated, the viewport only for large notes.
    pub fn reset_calc_after_note_wide_change(
        &mut self,
        doc: &mut Document,
        provider: &NoteCalcProvider<'_>,
    ) -> CalcReset {
        let viewport_only = self.viewport_only_calc(doc, provider.base);
        if !provider.base.math_enabled {
            self.calc.clear(doc);
            return CalcReset {
                viewport_only,
                cleared: true,
                refresh_viewport: false,
            };
        }
        self.calc.stale = true;
        if viewport_only {
            self.calc.clear(doc);
            return CalcReset {
                viewport_only,
                cleared: true,
                refresh_viewport: true,
            };
        }
        self.recompute_calc_with(doc, &with_viewport_only(provider, viewport_only));
        CalcReset {
            viewport_only,
            cleared: false,
            refresh_viewport: false,
        }
    }

    /// Clear calc results, e.g. when the math module is disabled.
    pub fn clear_calc(&mut self, doc: &Document) {
        self.calc.clear(doc);
    }
}

/// The provider with this reset's viewport decision, which later evaluation reads.
fn with_viewport_only<'a>(
    provider: &NoteCalcProvider<'a>,
    viewport_only: bool,
) -> NoteCalcProvider<'a> {
    NoteCalcProvider {
        base: CalcInputs {
            viewport_only,
            ..provider.base
        },
        ..*provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::calc_plan::CalcFeatureMask;

    fn base(math: bool, variables: bool) -> CalcInputs {
        CalcInputs {
            mask: CalcFeatureMask {
                math_enabled: math,
                table_enabled: true,
                variables_enabled: variables,
            },
            math_enabled: math,
            viewport_only: false,
        }
    }

    #[test]
    fn startup_plan_matches_the_previous_terminal_rules() {
        let vars = CalcSignalFlags {
            has_variable_assignment: true,
            ..Default::default()
        };
        let both = CalcSignalFlags {
            has_builtin_formula: true,
            has_variable_assignment: true,
            ..Default::default()
        };
        // Large note with variables only: viewport evaluation, nothing at startup.
        let plan = initial_calc_plan(CALC_VIEWPORT_ONLY_MIN_LINES, vars, base(true, true));
        assert!(plan.viewport_only && plan.skip && !plan.defer);
        // Formulas and variables in a large note: deferred to idle ticks.
        let plan = initial_calc_plan(CALC_ASYNC_MIN_LINES, both, base(true, true));
        assert!(!plan.viewport_only && !plan.skip && plan.defer);
        // Disabled modules hide their constructs.
        let plan = initial_calc_plan(10, vars, base(true, false));
        assert!(plan.skip && !plan.viewport_only);
        let plan = initial_calc_plan(10, both, base(false, true));
        assert!(plan.skip && !plan.defer);
        // Small note with calc: evaluated inline.
        let plan = initial_calc_plan(10, both, base(true, true));
        assert!(!plan.skip && !plan.defer && !plan.viewport_only);
    }
}
