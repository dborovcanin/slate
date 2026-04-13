pub mod command_catalog;
pub mod context;
pub mod format;
pub mod operations;
pub mod sum;
pub mod text_rules;
pub mod types;
pub mod vim;

#[cfg(target_arch = "wasm32")]
mod wasm;
