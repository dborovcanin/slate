use aho_corasick::AhoCorasick;
use std::cell::OnceCell;

fn is_variable_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn has_variable_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let left_ok = start == 0 || !is_variable_word_byte(bytes[start - 1]);
    let right_ok = end == bytes.len() || !is_variable_word_byte(bytes[end]);
    left_ok && right_ok
}

/// Calc variable names plus a matcher for highlighting them, built on first
/// use. A note can define thousands of variables, so rendered lines must not
/// be scanned once per name.
#[derive(Default)]
pub struct VariableNames {
    names: Vec<String>,
    matcher: OnceCell<Option<AhoCorasick>>,
    /// Revision of the source the names were derived from, when known.
    source_revision: Option<u64>,
}

impl VariableNames {
    pub fn new(names: Vec<String>) -> Self {
        Self {
            names,
            matcher: OnceCell::new(),
            source_revision: None,
        }
    }

    /// `set`, recording that the names were derived from source `revision`.
    pub fn set_from_revision(&mut self, names: Vec<String>, revision: u64) {
        self.set(names);
        self.source_revision = Some(revision);
    }

    /// The revision passed to the last `set_from_revision`, while the names
    /// have not changed since.
    pub fn source_revision(&self) -> Option<u64> {
        self.source_revision
    }

    /// Replaces the names, keeping the built matcher when they are unchanged
    /// (the common case for recomputes triggered by edits).
    pub fn set(&mut self, names: Vec<String>) {
        if names != self.names {
            *self = Self::new(names);
        }
    }

    pub fn clear(&mut self) {
        self.set(Vec::new());
    }

    pub fn shrink_to_fit(&mut self) {
        self.names.shrink_to_fit();
    }

    fn matcher(&self) -> Option<&AhoCorasick> {
        self.matcher
            .get_or_init(|| {
                let needles: Vec<&str> = self
                    .names
                    .iter()
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                    .collect();
                if needles.is_empty() {
                    return None;
                }
                AhoCorasick::builder()
                    .ascii_case_insensitive(true)
                    .build(needles)
                    .ok()
            })
            .as_ref()
    }

    /// Byte ranges of variable names in `text`, matched ASCII
    /// case-insensitively on word boundaries. Left-most matches win, and the
    /// longer name wins among matches that start together.
    pub fn find_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        if text.is_empty() {
            return Vec::new();
        }
        let Some(matcher) = self.matcher() else {
            return Vec::new();
        };

        let bytes = text.as_bytes();
        let mut matches: Vec<(usize, usize)> = matcher
            .find_overlapping_iter(text)
            .map(|found| (found.start(), found.end()))
            .filter(|&(start, end)| has_variable_word_boundaries(bytes, start, end))
            .collect();

        if matches.len() <= 1 {
            return matches;
        }

        matches.sort_by(|a, b| a.0.cmp(&b.0).then((b.1 - b.0).cmp(&(a.1 - a.0))));

        let mut deduped: Vec<(usize, usize)> = Vec::new();
        for candidate in matches {
            if deduped.last().is_some_and(|last| candidate.0 < last.1) {
                continue;
            }
            deduped.push(candidate);
        }
        deduped
    }
}

impl std::ops::Deref for VariableNames {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.names
    }
}

impl From<Vec<String>> for VariableNames {
    fn from(names: Vec<String>) -> Self {
        Self::new(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_ranges_fall_back_to_shorter_name_at_word_boundary() {
        let names = VariableNames::new(vec![
            "tax".to_string(),
            " Tax Rate ".to_string(),
            String::new(),
        ]);
        assert_eq!(names.find_ranges("tax rates TAX"), vec![(0, 3), (10, 13)]);
        assert_eq!(names.find_ranges("x = tax rate"), vec![(4, 12)]);
        assert_eq!(names.find_ranges("syntax"), Vec::new());
    }

    #[test]
    fn variable_names_set_keeps_matcher_for_unchanged_names() {
        let mut names = VariableNames::new(vec!["a".to_string()]);
        assert!(names.matcher().is_some());
        names.set(vec!["a".to_string()]);
        assert!(names.matcher.get().is_some());
        names.set(vec!["b".to_string()]);
        assert!(names.matcher.get().is_none());
        names.clear();
        assert!(names.matcher().is_none());
    }
}
