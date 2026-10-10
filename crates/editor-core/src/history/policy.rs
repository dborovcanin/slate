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

impl<M: Clone + Default> LineHistory<M> {
    /// Normal/visual commands begin a step; mutations within one key may merge.
    pub fn begin_input(&mut self, session: UndoSession) {
        if session == UndoSession::Command {
            self.break_coalescing();
        }
    }
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
        let changed = if let Some(delta) = delta.filter(|_| lines.len() > COALESCE_ANCHOR_MAX_LINES)
        {
            history.record_edit_span(lines, cursor.line, cursor.col, delta)
        } else {
            history.record_edit(lines, cursor.line, cursor.col, coalesce)
        };
        if changed && history.recorded_new_entry {
            self.push(UndoAction::Text);
            if history.recorded_eviction {
                // The text store evicts its oldest entry. Remove that entry's
                // marker while preserving reminder order and the new marker.
                if let Some(idx) = self
                    .actions
                    .iter()
                    .position(|action| matches!(action, UndoAction::Text))
                {
                    self.actions.remove(idx);
                    self.pos -= 1;
                }
            }
        } else if history.undo_depth() < depth_before
            && self.pos == self.actions.len()
            && matches!(self.actions.last(), Some(UndoAction::Text))
        {
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
