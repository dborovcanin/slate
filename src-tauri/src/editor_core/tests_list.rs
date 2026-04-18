#[cfg(test)]
mod tests_list {
    use crate::editor_core::context::ResolvedContext;
    use crate::editor_core::text_rules::{run_doc_change_rules, TextRuleOptions};
    use crate::editor_core::types::{EditorContextSnapshot, SelectionSnapshot};

    #[test]
    fn test_list_renumber() {
        let text = "4. sasas\n5. dsds\n6. sa\n7. sas\n8. sasa\n9. sas\n7. sas\n8. sas";
        let ctx = ResolvedContext::new(EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor: 0, head: 0 },
            changed_range: None,
        });
        let op = run_doc_change_rules(
            &ctx,
            TextRuleOptions {
                markdown_autoformat: true,
                ..TextRuleOptions::default()
            },
        );
        if let Some(op) = op {
            println!("Changes: {:?}", op.changes);
        } else {
            println!("No changes generated!");
        }
    }
}
