use crate::editor_core::markdown_tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSourceKind {
    Image,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSourceMatch {
    pub kind: MediaSourceKind,
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub src: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaLineTransform {
    pub rendered_line: String,
    pub mapped_cursor_col: Option<usize>,
    pub changed: bool,
}

pub fn find_media_sources(text: &str) -> Vec<MediaSourceMatch> {
    let has_image_hint = text.contains("![") && text.contains("](");
    let has_audio_hint = has_potential_audio_link(text);
    if !has_image_hint && !has_audio_hint {
        return Vec::new();
    }

    let mut matches = Vec::new();
    if has_image_hint {
        for image in markdown_tokens::find_markdown_image_matches(text) {
            matches.push(MediaSourceMatch {
                kind: MediaSourceKind::Image,
                from: image.from,
                to: image.to,
                label: normalize_media_label(&image.alt, &image.src, "Image"),
                src: image.src,
            });
        }
    }
    if has_audio_hint {
        matches.extend(find_markdown_audio_sources(text));
    }
    matches
}

pub fn collapse_media_sources_for_display(
    text: &str,
    active_cursor_col: Option<usize>,
) -> MediaLineTransform {
    let media_sources = find_media_sources(text);
    if media_sources.is_empty() {
        return MediaLineTransform {
            rendered_line: text.to_string(),
            mapped_cursor_col: active_cursor_col,
            changed: false,
        };
    }

    let chars: Vec<char> = text.chars().collect();
    let mut sorted = media_sources;
    sorted.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));

    let mut rendered = String::with_capacity(text.len() + 24);
    let mut cursor = 0usize;
    let mut rendered_char_count = 0usize;
    let mut mapped_cursor_col = active_cursor_col;
    let mut changed = false;

    for media in sorted {
        if media.from < cursor || media.to <= media.from || media.to > chars.len() {
            continue;
        }

        push_char_slice(
            &mut rendered,
            &chars,
            cursor,
            media.from,
            &mut rendered_char_count,
        );

        // Treat media span as half-open [from, to): at the right boundary we
        // are already outside, which avoids source/preview flicker while
        // vertical-scrolling across media lines.
        let cursor_inside = active_cursor_col
            .map(|col| col >= media.from && col < media.to)
            .unwrap_or(false);

        if cursor_inside {
            push_char_slice(
                &mut rendered,
                &chars,
                media.from,
                media.to,
                &mut rendered_char_count,
            );
        } else {
            let placeholder = media_placeholder_label(media.kind, &media.label);
            let placeholder_len = placeholder.chars().count();
            rendered.push_str(&placeholder);
            rendered_char_count += placeholder_len;
            changed = true;

            if let Some(col) = mapped_cursor_col {
                if col > media.to {
                    let removed_len = media.to.saturating_sub(media.from);
                    mapped_cursor_col = Some(col - removed_len + placeholder_len);
                } else if col > media.from {
                    mapped_cursor_col = Some(rendered_char_count);
                }
            }
        }

        cursor = media.to;
    }

    if cursor == 0 {
        return MediaLineTransform {
            rendered_line: text.to_string(),
            mapped_cursor_col: active_cursor_col,
            changed: false,
        };
    }

    push_char_slice(
        &mut rendered,
        &chars,
        cursor,
        chars.len(),
        &mut rendered_char_count,
    );

    if !changed {
        return MediaLineTransform {
            rendered_line: text.to_string(),
            mapped_cursor_col: active_cursor_col,
            changed: false,
        };
    }

    MediaLineTransform {
        rendered_line: rendered,
        mapped_cursor_col,
        changed: true,
    }
}

fn push_char_slice(
    out: &mut String,
    chars: &[char],
    from: usize,
    to: usize,
    rendered_char_count: &mut usize,
) {
    if from >= to {
        return;
    }
    out.extend(chars[from..to].iter());
    *rendered_char_count += to - from;
}

fn media_placeholder_label(kind: MediaSourceKind, label: &str) -> String {
    let kind_label = match kind {
        MediaSourceKind::Image => "image",
        MediaSourceKind::Audio => "audio",
    };
    format!("[{kind_label}: {label}]")
}

fn normalize_media_label(raw_label: &str, src: &str, fallback: &str) -> String {
    let trimmed = raw_label.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let from_src = src.trim().rsplit('/').next().unwrap_or("").trim();
    if !from_src.is_empty() {
        return from_src.to_string();
    }
    fallback.to_string()
}

