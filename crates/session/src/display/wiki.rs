//! Wiki links display as the title of the note they point to: `[[id]]`
//! becomes `Title`, and `[[id#Heading]]` becomes `Title#Heading`. A link to a
//! note that no longer exists shows as `?`. Front ends supply the title
//! lookup; positions are Unicode scalar columns.
use super::mapping::{MappedLineBuilder, Provenance, SourceDisplayMap};
use editor_core::markdown_tokens::find_wiki_link_matches;

/// A line with its wiki links replaced by titles.
pub struct WikiDisplay {
    pub text: String,
    /// Display columns `(from, to)` of each replaced link, to underline.
    pub underline: Vec<(usize, usize)>,
    /// Source columns to display columns; the replaced links are owned
    /// spans, so a click on a title resolves to the whole link.
    pub map: SourceDisplayMap,
}

/// `resolve` returns a note's title by id, or `None` when it is gone. Links
/// with their own alt text (`[[id|text]]`) are left to the markdown display.
/// `None` when the line has nothing to replace.
pub fn render_wiki_links(
    line: &str,
    resolve: &mut dyn FnMut(&str) -> Option<String>,
) -> Option<WikiDisplay> {
    let links = find_wiki_link_matches(line);
    if links.is_empty() {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = MappedLineBuilder::new(chars.len());
    let mut underline = Vec::new();
    let mut cursor = 0usize;
    let mut replaced = false;
    for link in links {
        if link.title.is_some() || link.from < cursor || link.to > chars.len() {
            continue;
        }
        if link.from > cursor {
            let kept: String = chars[cursor..link.from].iter().collect();
            out.push(&kept, Provenance::Copied(cursor..link.from));
        }
        let base = match resolve(&link.note_id) {
            Some(title) if title.trim().is_empty() => "Untitled".to_string(),
            Some(title) => title,
            None => "?".to_string(),
        };
        let display = match link.heading.as_deref().filter(|h| !h.is_empty()) {
            Some(heading) => format!("{base}#{heading}"),
            None => base,
        };
        let start = out.map.display_len;
        out.push(&display, Provenance::Owned(link.from..link.to));
        underline.push((start, out.map.display_len));
        cursor = link.to;
        replaced = true;
    }
    if !replaced {
        return None;
    }
    if cursor < chars.len() {
        let rest: String = chars[cursor..].iter().collect();
        out.push(&rest, Provenance::Copied(cursor..chars.len()));
    }
    Some(WikiDisplay {
        text: out.text,
        underline,
        map: out.map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::mapping::Affinity;

    fn titles(id: &str) -> Option<String> {
        match id {
            "A" => Some("Alpha".into()),
            "B" => Some("   ".into()),
            _ => None,
        }
    }

    #[test]
    fn links_show_the_target_title() {
        let d = render_wiki_links("see [[A]] now", &mut titles).expect("link");
        assert_eq!(d.text, "see Alpha now");
        assert_eq!(d.underline, vec![(4, 9)]);
        assert_eq!(d.map.source_len, 13);
        assert_eq!(d.map.display_len, 13);
    }

    #[test]
    fn headings_blank_titles_and_broken_links() {
        let d = render_wiki_links("[[A#Plan]] [[B]] [[ZZ]]", &mut titles).expect("links");
        assert_eq!(d.text, "Alpha#Plan Untitled ?");
        assert_eq!(d.underline.len(), 3);
    }

    #[test]
    fn lines_without_links_are_left_alone() {
        assert!(render_wiki_links("no links, just [brackets]", &mut titles).is_none());
    }

    #[test]
    fn a_click_inside_a_title_resolves_to_the_whole_link() {
        let line = "see [[A]] now";
        let d = render_wiki_links(line, &mut titles).expect("link");
        // Display column 6 is inside "Alpha"; the link spans source 4..9.
        let hit = d.map.display_to_source(6, Affinity::After).expect("hit");
        assert_eq!(hit.owner, Some(4..9));
        // Text outside the link maps one to one.
        let hit = d.map.display_to_source(11, Affinity::After).expect("hit");
        assert_eq!(hit.caret, Some(9 + 2));
    }
}
