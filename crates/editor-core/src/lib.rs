pub mod calc_plan;
pub mod command_catalog;
pub mod command_history;
pub mod context;
pub mod engine;
pub mod folding;
pub mod format;
pub mod markdown_tokens;
// Math command execution pulls in the full `fend-core` evaluator. It is only
// reachable from the command bar (`:sum` / `:avg`), so both front ends run it
// natively through the host rather than paying for the evaluator in the wasm
// bundle shipped to the UI.
#[cfg(not(target_arch = "wasm32"))]
pub mod math_commands;
pub mod operations;
pub mod substitute;
pub mod sum;
pub mod table;
pub mod text_rules;
pub mod types;
pub mod vim;
pub mod vim_actions;

#[cfg(target_arch = "wasm32")]
mod wasm;
