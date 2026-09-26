mod adapter;
mod ansi;
mod ansi_bridge;
mod app;
mod calc_cache;
mod clipboard;
mod date_picker;
mod folding;
mod folding_state;
mod history;
mod input;
mod markdown_view;
mod media_sources;
mod notifications;
pub mod render;
mod render_styles;
mod session;
mod switcher;
mod text_utils;
pub mod theme;

pub use app::{run_terminal_session, TerminalOptions};
#[cfg(feature = "imap")]
pub(crate) use notifications::send_system_notification;
