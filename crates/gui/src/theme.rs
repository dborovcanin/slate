//! Colors from the design canvas, and the mapping from the session's semantic
//! styles to GPUI text styles. Semantics stay in `note_session::display`; this
//! file only decides what each role looks like.
use gpui::{
    px, rgb, FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle,
};
use note_session::display::semantic::{SemanticRole, SemanticStyle};

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Hsla,
    pub panel: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub heading: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    /// Calc results and formula values.
    pub amber: Hsla,
    /// Variables, links and the Normal mode pill.
    pub blue: Hsla,
    pub chip: Hsla,
    pub cursorline: Hsla,
    pub active: Hsla,
    pub on_accent: Hsla,
    pub code_bg: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub comment: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            bg: c(0x15171a),
            panel: c(0x1b1e22),
            border: c(0x2a2e35),
            text: c(0xe4e2dc),
            heading: c(0xf3f1ea),
            muted: c(0x9aa0a8),
            faint: c(0x6b717a),
            amber: c(0xe3a857),
            blue: c(0x7fa6d9),
            chip: c(0x262a30),
            cursorline: c(0x1f242b),
            active: c(0x252b33),
            on_accent: c(0x11161d),
            code_bg: c(0x1b1e22),
            keyword: c(0xc792ea),
            string: c(0xa5c58a),
            comment: c(0x6b717a),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: c(0xfbfaf7),
            panel: c(0xf2f0eb),
            border: c(0xdedad2),
            text: c(0x23262b),
            heading: c(0x111317),
            muted: c(0x555b64),
            faint: c(0x7d828a),
            amber: c(0x94560a),
            blue: c(0x2c5c99),
            chip: c(0xe7e4dd),
            cursorline: c(0xeef2f7),
            active: c(0xe4e9f0),
            on_accent: c(0xffffff),
            code_bg: c(0xf2f0eb),
            keyword: c(0x7a3e9d),
            string: c(0x3f6b21),
            comment: c(0x7d828a),
        }
    }

    pub fn role(&self, role: SemanticRole) -> Hsla {
        match role {
            SemanticRole::Text | SemanticRole::Surface => self.text,
            SemanticRole::Heading => self.heading,
            SemanticRole::HiddenMarker => self.faint,
            SemanticRole::CalcResult => self.amber,
            SemanticRole::CodeBlock => self.code_bg,
            SemanticRole::Variable | SemanticRole::Primary => self.blue,
            SemanticRole::CodeKeyword | SemanticRole::CodeType => self.keyword,
            SemanticRole::CodeString => self.string,
            SemanticRole::CodeNumber | SemanticRole::CodeFunction => self.amber,
            SemanticRole::CodeComment => self.comment,
            SemanticRole::SearchMatch | SemanticRole::SearchCurrent => self.amber,
        }
    }

    /// Text style for one run. Surface backgrounds are left to the line.
    pub fn highlight(&self, style: SemanticStyle) -> HighlightStyle {
        let mut color = style.fg.map(|r| self.role(r)).unwrap_or(self.text);
        if style.dim {
            color = self.faint;
        }
        let background_color = match style.bg {
            Some(SemanticRole::Surface) | None => None,
            Some(SemanticRole::CodeBlock) => None,
            Some(role) => Some(self.role(role).opacity(0.25)),
        };
        HighlightStyle {
            color: Some(color),
            font_weight: style.bold.then_some(FontWeight::SEMIBOLD),
            font_style: style.italic.then_some(FontStyle::Italic),
            background_color,
            underline: style.underline.then(|| UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: style.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
            fade_out: None,
        }
    }
}
