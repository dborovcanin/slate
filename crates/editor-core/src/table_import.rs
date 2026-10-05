//! Turning pasted CSV or TSV, such as cells copied from a spreadsheet, into a
//! markdown table.
//!
//! TSV is taken whenever every row has the same number of tabs: prose rarely
//! holds tabs. CSV (`,` or `;`) needs more: every row the same number of
//! fields, and no field after a delimiter starting with a space, which keeps
//! prose like `Hello, world` as text. The first row becomes the header.

use crate::table::{format_table_lines, serialize_table_row};

/// The markdown table for pasted CSV or TSV, aligned, or `None` when `text`
/// does not read as one: under two rows or two columns, or rows of
/// different lengths.
pub fn delimited_text_to_table(text: &str) -> Option<Vec<String>> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim_end_matches('\n');
    if !text.contains('\n') {
        return None;
    }
    let rows = ['\t', ',', ';']
        .into_iter()
        .find_map(|delimiter| parse_rows(text, delimiter))?;

    let mut lines = Vec::with_capacity(rows.len() + 1);
    let columns = rows[0].len();
    lines.push(serialize_table_row(&rows[0]));
    lines.push(serialize_table_row(&vec!["---".to_string(); columns]));
    lines.extend(rows[1..].iter().map(|row| serialize_table_row(row)));
    Some(format_table_lines(&lines))
}

/// Rows of cells split on `delimiter`, with `"`-quoted fields (`""` is a
/// quote; a line break inside quotes becomes a space). `None` unless the
/// text reads as a table with this delimiter.
fn parse_rows(text: &str, delimiter: char) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut field_start = true;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => field.push(' '),
                _ => field.push(ch),
            }
            continue;
        }
        match ch {
            '"' if field_start => quoted = true,
            ch if ch == delimiter => {
                row.push(cell_text(&field));
                field.clear();
                field_start = true;
                // `a, b` is prose, not CSV; a row may end in an empty field.
                if delimiter != '\t' && chars.peek().is_some_and(|next| matches!(next, ' ' | '\t'))
                {
                    return None;
                }
                continue;
            }
            '\n' => {
                row.push(cell_text(&field));
                field.clear();
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(ch),
        }
        field_start = ch == '\n';
    }
    if quoted {
        return None;
    }
    row.push(cell_text(&field));
    rows.push(row);

    let columns = rows[0].len();
    (rows.len() >= 2 && columns >= 2 && rows.iter().all(|row| row.len() == columns)).then_some(rows)
}

/// A field as a table cell: trimmed, with `|` escaped so it stays one cell.
/// A pipe is escaped by an odd number of backslashes, so any already before
/// it are doubled: `a\|b` becomes `a\\\|b`.
fn cell_text(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut backslashes = 0;
    for ch in field.trim().chars() {
        match ch {
            '\\' => backslashes += 1,
            '|' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('|');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(ch);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Option<Vec<String>> {
        delimited_text_to_table(text)
    }

    #[test]
    fn tsv_from_a_spreadsheet_becomes_an_aligned_table() {
        assert_eq!(
            table("Name\tHours\nAna\t7.5\nBob\t12\n").unwrap(),
            vec![
                "| Name | Hours |",
                "| ---- | ----- |",
                "| Ana  | 7.5   |",
                "| Bob  | 12    |",
            ]
        );
    }

    #[test]
    fn csv_with_quotes_becomes_a_table() {
        assert_eq!(
            table("item,note\r\napple,\"red, sweet\"\r\npear,\"say \"\"hi\"\"\nthen\"\r\n")
                .unwrap(),
            vec![
                "| item  | note          |",
                "| ----- | ------------- |",
                "| apple | red, sweet    |",
                "| pear  | say \"hi\" then |",
            ]
        );
    }

    #[test]
    fn semicolons_and_empty_cells_work_and_pipes_stay_inside_cells() {
        assert_eq!(
            table("a;b;c\n1;;a|b\n").unwrap(),
            vec![
                "| a   | b   | c    |",
                "| --- | --- | ---- |",
                "| 1   |     | a\\|b |"
            ]
        );
    }

    #[test]
    fn rows_may_end_in_an_empty_field() {
        assert_eq!(
            table("a,b,c\n1,2,\n3,4,5").unwrap(),
            vec![
                "| a   | b   | c   |",
                "| --- | --- | --- |",
                "| 1   | 2   |     |",
                "| 3   | 4   | 5   |"
            ]
        );
    }

    #[test]
    fn backslashes_before_a_pipe_keep_it_inside_the_cell() {
        assert_eq!(cell_text(r"a\|b"), r"a\\\|b");
        assert_eq!(cell_text(r"a\\|b"), r"a\\\\\|b");
        assert_eq!(cell_text(r"C:\dir\"), r"C:\dir\");
        let lines = table("name,note\nitem,a\\|b").unwrap();
        for line in &lines {
            assert_eq!(table_syntax::split_table_cells(line).len(), 2, "{line}");
        }
    }

    #[test]
    fn prose_and_uneven_rows_stay_text() {
        for text in [
            "just one line\twith a tab",
            "a,b",
            "Hello, world\nGoodbye, moon",
            "one\ttwo\nthree",
            "x,y,z\n1,2",
            "no delimiters\nat all",
            "\"open,quote\nnever,closed",
            "single\ncolumn",
        ] {
            assert_eq!(table(text), None, "{text:?}");
        }
    }
}