fn has_potential_audio_link(text: &str) -> bool {
    if !text.contains("](") {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    lower.contains("data:audio/")
        || lower.contains(".mp3")
        || lower.contains(".wav")
        || lower.contains(".ogg")
        || lower.contains(".m4a")
        || lower.contains(".flac")
        || lower.contains(".aac")
        || lower.contains(".opus")
        || lower.contains(".weba")
        || lower.contains(".aiff")
        || lower.contains(".oga")
}

fn find_markdown_audio_sources(text: &str) -> Vec<MediaSourceMatch> {
    let tokens = markdown_tokens::tokenize_inline_markdown(text);
    let mut matches = Vec::new();
    let mut open_idx: Option<usize> = None;
    let mut marker_count = 0usize;
    let mut label = String::new();
    let mut src = String::new();

    for (idx, token) in tokens.iter().enumerate() {
        match token.kind {
            markdown_tokens::InlineTokenType::LinkMarker => {
                if open_idx.is_none() {
                    open_idx = Some(idx);
                    marker_count = 1;
                    label.clear();
                    src.clear();
                    continue;
                }
                marker_count += 1;
                if marker_count >= 3 {
                    let Some(start_idx) = open_idx.take() else {
                        continue;
                    };
                    let start = tokens[start_idx].from;
                    let end = token.to;
                    if !src.is_empty() && is_audio_source(&src) {
                        matches.push(MediaSourceMatch {
                            kind: MediaSourceKind::Audio,
                            from: start,
                            to: end,
                            label: normalize_media_label(&label, &src, "Audio"),
                            src: src.clone(),
                        });
                    }
                    marker_count = 0;
                }
            }
            markdown_tokens::InlineTokenType::LinkText => {
                if open_idx.is_some() {
                    label = text
                        .chars()
                        .skip(token.from)
                        .take(token.to.saturating_sub(token.from))
                        .collect();
                }
            }
            markdown_tokens::InlineTokenType::LinkUrl => {
                if open_idx.is_some() {
                    src = text
                        .chars()
                        .skip(token.from)
                        .take(token.to.saturating_sub(token.from))
                        .collect();
                }
            }
            _ => {}
        }
    }

    matches
}

fn is_audio_source(raw_src: &str) -> bool {
    let src = raw_src.trim().to_ascii_lowercase();
    if src.starts_with("data:audio/") {
        return true;
    }
    let path_only = src
        .split('#')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    let extension = path_only.rsplit('.').next().unwrap_or("");
    matches!(
        extension,
        "mp3" | "wav" | "ogg" | "m4a" | "flac" | "aac" | "opus" | "weba" | "aiff" | "oga"
    )
}

#[cfg(test)]
mod tests {
    use super::{collapse_media_sources_for_display, find_media_sources, MediaSourceKind};

    #[test]
    fn find_media_sources_detects_markdown_images() {
        let line = "hello ![diagram](./assets/plan.png) world";
        let matches = find_media_sources(line);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, MediaSourceKind::Image);
        assert_eq!(matches[0].label, "diagram");
        assert_eq!(matches[0].src, "./assets/plan.png");
    }

    #[test]
    fn collapse_media_sources_for_display_replaces_image_with_placeholder() {
        let line = "hello ![diagram](./assets/plan.png) world";
        let transformed = collapse_media_sources_for_display(line, None);
        assert!(transformed.changed);
        assert_eq!(transformed.rendered_line, "hello [image: diagram] world");
        assert_eq!(transformed.mapped_cursor_col, None);
    }

    #[test]
    fn collapse_media_sources_for_display_uses_source_name_when_alt_is_empty() {
        let line = "![ ](./assets/cover.png)";
        let transformed = collapse_media_sources_for_display(line, None);
        assert_eq!(transformed.rendered_line, "[image: cover.png]");
    }

    #[test]
    fn collapse_media_sources_for_display_reveals_image_when_cursor_inside() {
        let line = "![diagram](./assets/plan.png)";
        let transformed = collapse_media_sources_for_display(line, Some(10));
        assert!(!transformed.changed);
        assert_eq!(transformed.rendered_line, line);
        assert_eq!(transformed.mapped_cursor_col, Some(10));
    }

    #[test]
    fn collapse_media_sources_for_display_hides_image_source_at_right_boundary() {
        let line = "![diagram](./assets/plan.png)";
        let boundary = line.chars().count();
        let transformed = collapse_media_sources_for_display(line, Some(boundary));
        assert!(transformed.changed);
        assert_eq!(transformed.rendered_line, "[image: diagram]");
        assert_eq!(transformed.mapped_cursor_col, Some("[image: diagram]".chars().count()));
    }

    #[test]
    fn collapse_media_sources_for_display_remaps_cursor_after_placeholder() {
        let line = "a ![diagram](./assets/plan.png) z";
        let transformed = collapse_media_sources_for_display(line, Some(line.chars().count()));
        assert!(transformed.changed);
        assert_eq!(transformed.rendered_line, "a [image: diagram] z");
        assert_eq!(transformed.mapped_cursor_col, Some(20));
    }

    #[test]
    fn find_media_sources_detects_audio_links() {
        let line = "listen [theme](./audio/theme.mp3)";
        let matches = find_media_sources(line);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, MediaSourceKind::Audio);
        assert_eq!(matches[0].label, "theme");
    }

    #[test]
    fn collapse_media_sources_for_display_replaces_audio_link_with_placeholder() {
        let line = "listen [theme](./audio/theme.mp3)";
        let transformed = collapse_media_sources_for_display(line, None);
        assert!(transformed.changed);
        assert_eq!(transformed.rendered_line, "listen [audio: theme]");
    }

    #[test]
    fn collapse_media_sources_for_display_keeps_regular_link_unchanged() {
        let line = "docs [guide](./docs/guide.md)";
        let transformed = collapse_media_sources_for_display(line, None);
        assert!(!transformed.changed);
        assert_eq!(transformed.rendered_line, line);
    }
}
