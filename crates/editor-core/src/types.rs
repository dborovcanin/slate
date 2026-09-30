use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionSnapshot {
    pub anchor: usize,
    pub head: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorContextSnapshot {
    pub text: String,
    pub selection: SelectionSnapshot,
    pub changed_range: Option<TextRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineContext {
    pub number: usize, // 1-based
    pub from: usize,
    pub to: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionContext {
    pub anchor: usize,
    pub head: usize,
    pub from: usize,
    pub to: usize,
    pub empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordContext {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockLineRange {
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChange {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationSelection {
    pub anchor: usize,
    pub head: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOperation {
    pub changes: Vec<TextChange>,
    pub selection: Option<OperationSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandMode {
    Vim,
    Editor,
}

impl CommandMode {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSuggestion {
    pub value: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExecutionResult {
    pub message: String,
    pub operations: Vec<EditOperation>,
    pub clipboard_text: Option<String>,
    pub quit_requested: bool,
}
