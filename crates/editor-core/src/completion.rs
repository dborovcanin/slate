//! Completion semantics. Candidate data is supplied by the host; no database or popup state.
#[derive(Debug, Clone)]
pub struct VariableCompletionPrefix {
    pub from_col: usize,
    pub to_col: usize,
    pub query: String,
}

#[derive(Debug, Clone)]
pub struct VariableAutocompleteState {
    /// Column used to anchor the popup box visually (start of `[[` for cross-note,
    /// same as `from_col` for regular variables).
    pub popup_anchor_col: usize,
    pub from_col: usize,
    pub to_col: usize,
    pub query: String,
    pub suggestions: Vec<String>,
}
pub fn variable_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == ' '
}

pub fn extract_variable_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<VariableCompletionPrefix> {
    let chars = line_text.chars().collect::<Vec<_>>();
    let col = cursor_col.min(chars.len());
    let mut from = col;

    while from > 0 && variable_query_char(chars[from - 1]) {
        from -= 1;
    }

    while from < col && chars[from] == ' ' {
        from += 1;
    }

    if from >= col {
        return None;
    }

    let query = chars[from..col].iter().collect::<String>();
    if query.is_empty() || query.ends_with(' ') {
        return None;
    }

    Some(VariableCompletionPrefix {
        from_col: from,
        to_col: col,
        query,
    })
}

/// Candidate queries for a completion prefix, longest first: the whole run
/// (variable names may contain spaces, e.g. `tax ra` -> `tax rate`) and then
/// each shorter run starting after a space (`then pri` -> `pri`).
pub fn variable_completion_candidates(
    prefix: &VariableCompletionPrefix,
) -> Vec<VariableCompletionPrefix> {
    let chars: Vec<char> = prefix.query.chars().collect();
    let mut candidates = vec![VariableCompletionPrefix {
        from_col: prefix.from_col,
        to_col: prefix.to_col,
        query: prefix.query.clone(),
    }];
    for (idx, ch) in chars.iter().enumerate() {
        if *ch == ' ' && idx + 1 < chars.len() && chars[idx + 1] != ' ' {
            candidates.push(VariableCompletionPrefix {
                from_col: prefix.from_col + idx + 1,
                to_col: prefix.to_col,
                query: chars[idx + 1..].iter().collect(),
            });
        }
    }
    candidates
}

pub fn build_variable_suggestions(
    variable_names: &[String],
    query: &str,
    min_chars: usize,
    max_suggestions: usize,
) -> Vec<String> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.chars().count() < min_chars {
        return Vec::new();
    }

    let mut matches = variable_names
        .iter()
        .filter(|name| name.starts_with(&normalized_query) && **name != normalized_query)
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    matches.truncate(max_suggestions.max(1));
    matches
}

/// A `[[ID]].partial` reference being typed before the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossNoteCompletionPrefix {
    pub note_id: String,
    /// Character column of the opening `[[`, used to anchor the popup.
    pub bracket_col: usize,
    /// Character column where the typed variable name starts.
    pub from_col: usize,
    /// Typed variable name, trimmed and lowercased.
    pub partial: String,
}

pub fn extract_cross_note_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<CrossNoteCompletionPrefix> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Note ids as `markdown_tokens::is_note_link_id` accepts them.
        Regex::new(r"\[\[([A-Za-z0-9][A-Za-z0-9_-]{0,63})\]\]\.([A-Za-z0-9_][A-Za-z0-9_ ]*)?$")
            .unwrap()
    });

    let chars: Vec<char> = line_text.chars().collect();
    let col = cursor_col.min(chars.len());
    let text_before: String = chars[..col].iter().collect();
    let m = re.captures(&text_before)?;
    let full_match_start_byte = m.get(0)?.start();
    let bracket_col = text_before[..full_match_start_byte].chars().count();

    let note_id = m.get(1)?.as_str().to_string();
    let partial_raw = m.get(2).map(|g| g.as_str()).unwrap_or("");
    let partial = partial_raw.trim().to_lowercase();

    let partial_chars = partial_raw.chars().count();
    let from_col = col.saturating_sub(partial_chars);

    Some(CrossNoteCompletionPrefix {
        note_id,
        bracket_col,
        from_col,
        partial,
    })
}

pub fn parse_wiki_link_query(query: &str) -> Option<(&str, Option<&str>)> {
    if query.contains(']') || query.contains('|') {
        return None;
    }
    if let Some(hash_idx) = query.find('#') {
        let note_id = &query[..hash_idx];
        if !crate::markdown_tokens::is_note_link_id(note_id) {
            return None;
        }
        return Some((note_id, Some(&query[hash_idx + 1..])));
    }
    Some((query, None))
}

