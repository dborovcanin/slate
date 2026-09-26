use std::time::{SystemTime, UNIX_EPOCH};

use super::canvas::{contrast_fg_for_bg, draw_framed_surface, draw_row_at_styled, put_str, TextStyle};
use ratatui::buffer::Buffer;
use super::render::RenderPalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePickerAction {
    InsertDate,
    SetRemind,
}

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub const MONTH_NAMES_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Zeller-style day of week: 0=Mon, 1=Tue, ..., 6=Sun
pub fn day_of_week(year: i32, month: u32, day: u32) -> u32 {
    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let q = day as i32;
    let k = y % 100;
    let j = y / 100;
    let m = m as i32;
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 - 2 * j) % 7;
    let dow = ((h + 5) % 7 + 7) % 7;
    dow as u32
}

pub fn current_local_datetime_parts() -> Option<(i32, u32, u32, u32, u32)> {
    let epoch_seconds: libc::time_t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()?;
    let mut local_tm = unsafe { std::mem::zeroed::<libc::tm>() };
    let ptr = unsafe { libc::localtime_r(&epoch_seconds, &mut local_tm as *mut libc::tm) };
    if ptr.is_null() {
        return None;
    }
    Some((
        local_tm.tm_year + 1900,
        (local_tm.tm_mon + 1) as u32,
        local_tm.tm_mday as u32,
        local_tm.tm_hour as u32,
        local_tm.tm_min as u32,
    ))
}

pub fn local_datetime_to_epoch_ms(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> Option<i64> {
    if !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 {
        return None;
    }

    let mut local_tm = unsafe { std::mem::zeroed::<libc::tm>() };
    local_tm.tm_year = year - 1900;
    local_tm.tm_mon = i32::try_from(month).ok()? - 1;
    local_tm.tm_mday = i32::try_from(day).ok()?;
    local_tm.tm_hour = i32::try_from(hour).ok()?;
    local_tm.tm_min = i32::try_from(minute).ok()?;
    local_tm.tm_sec = 0;
    local_tm.tm_isdst = -1;

    let epoch_seconds = unsafe { libc::mktime(&mut local_tm as *mut libc::tm) };
    if epoch_seconds < 0 {
        return None;
    }
    i64::try_from(i128::from(epoch_seconds) * 1000).ok()
}

pub fn format_datetime_with_pattern(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    pattern: &str,
) -> String {
    let month_idx = month.saturating_sub(1).min(11) as usize;
    let yyyy = format!("{year:04}");
    let yy = format!("{:02}", year.rem_euclid(100));
    let mm = format!("{month:02}");
    let m = month.to_string();
    let dd = format!("{day:02}");
    let d = day.to_string();
    let hh = format!("{hour:02}");
    let h = hour.to_string();
    let min2 = format!("{minute:02}");
    let mmm = MONTH_NAMES_SHORT[month_idx];
    let mmmm = MONTH_NAMES[month_idx];

    let mut out = if pattern.trim().is_empty() {
        "%Y-%m-%d".to_string()
    } else {
        pattern.to_string()
    };

    if out.contains('%') {
        for (token, value) in [
            ("%Y", yyyy.as_str()),
            ("%y", yy.as_str()),
            ("%m", mm.as_str()),
            ("%d", dd.as_str()),
            ("%H", hh.as_str()),
            ("%M", min2.as_str()),
            ("%b", mmm),
            ("%B", mmmm),
        ] {
            out = out.replace(token, value);
        }
        return out;
    }

    for (token, value) in [
        ("YYYY", yyyy.as_str()),
        ("MMMM", mmmm),
        ("MMM", mmm),
        ("MM", mm.as_str()),
        ("DD", dd.as_str()),
        ("HH", hh.as_str()),
        ("mm", min2.as_str()),
        ("YY", yy.as_str()),
        ("M", m.as_str()),
        ("D", d.as_str()),
        ("H", h.as_str()),
    ] {
        out = out.replace(token, value);
    }
    out
}

/// View data required to render the date picker overlay.
pub struct DatePickerView<'a> {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub include_time: bool,
    pub require_time: bool,
    pub is_remind: bool,
    pub date_format: &'a str,
    pub date_time_format: &'a str,
}

