// Pure logic lives in the editor-core crate. Re-export so terminal code can use
// `crate::editor_core::*` paths.
pub use editor_core::buffer;
pub use editor_core::calc_plan;
pub use editor_core::command_catalog;
pub use editor_core::command_history;
pub use editor_core::commands;
pub use editor_core::completion;
pub use editor_core::context;
pub use editor_core::engine;
pub use editor_core::folding;
pub use editor_core::format;
pub use editor_core::history;
pub use editor_core::markdown_tokens;
pub use editor_core::math_commands;
pub use editor_core::operations;
pub use editor_core::scripts;
pub use editor_core::search;
pub use editor_core::substitute;
pub use editor_core::sum;
pub use editor_core::table;
pub use editor_core::table_import;
pub use editor_core::text_rules;
pub use editor_core::types;
pub use editor_core::vim;
pub use editor_core::vim_actions;

#[cfg(test)]
mod tests {
    use super::commands::{insert_value_at_selection, list_command_suggestions};
    use super::context::ResolvedContext;
    use super::types::{CommandMode, EditorContextSnapshot, SelectionSnapshot};

    #[test]
    fn module_smoke_test() {
        let snapshot = EditorContextSnapshot {
            text: "1 + 2".to_string(),
            selection: SelectionSnapshot { anchor: 0, head: 0 },
            changed_range: None,
        };
        let ctx = ResolvedContext::new(snapshot.clone());
        assert_eq!(ctx.current_line().text, "1 + 2");

        let suggestions = list_command_suggestions(CommandMode::Editor, "su");
        assert!(suggestions.iter().any(|entry| entry.value == "sum"));

        let operation = insert_value_at_selection(&snapshot, "3");
        assert_eq!(operation.changes.len(), 1);
        assert_eq!(operation.changes[0].insert, "3");
    }
}