pub fn cross_note_suggestions<'a>(
    exports: impl Iterator<Item = (&'a str, &'a str)>,
    partial: &str,
) -> Vec<String> {
    exports
        .filter(|(normalized, _)| normalized.starts_with(partial) && *normalized != partial)
        .map(|(_, name)| name.to_string())
        .collect()
}
pub fn filter_wiki_suggestions<T: Clone>(
    suggestions: &[T],
    query: &str,
    title_lower: impl Fn(&T) -> &str,
) -> Vec<T> {
    let query = query.to_lowercase();
    suggestions
        .iter()
        .filter(|suggestion| title_lower(suggestion).contains(&query))
        .cloned()
        .collect()
}
pub fn local_variable_completion(
    line: &str,
    cursor_col: usize,
    table_enabled: bool,
    variables_enabled: bool,
    variable_names: &[String],
    min_chars: usize,
    max_suggestions: usize,
) -> Option<VariableAutocompleteState> {
    if table_enabled {
        if let Some(completion) =
            crate::calc_plan::table_formula_function_completion(line, cursor_col, min_chars)
        {
            // Helpers first, then variables sharing the typed prefix.
            let mut suggestions: Vec<String> = completion
                .suggestions
                .into_iter()
                .map(String::from)
                .collect();
            if variables_enabled {
                suggestions.extend(build_variable_suggestions(
                    variable_names,
                    &completion.query,
                    min_chars,
                    max_suggestions,
                ));
                suggestions.truncate(max_suggestions);
            }
            return Some(VariableAutocompleteState {
                popup_anchor_col: completion.from_col,
                from_col: completion.from_col,
                to_col: completion.to_col,
                query: completion.query,
                suggestions,
            });
        }
    }
    if !variables_enabled {
        return None;
    }

    if variable_names.is_empty() {
        return None;
    }
    let run = extract_variable_completion_prefix(line, cursor_col)?;
    let (prefix, suggestions) = variable_completion_candidates(&run)
        .into_iter()
        .map(|candidate| {
            let suggestions = build_variable_suggestions(
                variable_names,
                &candidate.query,
                min_chars,
                max_suggestions,
            );
            (candidate, suggestions)
        })
        .find(|(_, suggestions)| !suggestions.is_empty())?;
    Some(VariableAutocompleteState {
        popup_anchor_col: prefix.from_col,
        from_col: prefix.from_col,
        to_col: prefix.to_col,
        query: prefix.query,
        suggestions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefix_and_suffix_candidates_keep_character_columns() {
        let prefix = extract_variable_completion_prefix("é then pri", 10).unwrap();
        assert_eq!(prefix.from_col, 2);
        let candidates = variable_completion_candidates(&prefix);
        assert_eq!(
            candidates
                .iter()
                .map(|c| (c.from_col, c.query.as_str()))
                .collect::<Vec<_>>(),
            [(2, "then pri"), (7, "pri")]
        );
        assert!(extract_variable_completion_prefix("total ", 6).is_none());
        assert!(extract_variable_completion_prefix("é", 1).is_none());
    }
    #[test]
    fn local_completion_prefers_full_names_then_suffixes() {
        let names = vec!["tax rate".into(), "price".into()];
        let state = local_variable_completion("then pri", 8, false, true, &names, 3, 8).unwrap();
        assert_eq!(state.from_col, 5);
        assert_eq!(state.suggestions, ["price"]);
        let state = local_variable_completion("tax ra", 6, false, true, &names, 3, 8).unwrap();
        assert_eq!(state.from_col, 0);
        assert_eq!(state.suggestions, ["tax rate"]);
        assert!(local_variable_completion("pri", 3, false, false, &names, 3, 8).is_none());
    }
    #[test]
    fn suggestion_order_exact_exclusion_and_limits() {
        let names = vec!["total revenue".into(), "total".into(), "total cost".into()];
        assert!(build_variable_suggestions(&names, "to", 3, 8).is_empty());
        assert_eq!(
            build_variable_suggestions(&names, " TOT ", 3, 2),
            ["total", "total cost"]
        );
        assert_eq!(
            build_variable_suggestions(&names, "total", 3, 0),
            ["total cost"]
        );
    }
    #[test]
    fn formula_helpers_work_when_variables_are_disabled() {
        let line = "| :=sum |";
        let state = local_variable_completion(line, 7, true, false, &[], 3, 8).unwrap();
        assert_eq!(state.suggestions, ["sum_row()", "sum_col()"]);
        assert!(local_variable_completion(line, 7, false, false, &[], 3, 8).is_none());
    }
    #[test]
    fn qualified_prefix_and_export_matching_preserve_unicode_names() {
        let prefix = extract_cross_note_completion_prefix("é [[NOTE_1]].Tax ra", 19).unwrap();
        assert_eq!(
            prefix,
            CrossNoteCompletionPrefix {
                note_id: "NOTE_1".into(),
                bracket_col: 2,
                from_col: 13,
                partial: "tax ra".into(),
            }
        );
        assert_eq!(
            cross_note_suggestions(
                [
                    ("tax rate", "Tax Rate"),
                    ("tax ra", "Tax Ra"),
                    ("price", "Price")
                ]
                .into_iter(),
                "tax ra"
            ),
            ["Tax Rate"]
        );
        assert!(extract_cross_note_completion_prefix("[[bad id]].a", 12).is_none());
    }
    #[test]
    fn wiki_query_validation_and_filtering() {
        assert_eq!(
            parse_wiki_link_query("NOTE_1#Heading"),
            Some(("NOTE_1", Some("Heading")))
        );
        assert_eq!(parse_wiki_link_query("title"), Some(("title", None)));
        assert_eq!(parse_wiki_link_query("bad id#head"), None);
        assert_eq!(parse_wiki_link_query("NOTE|title"), None);
        assert_eq!(parse_wiki_link_query("NOTE]]"), None);
        let names = vec!["ärger".to_string(), "other".to_string()];
        assert_eq!(
            filter_wiki_suggestions(&names, "ÄR", String::as_str),
            ["ärger"]
        );
        assert_eq!(filter_wiki_suggestions(&names, "", String::as_str), names);
    }
}
