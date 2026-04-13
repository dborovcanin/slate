use crate::text_rules::format_table_lines;

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn normalize_list_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    // Unordered: normalize `*` and `+` → `-`, ensure single space after marker
    if let Some(rest) = trimmed.strip_prefix("* ").or_else(|| trimmed.strip_prefix("+ ")) {
        return format!("{}- {}", indent, rest.trim_start());
    }
    if trimmed == "*" || trimmed == "+" {
        return format!("{}-", indent);
    }

    // Ordered: normalize extra spaces between marker and content ("1.   foo" → "1. foo")
    if let Some(dot_pos) = trimmed.find(|c: char| c == '.') {
        let marker = &trimmed[..=dot_pos];
        let is_numeric = marker[..dot_pos].chars().all(|c| c.is_ascii_digit()) && !marker.is_empty();
        if is_numeric {
            let after_marker = &trimmed[dot_pos + 1..];
            if after_marker.starts_with(char::is_whitespace) {
                let content = after_marker.trim_start();
                return format!("{}{}. {}", indent, &marker[..dot_pos], content);
            }
        }
    }

    line.to_string()
}

pub fn format_markdown(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(|s| s.trim_end().to_string()).collect();

    // Process table blocks
    let mut i = 0;
    while i < lines.len() {
        if is_table_row(&lines[i]) {
            let start = i;
            while i < lines.len() && is_table_row(&lines[i]) {
                i += 1;
            }
            let end = i;
            let block: Vec<String> = lines[start..end].to_vec();
            let formatted = format_table_lines(&block);
            lines.splice(start..end, formatted);
            // Recalculate i since formatted may have inserted a delimiter row
            i = start;
            while i < lines.len() && is_table_row(&lines[i]) {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Normalize headings, block quotes, and list markers
    for line in &mut lines {
        // Headings: ensure space after `#` prefix
        if line.starts_with('#') {
            let hashes = line.chars().take_while(|&c| c == '#').count();
            if hashes > 0 && hashes <= 6 {
                let rest = line[hashes..].trim_start();
                if !rest.is_empty() {
                    *line = format!("{} {}", "#".repeat(hashes), rest);
                }
            }
        }

        // Block quotes: ensure space after `>`
        if line.starts_with('>') {
            let rest = line[1..].trim_start();
            *line = format!("> {}", rest);
        }

        // Lists: normalize markers and spacing
        if !line.is_empty() {
            *line = normalize_list_line(line);
        }
    }

    lines.join("\n")
}
