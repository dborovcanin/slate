use super::types::{EditOperation, OperationSelection, TextChange};

pub fn single_change(change: TextChange, selection: Option<OperationSelection>) -> EditOperation {
    EditOperation {
        changes: vec![change],
        selection,
    }
}

pub fn replace_range(
    from: usize,
    to: usize,
    insert: impl Into<String>,
    selection: Option<OperationSelection>,
) -> EditOperation {
    single_change(
        TextChange {
            from,
            to,
            insert: insert.into(),
        },
        selection,
    )
}
