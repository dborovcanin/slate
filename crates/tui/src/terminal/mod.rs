mod adapter;
mod app;
mod browser;
mod calc_cache;
mod canvas;
mod clipboard;
mod date_picker;
mod external_open;
mod folding;
mod folding_state;
pub(crate) mod graphics;
mod history;
mod icons;
mod input;
mod markdown_view;
mod media_sources;
mod notifications;
mod picker;
pub mod render;
mod render_styles;
mod session;
mod switcher;
mod text_input;
mod text_utils;
pub mod theme;

pub use app::{run_terminal_session, TerminalOptions};
#[cfg(feature = "imap")]
pub(crate) use notifications::send_system_notification;

/// Current local date and time for daily notes (UTC if the local time
/// cannot be determined).
pub(crate) fn local_stamp() -> app_core::daily::LocalStamp {
    let (year, month, day, hour, minute) = date_picker::current_local_datetime_parts()
        .unwrap_or_else(|| {
            let now = time::OffsetDateTime::now_utc();
            (
                now.year(),
                u32::from(u8::from(now.month())),
                u32::from(now.day()),
                u32::from(now.hour()),
                u32::from(now.minute()),
            )
        });
    app_core::daily::LocalStamp {
        year,
        month,
        day,
        hour,
        minute,
    }
}

/// Date label for a daily note, formatted with `[editor] date_format`.
pub(crate) fn daily_date_label(stamp: app_core::daily::LocalStamp, pattern: &str) -> String {
    date_picker::format_datetime_with_pattern(
        stamp.year,
        stamp.month,
        stamp.day,
        stamp.hour,
        stamp.minute,
        pattern,
    )
}
