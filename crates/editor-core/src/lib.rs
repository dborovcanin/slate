pub mod calc_plan;
pub mod command_history;
pub mod command_catalog;
pub mod context;
pub mod folding;
pub mod format;
pub mod markdown_tokens;
pub mod operations;
pub mod substitute;
pub mod sum;
pub mod table;
pub mod text_rules;
pub mod types;
pub mod vim;

#[cfg(target_arch = "wasm32")]
mod wasm;
