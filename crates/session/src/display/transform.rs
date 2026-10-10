//! Explicit substitutions used for formula results, resolved links and media.
use super::mapping::{scalar_to_byte, MappedLineBuilder, Provenance};
use std::ops::Range;
/// Replacement text is generated and noneditable, owned by its source span.
pub struct Replacement<'a> {
    pub source: Range<usize>,
    pub text: &'a str,
}
pub fn substitute(text: &str, replacements: &[Replacement<'_>]) -> MappedLineBuilder {
    let len = text.chars().count();
    let mut out = MappedLineBuilder::new(len);
    let mut cursor = 0;
    for replacement in replacements {
        assert!(replacement.source.start >= cursor && replacement.source.end <= len);
        out.push(
            &text[scalar_to_byte(text, cursor)..scalar_to_byte(text, replacement.source.start)],
            Provenance::Copied(cursor..replacement.source.start),
        );
        out.push(
            replacement.text,
            Provenance::Owned(replacement.source.clone()),
        );
        cursor = replacement.source.end;
    }
    out.push(
        &text[scalar_to_byte(text, cursor)..],
        Provenance::Copied(cursor..len),
    );
    out
}
#[cfg(test)]
mod tests {
    use super::super::mapping::Affinity;
    use super::*;
    #[test]
    fn unicode_formula_values_keep_exact_owners() {
        let line = substitute(
            "é =SUM(A) 終",
            &[Replacement {
                source: 2..9,
                text: "12*",
            }],
        );
        assert_eq!(line.text, "é 12* 終");
        let hit = line.map.display_to_source(3, Affinity::After).unwrap();
        assert_eq!(hit.owner, Some(2..9));
        assert_eq!(hit.caret, None);
        assert_eq!(line.map.source_to_display(10, Affinity::After), Some(6));
    }
}
