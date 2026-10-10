//! In-note matching with character offsets, including Unicode case expansion.
pub fn case_insensitive_matches(line: &str, query_lower: &str) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    if query_lower.is_empty() {
        return matches;
    }
    if line.is_ascii() && query_lower.is_ascii() {
        // ASCII lowercasing keeps byte offsets, which are char offsets here.
        let lower = line.to_ascii_lowercase();
        let mut start = 0;
        while let Some(pos) = lower[start..].find(query_lower) {
            let from = start + pos;
            matches.push((from, from + query_lower.len()));
            start = from + query_lower.len();
        }
        return matches;
    }
    // Lowercasing can change a char's length (`İ` becomes two chars), so
    // keep, for every lowered char, the index of the char it came from.
    let query: Vec<char> = query_lower.chars().collect();
    let lowered: Vec<(char, usize)> = line
        .chars()
        .enumerate()
        .flat_map(|(idx, ch)| ch.to_lowercase().map(move |lower| (lower, idx)))
        .collect();
    let mut start = 0;
    while start + query.len() <= lowered.len() {
        let window = &lowered[start..start + query.len()];
        if window.iter().map(|(ch, _)| *ch).eq(query.iter().copied()) {
            matches.push((window[0].1, window[query.len() - 1].1 + 1));
            start += query.len();
        } else {
            start += 1;
        }
    }
    matches
}

/// Fills the host's reusable match vector, preserving non-overlapping line order.
pub fn find_matches(lines: &[String], query_lower: &str, matches: &mut Vec<(usize, usize, usize)>) {
    matches.clear();
    for (line_idx, line) in lines.iter().enumerate() {
        matches.extend(
            case_insensitive_matches(line, query_lower)
                .into_iter()
                .map(|(from, to)| (line_idx, from, to)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_case_expansion_returns_original_character_ranges() {
        assert_eq!(case_insensitive_matches("İİx", "x"), [(2, 3)]);
        assert_eq!(case_insensitive_matches("ÄBc äbC", "äb"), [(0, 2), (4, 6)]);
        assert_eq!(case_insensitive_matches("🙂É🙂é", "é"), [(1, 2), (3, 4)]);
    }
    #[test]
    fn matches_are_non_overlapping_and_empty_query_clears_previous_results() {
        assert_eq!(case_insensitive_matches("aaaa", "aa"), [(0, 2), (2, 4)]);
        let lines = vec!["Total total".into(), "İİx".into()];
        let mut matches = vec![(99, 99, 99)];
        find_matches(&lines, "total", &mut matches);
        assert_eq!(matches, [(0, 0, 5), (0, 6, 11)]);
        find_matches(&lines, "x", &mut matches);
        assert_eq!(matches, [(1, 2, 3)]);
        find_matches(&lines, "", &mut matches);
        assert!(matches.is_empty());
    }
}
