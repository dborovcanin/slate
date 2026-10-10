//! Shared formula result and error presentation.
use std::borrow::Cow;
pub fn table_error_text(
    error_kind: Option<&app_core::calc::TableCellErrorKind>,
    value: &str,
) -> Option<String> {
    match error_kind {
        Some(app_core::calc::TableCellErrorKind::OutOfBounds) => {
            Some(String::from("table error: out_of_bounds"))
        }
        Some(app_core::calc::TableCellErrorKind::NonNumeric) => {
            Some(String::from("table error: non_numeric"))
        }
        Some(app_core::calc::TableCellErrorKind::SelfReference) => {
            Some(String::from("table error: self_reference"))
        }
        Some(app_core::calc::TableCellErrorKind::Cycle) => Some(String::from("table error: cycle")),
        Some(app_core::calc::TableCellErrorKind::Unknown) => {
            if let Some(code) = value.strip_prefix("!ERROR#") {
                Some(format!("table error: {code}"))
            } else {
                Some(String::from("table error: unknown"))
            }
        }
        None => None,
    }
}

pub fn masked_formula_value<'a>(value: &'a str, is_error: bool) -> Cow<'a, str> {
    // Keep table columns aligned while avoiding long `!ERROR#...` payloads
    // that get awkwardly cut inside narrow cells.
    if is_error {
        Cow::Borrowed("!ERROR")
    } else {
        Cow::Borrowed(value)
    }
}

/// Value shown for a formula cell: its formatted result, or `…` while it
/// is still being computed.
pub fn formula_cell_value(eval: Option<&app_core::calc::TableCellEvaluation>) -> String {
    eval.map(|entry| super::table::format_formula_display_value(&entry.value))
        .unwrap_or_else(|| String::from("…"))
}

/// Text a formula cell shows while not focused: its value followed by its
/// `*` marker.
pub fn resting_formula_cell_text(
    eval: Option<&app_core::calc::TableCellEvaluation>,
    marker: &str,
) -> String {
    let value = formula_cell_value(eval);
    let has_error = eval.and_then(|entry| entry.error_kind.as_ref()).is_some();
    let masked = masked_formula_value(&value, has_error);
    let mut out = String::with_capacity(masked.len() + marker.len());
    out.push_str(&masked);
    out.push_str(marker);
    out
}
