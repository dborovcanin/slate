//! Deterministic undo grouping and ordering; clocks and side effects stay in the host.
use super::{HistoryCursor, LineHistory, COALESCE_ANCHOR_MAX_LINES};
use crate::buffer::EditDelta;
use std::time::Duration;

const TYPING_DEBOUNCE: Duration = Duration::from_millis(300);

/// The host classifies input; core decides where a new undo step begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoSession {
    Insert,
    Command,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct UndoGrouping {
    pub session: UndoSession,
    pub elapsed: Duration,
}

impl UndoGrouping {
    fn coalesces(self) -> bool {
        self.session == UndoSession::Insert || self.elapsed < TYPING_DEBOUNCE
    }
}

/// Reminder payloads are opaque to the core and applied by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoAction<R> {
    Text,
    Reminder(R),
}

#[derive(Debug, Clone)]
pub struct UndoPolicy<R> {
    actions: Vec<UndoAction<R>>,
    pos: usize,
}

impl<R> Default for UndoPolicy<R> {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            pos: 0,
        }
    }
}

impl<R> UndoPolicy<R> {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.pos = 0;
    }

    /// Normal/visual commands begin a step; mutations within one key may merge.
    pub fn begin_input<M: Clone + Default>(
        &mut self,
        history: &mut LineHistory<M>,
        session: UndoSession,
    ) {
        if session == UndoSession::Command {
            history.break_coalescing();
        }
    }

    pub fn record_reminder(&mut self, payload: R) {
        self.push(UndoAction::Reminder(payload));
    }

    fn push(&mut self, action: UndoAction<R>) {
        if self.pos < self.actions.len() {
            self.actions.truncate(self.pos);
        }
        self.actions.push(action);
        self.pos = self.actions.len();
    }

    /// Keep the text store and semantic action sequence aligned after an edit.
    pub fn record_text<M: Clone + Default>(
        &mut self,
        history: &mut LineHistory<M>,
        lines: &[String],
        cursor: HistoryCursor,
        delta: Option<EditDelta>,
        grouping: UndoGrouping,
    ) {
        let coalesce = grouping.coalesces();
        let depth_before = history.undo_depth();
        if let Some(delta) = delta.filter(|_| lines.len() > COALESCE_ANCHOR_MAX_LINES) {
            // Above the anchor cap, the span path preserves one entry per edit
            // without the full-document prefix/suffix scan.
            let changed = history.record_edit_span(lines, cursor.line, cursor.col, delta);
            if changed && (!coalesce || history.undo_depth() > depth_before) {
                self.push(UndoAction::Text);
            }
            return;
        }
        let changed = history.record_edit(lines, cursor.line, cursor.col, coalesce);
        let depth_after = history.undo_depth();
        if changed && (!coalesce || depth_after > depth_before) {
            self.push(UndoAction::Text);
        } else if depth_after < depth_before
            && self.pos == self.actions.len()
            && matches!(self.actions.last(), Some(UndoAction::Text))
        {
            // A coalesced edit cancelled itself; remove its marker as well.
            self.actions.pop();
            self.pos = self.actions.len();
        }
    }

    pub fn undo_depth(&self) -> usize {
        self.pos
    }

    pub fn undo_action(&self) -> Option<&UndoAction<R>> {
        self.pos
            .checked_sub(1)
            .and_then(|pos| self.actions.get(pos))
    }

    pub fn redo_action(&self) -> Option<&UndoAction<R>> {
        self.actions.get(self.pos)
    }

    /// Acknowledge the offered action after the host applies it. On a reminder
    /// error the host leaves it pending. Text-store exhaustion retains the
    /// existing behavior: the host reports it and acknowledges the marker.
    pub fn complete_undo(&mut self) {
        self.pos = self.pos.saturating_sub(1);
    }

    /// Call only after applying the action returned by `redo_action`.
    pub fn complete_redo(&mut self) {
        self.pos += 1;
    }
}

#[cfg(test)]
mod tests;
