//! Icon glyphs for lists and the collection browser, in three sets: Nerd
//! Font glyphs, plain Unicode symbols and ASCII. Every glyph is one cell wide
//! so rows stay aligned whichever set is active.

use app_core::config::IconStyle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icons {
    /// "All notes" entry.
    pub library: &'static str,
    /// "Unsorted" entry (notes in no collection).
    pub inbox: &'static str,
    pub collection: &'static str,
    pub collection_open: &'static str,
    pub note: &'static str,
    pub daily_note: &'static str,
    pub locked: &'static str,
    pub encrypted: &'static str,
    /// The note open in the editor.
    pub active: &'static str,
    /// The session working collection.
    pub working: &'static str,
    pub copied: &'static str,
    pub cut: &'static str,
    pub marked: &'static str,
    pub separator: &'static str,
    pub filter: &'static str,
    pub clock: &'static str,
    /// Creation date.
    pub added: &'static str,
    pub tag: &'static str,
}

const NERD: Icons = Icons {
    library: "\u{f02d}",         // nf-fa-book
    inbox: "\u{f01c}",           // nf-fa-inbox
    collection: "\u{f07b}",      // nf-fa-folder
    collection_open: "\u{f07c}", // nf-fa-folder_open
    note: "\u{f15c}",            // nf-fa-file_text
    daily_note: "\u{f073}",      // nf-fa-calendar
    locked: "\u{f023}",          // nf-fa-lock
    encrypted: "\u{f132}",       // nf-fa-shield
    active: "\u{f040}",          // nf-fa-pencil
    working: "\u{f08d}",         // nf-fa-thumb_tack
    copied: "\u{f0c5}",          // nf-fa-copy
    cut: "\u{f0c4}",             // nf-fa-scissors
    marked: "\u{f00c}",          // nf-fa-check
    separator: "\u{f105}",       // nf-fa-angle_right
    filter: "\u{f002}",          // nf-fa-search
    clock: "\u{f017}",           // nf-fa-clock_o
    added: "\u{f055}",           // nf-fa-plus_circle
    tag: "\u{f02b}",             // nf-fa-tag
};

const UNICODE: Icons = Icons {
    library: "◈",
    inbox: "◇",
    collection: "▸",
    collection_open: "▾",
    note: "≡",
    daily_note: "◷",
    locked: "⊠",
    encrypted: "⊗",
    active: "✎",
    working: "★",
    copied: "+",
    cut: "✂",
    marked: "✓",
    separator: "›",
    filter: "⌕",
    clock: "◔",
    added: "+",
    tag: "#",
};

const ASCII: Icons = Icons {
    library: "*",
    inbox: "?",
    collection: "+",
    collection_open: "-",
    note: "-",
    daily_note: "d",
    locked: "L",
    encrypted: "E",
    active: ">",
    working: "*",
    copied: "+",
    cut: "x",
    marked: "v",
    separator: ">",
    filter: "/",
    clock: "@",
    added: "+",
    tag: "#",
};

impl Icons {
    pub fn for_style(style: IconStyle) -> &'static Icons {
        match style {
            IconStyle::Nerd => &NERD,
            IconStyle::Unicode => &UNICODE,
            IconStyle::Ascii => &ASCII,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn every_glyph_is_one_cell_wide() {
        for style in [IconStyle::Nerd, IconStyle::Unicode, IconStyle::Ascii] {
            let icons = Icons::for_style(style);
            for glyph in [
                icons.library,
                icons.inbox,
                icons.collection,
                icons.collection_open,
                icons.note,
                icons.daily_note,
                icons.locked,
                icons.encrypted,
                icons.active,
                icons.working,
                icons.copied,
                icons.cut,
                icons.marked,
                icons.separator,
                icons.filter,
                icons.clock,
                icons.added,
                icons.tag,
            ] {
                assert_eq!(glyph.width(), 1, "{style:?} glyph {glyph:?}");
            }
        }
    }
}
