pub mod commands;
pub mod context;
pub mod operations;
pub mod text_rules;
pub mod types;

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