pub fn draw_date_picker(
    view: &DatePickerView,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let box_w: usize = 46;
    let box_h: usize = 18;
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    let surface_bg = palette.surface_bg();

    let title_style = TextStyle {
        fg: Some(palette.primary()),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };
    let header_style = TextStyle {
        fg: Some(palette.code_comment),
        bg: Some(surface_bg),
        dim: true,
        ..Default::default()
    };
    let day_style = TextStyle {
        fg: Some(palette.variable),
        bg: Some(surface_bg),
        ..Default::default()
    };
    let selected_bg = palette.primary();
    let selected_day_style = TextStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };
    let footer_style = TextStyle {
        fg: Some(palette.search_match),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };
    let time_style = TextStyle {
        fg: Some(palette.code_string),
        bg: Some(surface_bg),
        ..Default::default()
    };
    let hint_style = TextStyle {
        fg: Some(palette.code_comment),
        bg: Some(surface_bg),
        dim: true,
        ..Default::default()
    };

    draw_framed_surface(
        buf,
        y,
        x,
        box_w,
        box_h,
        surface_bg,
        palette.primary(),
        false,
        Some(if view.is_remind { "Set reminder" } else { "Insert date" }),
        None,
    );

    let inner_w = box_w.saturating_sub(2);
    let inner_h = box_h.saturating_sub(2);
    let content_h = 14usize;
    let content_top = y + 1 + inner_h.saturating_sub(content_h) / 2;
    let title_row = content_top;
    let header_row = title_row + 2;
    let grid_start_row = header_row + 1;
    let time_row = grid_start_row + 7;
    let hint_row = time_row + 1;
    let footer_row = hint_row + 2;

    // Title: month + year
    let month_name = MONTH_NAMES[view.month.saturating_sub(1).min(11) as usize];
    let title = format!("< {} {} >", month_name, view.year);
    let title_x = x + 1 + inner_w.saturating_sub(title.chars().count()) / 2;
    draw_row_at_styled(
        buf,
        title_row,
        title_x,
        title.chars().count(),
        &title,
        title_style,
    );

    // Calendar block
    const CAL_COLS: usize = 7;
    const CAL_CELL_W: usize = 4;
    let calendar_block_w = CAL_COLS * CAL_CELL_W;
    let calendar_x = x + 1 + inner_w.saturating_sub(calendar_block_w) / 2;
    let weekday_labels = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    for (idx, label) in weekday_labels.iter().enumerate() {
        let col = calendar_x + idx * CAL_CELL_W + (CAL_CELL_W.saturating_sub(label.len())) / 2;
        put_str(buf, header_row, col, label, header_style.to_style());
    }

    // Calendar grid
    let first_dow = day_of_week(view.year, view.month, 1);
    let max_days = days_in_month(view.year, view.month);

    let mut row_idx = 0;
    let mut col_idx = first_dow as usize;

    for day in 1..=max_days {
        let grid_row = grid_start_row + row_idx;
        let grid_col = calendar_x + col_idx * CAL_CELL_W + (CAL_CELL_W.saturating_sub(2) / 2);

        if grid_row < y + box_h - 1 {
            let style = if day == view.day {
                selected_day_style
            } else {
                day_style
            };
            put_str(buf, grid_row, grid_col, &format!("{:>2}", day), style.to_style());
        }

        col_idx += 1;
        if col_idx >= 7 {
            col_idx = 0;
            row_idx += 1;
        }
    }

    let action = if view.is_remind { "remind" } else { "date" };
    let time_label = if view.require_time {
        format!("Time {:02}:{:02} (required)", view.hour, view.minute)
    } else {
        let state = if view.include_time { "on" } else { "off" };
        format!(
            "Time {:02}:{:02} ({state}, Tab toggle)",
            view.hour, view.minute
        )
    };
    let selected = if view.include_time {
        format_datetime_with_pattern(
            view.year,
            view.month,
            view.day,
            view.hour,
            view.minute,
            view.date_time_format,
        )
    } else {
        format_datetime_with_pattern(
            view.year,
            view.month,
            view.day,
            view.hour,
            view.minute,
            view.date_format,
        )
    };
    draw_row_at_styled(
        buf,
        time_row,
        x + 1 + inner_w.saturating_sub(time_label.chars().count()) / 2,
        time_label.chars().count(),
        &time_label,
        time_style,
    );
    let hint = format!("{action}: h/l hour  j/k minute  Enter confirm");
    draw_row_at_styled(
        buf,
        hint_row,
        x + 1 + inner_w.saturating_sub(hint.chars().count()) / 2,
        hint.chars().count(),
        &hint,
        hint_style,
    );
    let footer_x = x + 1 + inner_w.saturating_sub(selected.chars().count()) / 2;
    put_str(buf, footer_row, footer_x, &selected, footer_style.to_style());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::canvas::test_support::{has_styled_symbol, screen};
    use crate::terminal::render::RenderPalette;

    #[test]
    fn draw_date_picker_border_uses_accent_and_surface_background() {
        let palette = RenderPalette {
            primary: 201,
            surface_bg: 250,
            ..RenderPalette::default()
        };
        let view = DatePickerView {
            year: 2026,
            month: 5,
            day: 11,
            hour: 14,
            minute: 30,
            include_time: true,
            require_time: false,
            is_remind: false,
            date_format: "YYYY-MM-DD",
            date_time_format: "YYYY-MM-DD HH:mm",
        };
        let mut buf = screen(24, 80);
        draw_date_picker(&view, &mut buf, 24, 80, palette);
        assert!(
            has_styled_symbol(&buf, "╭", 201, 250),
            "date picker border should use accent fg with surface bg"
        );
    }
}
