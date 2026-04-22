use crate::command_catalog::{self, CommandDefinition, CommandId, ParsedNoteSecurityCommand};
use crate::types::{CommandMode, CommandSuggestion};
use crate::vim::{self, VimContext, VimKey, VimState, VimStep};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModuleState {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
}

impl ModuleState {
    pub fn status_message(self) -> String {
        format!(
            "modules math={} table={} variables={} style={}",
            if self.math { "on" } else { "off" },
            if self.table { "on" } else { "off" },
            if self.variables { "on" } else { "off" },
            if self.style { "on" } else { "off" },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleName {
    Math,
    Table,
    Variables,
    Style,
}

impl ModuleName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Math => "math",
            Self::Table => "table",
            Self::Variables => "variables",
            Self::Style => "style",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleMutation {
    Set { module: ModuleName, enabled: bool },
    Toggle { module: ModuleName },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCommandPlan {
    pub changed: bool,
    pub next: ModuleState,
    pub message: String,
}

pub struct EditorEngine;

impl EditorEngine {
    pub fn normalize_command(raw_input: &str) -> String {
        command_catalog::normalize_command(raw_input)
    }

    pub fn resolve_command(
        mode: CommandMode,
        raw_input: &str,
    ) -> Option<&'static CommandDefinition> {
        command_catalog::resolve_command(mode, raw_input)
    }

    pub fn list_command_suggestions(mode: CommandMode, raw_input: &str) -> Vec<CommandSuggestion> {
        command_catalog::list_command_suggestions(mode, raw_input)
    }

    pub fn parse_note_security_command(raw_input: &str) -> Option<ParsedNoteSecurityCommand> {
        command_catalog::parse_note_security_command(raw_input)
    }

    pub fn step_vim(state: &VimState, key: VimKey, ctx: &VimContext) -> VimStep {
        vim::step(state, key, ctx)
    }

    pub fn module_mutation_for_command(command_id: CommandId) -> Option<ModuleMutation> {
        use CommandId::*;
        Some(match command_id {
            ModuleOnMath => ModuleMutation::Set {
                module: ModuleName::Math,
                enabled: true,
            },
            ModuleOffMath => ModuleMutation::Set {
                module: ModuleName::Math,
                enabled: false,
            },
            ModuleToggleMath => ModuleMutation::Toggle {
                module: ModuleName::Math,
            },
            ModuleOnTable => ModuleMutation::Set {
                module: ModuleName::Table,
                enabled: true,
            },
            ModuleOffTable => ModuleMutation::Set {
                module: ModuleName::Table,
                enabled: false,
            },
            ModuleToggleTable => ModuleMutation::Toggle {
                module: ModuleName::Table,
            },
            ModuleOnVariables => ModuleMutation::Set {
                module: ModuleName::Variables,
                enabled: true,
            },
            ModuleOffVariables => ModuleMutation::Set {
                module: ModuleName::Variables,
                enabled: false,
            },
            ModuleToggleVariables => ModuleMutation::Toggle {
                module: ModuleName::Variables,
            },
            ModuleOnStyle => ModuleMutation::Set {
                module: ModuleName::Style,
                enabled: true,
            },
            ModuleOffStyle => ModuleMutation::Set {
                module: ModuleName::Style,
                enabled: false,
            },
            ModuleToggleStyle => ModuleMutation::Toggle {
                module: ModuleName::Style,
            },
            _ => return None,
        })
    }

    pub fn plan_module_command(command_id: CommandId, current: ModuleState) -> Option<ModuleCommandPlan> {
        if command_id == CommandId::ModuleStatus {
            return Some(ModuleCommandPlan {
                changed: false,
                next: current,
                message: current.status_message(),
            });
        }

        let mutation = Self::module_mutation_for_command(command_id)?;
        Some(apply_module_mutation(current, mutation))
    }
}

fn module_enabled(state: ModuleState, module: ModuleName) -> bool {
    match module {
        ModuleName::Math => state.math,
        ModuleName::Table => state.table,
        ModuleName::Variables => state.variables,
        ModuleName::Style => state.style,
    }
}

fn set_module_enabled(state: &mut ModuleState, module: ModuleName, enabled: bool) {
    match module {
        ModuleName::Math => state.math = enabled,
        ModuleName::Table => state.table = enabled,
        ModuleName::Variables => state.variables = enabled,
        ModuleName::Style => state.style = enabled,
    }
}

fn apply_module_mutation(current: ModuleState, mutation: ModuleMutation) -> ModuleCommandPlan {
    match mutation {
        ModuleMutation::Set { module, enabled } => {
            let already = module_enabled(current, module) == enabled;
            if already {
                return ModuleCommandPlan {
                    changed: false,
                    next: current,
                    message: format!(
                        "module {} already {}",
                        module.as_str(),
                        if enabled { "on" } else { "off" }
                    ),
                };
            }
            let mut next = current;
            set_module_enabled(&mut next, module, enabled);
            ModuleCommandPlan {
                changed: true,
                next,
                message: next.status_message(),
            }
        }
        ModuleMutation::Toggle { module } => {
            let mut next = current;
            let toggled = !module_enabled(current, module);
            set_module_enabled(&mut next, module, toggled);
            ModuleCommandPlan {
                changed: true,
                next,
                message: next.status_message(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_plan_is_idempotent_for_set_operations() {
        let current = ModuleState {
            math: true,
            table: true,
            variables: true,
            style: true,
        };
        let plan = EditorEngine::plan_module_command(CommandId::ModuleOnMath, current)
            .expect("plan");
        assert!(!plan.changed);
        assert_eq!(plan.next, current);
        assert_eq!(plan.message, "module math already on");
    }

    #[test]
    fn module_plan_toggles_and_formats_status() {
        let current = ModuleState {
            math: true,
            table: true,
            variables: true,
            style: true,
        };
        let plan = EditorEngine::plan_module_command(CommandId::ModuleToggleStyle, current)
            .expect("plan");
        assert!(plan.changed);
        assert!(!plan.next.style);
        assert_eq!(
            plan.message,
            "modules math=on table=on variables=on style=off"
        );
    }

    #[test]
    fn module_status_plan_is_non_mutating() {
        let current = ModuleState {
            math: false,
            table: true,
            variables: false,
            style: true,
        };
        let plan = EditorEngine::plan_module_command(CommandId::ModuleStatus, current)
            .expect("plan");
        assert!(!plan.changed);
        assert_eq!(plan.next, current);
        assert_eq!(
            plan.message,
            "modules math=off table=on variables=off style=on"
        );
    }
}
