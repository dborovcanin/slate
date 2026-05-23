use crate::command_catalog::{
    self, BackupAction, CollectionCommandAction, CommandDefinition, CommandId, ExportFormat,
    NoteSecurityAction, ParsedNoteSecurityCommand,
};
use crate::types::{CommandMode, CommandSuggestion};
use crate::vim::{self, VimContext, VimKey, VimState, VimStep};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModuleState {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
    pub cross_note: bool,
}

impl ModuleState {
    pub fn status_message(self) -> String {
        format!(
            "modules math={} table={} variables={} style={} cross_note={}",
            if self.math { "on" } else { "off" },
            if self.table { "on" } else { "off" },
            if self.variables { "on" } else { "off" },
            if self.style { "on" } else { "off" },
            if self.cross_note { "on" } else { "off" },
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
    CrossNote,
}

impl ModuleName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Math => "math",
            Self::Table => "table",
            Self::Variables => "variables",
            Self::Style => "style",
            Self::CrossNote => "cross_note",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandDispatchKind {
    Core,
    HostDate,
    HostRemind,
    HostRemindToggle,
    HostWrite,
    HostExport,
    HostBackupExport,
    HostBackupLoad,
    HostModule,
    HostCollection,
    HostFold,
    HostClipWatch,
    HostNoteSecurity,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostFoldAction {
    Fold,
    Unfold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostClipWatchAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCollectionAction {
    Choose,
    Clear,
    Create,
    Delete,
    Update,
    Purge,
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCommandPlan {
    Date,
    Remind,
    RemindToggle,
    Export {
        format: ExportFormat,
        path: Option<String>,
    },
    BackupExport {
        path: Option<String>,
    },
    BackupLoad {
        path: Option<String>,
    },
    Module {
        command_id: CommandId,
    },
    Collection {
        action: HostCollectionAction,
        collection: Option<String>,
    },
    Fold {
        action: HostFoldAction,
    },
    ClipWatch {
        action: HostClipWatchAction,
    },
    NoteSecurity {
        action: NoteSecurityAction,
        password: String,
    },
    Write {
        quit: bool,
        force: bool,
    },
    Quit {
        force: bool,
    },
}

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

    pub fn classify_command_dispatch(
        mode: CommandMode,
        raw_input: &str,
    ) -> Option<CommandDispatchKind> {
        if let Some(host) = Self::plan_host_command(mode, raw_input) {
            return Some(match host {
                HostCommandPlan::Date => CommandDispatchKind::HostDate,
                HostCommandPlan::Remind => CommandDispatchKind::HostRemind,
                HostCommandPlan::RemindToggle => CommandDispatchKind::HostRemindToggle,
                HostCommandPlan::Write { .. } => CommandDispatchKind::HostWrite,
                HostCommandPlan::Export { .. } => CommandDispatchKind::HostExport,
                HostCommandPlan::BackupExport { .. } => CommandDispatchKind::HostBackupExport,
                HostCommandPlan::BackupLoad { .. } => CommandDispatchKind::HostBackupLoad,
                HostCommandPlan::Module { .. } => CommandDispatchKind::HostModule,
                HostCommandPlan::Collection { .. } => CommandDispatchKind::HostCollection,
                HostCommandPlan::Fold { .. } => CommandDispatchKind::HostFold,
                HostCommandPlan::ClipWatch { .. } => CommandDispatchKind::HostClipWatch,
                HostCommandPlan::NoteSecurity { .. } => CommandDispatchKind::HostNoteSecurity,
                HostCommandPlan::Quit { .. } => CommandDispatchKind::Quit,
            });
        }

        let command = command_catalog::resolve_command(mode, raw_input)?;
        Some(match command.id {
            CommandId::Quit => CommandDispatchKind::Quit,
            _ => CommandDispatchKind::Core,
        })
    }

    pub fn plan_host_command(mode: CommandMode, raw_input: &str) -> Option<HostCommandPlan> {
        if let Some(parsed) = command_catalog::parse_note_security_command(raw_input) {
            return Some(HostCommandPlan::NoteSecurity {
                action: parsed.action,
                password: parsed.password,
            });
        }
        if let Some(parsed) = command_catalog::parse_export_command(raw_input) {
            return Some(HostCommandPlan::Export {
                format: parsed.format,
                path: parsed.path,
            });
        }
        if let Some(parsed) = command_catalog::parse_backup_command(raw_input) {
            return match parsed.action {
                BackupAction::Export => {
                    Some(HostCommandPlan::BackupExport { path: parsed.path })
                }
                BackupAction::Load => Some(HostCommandPlan::BackupLoad { path: parsed.path }),
            };
        }
        if let Some(parsed) = command_catalog::parse_collection_command(raw_input) {
            return Some(HostCommandPlan::Collection {
                action: match parsed.action {
                    CollectionCommandAction::Choose => HostCollectionAction::Choose,
                    CollectionCommandAction::Clear => HostCollectionAction::Clear,
                    CollectionCommandAction::Create => HostCollectionAction::Create,
                    CollectionCommandAction::Delete => HostCollectionAction::Delete,
                    CollectionCommandAction::Update => HostCollectionAction::Update,
                    CollectionCommandAction::Purge => HostCollectionAction::Purge,
                    CollectionCommandAction::Add => HostCollectionAction::Add,
                    CollectionCommandAction::Remove => HostCollectionAction::Remove,
                },
                collection: parsed.collection,
            });
        }

        let normalized = command_catalog::normalize_command(raw_input);
        let command = command_catalog::resolve_command(mode, raw_input)?;
        match command.id {
            CommandId::Date => Some(HostCommandPlan::Date),
            CommandId::Remind => Some(HostCommandPlan::Remind),
            CommandId::RemindToggle => Some(HostCommandPlan::RemindToggle),
            CommandId::ModuleStatus
            | CommandId::ModuleOnMath
            | CommandId::ModuleOffMath
            | CommandId::ModuleToggleMath
            | CommandId::ModuleOnTable
            | CommandId::ModuleOffTable
            | CommandId::ModuleToggleTable
            | CommandId::ModuleOnVariables
            | CommandId::ModuleOffVariables
            | CommandId::ModuleToggleVariables
            | CommandId::ModuleOnStyle
            | CommandId::ModuleOffStyle
            | CommandId::ModuleToggleStyle
            | CommandId::ModuleOnCrossNote
            | CommandId::ModuleOffCrossNote
            | CommandId::ModuleToggleCrossNote => Some(HostCommandPlan::Module {
                command_id: command.id,
            }),
            CommandId::Fold => Some(HostCommandPlan::Fold {
                action: HostFoldAction::Fold,
            }),
            CommandId::Unfold => Some(HostCommandPlan::Fold {
                action: HostFoldAction::Unfold,
            }),
            CommandId::FoldToggle => Some(HostCommandPlan::Fold {
                action: HostFoldAction::Toggle,
            }),
            CommandId::ClipWatch => Some(HostCommandPlan::ClipWatch {
                action: HostClipWatchAction::Start,
            }),
            CommandId::ClipWatchStop => Some(HostCommandPlan::ClipWatch {
                action: HostClipWatchAction::Stop,
            }),
            CommandId::BackupExport => Some(HostCommandPlan::BackupExport { path: None }),
            CommandId::BackupLoad => Some(HostCommandPlan::BackupLoad { path: None }),
            CommandId::Write => Some(HostCommandPlan::Write {
                quit: false,
                force: normalized == "w!",
            }),
            CommandId::WriteQuit => Some(HostCommandPlan::Write {
                quit: true,
                force: normalized == "wq!",
            }),
            CommandId::Quit => Some(HostCommandPlan::Quit {
                force: normalized == "q!",
            }),
            _ => None,
        }
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
            ModuleOnCrossNote => ModuleMutation::Set {
                module: ModuleName::CrossNote,
                enabled: true,
            },
            ModuleOffCrossNote => ModuleMutation::Set {
                module: ModuleName::CrossNote,
                enabled: false,
            },
            ModuleToggleCrossNote => ModuleMutation::Toggle {
                module: ModuleName::CrossNote,
            },
            _ => return None,
        })
    }

    pub fn plan_module_command(
        command_id: CommandId,
        current: ModuleState,
    ) -> Option<ModuleCommandPlan> {
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
        ModuleName::CrossNote => state.cross_note,
    }
}

fn set_module_enabled(state: &mut ModuleState, module: ModuleName, enabled: bool) {
    match module {
        ModuleName::Math => state.math = enabled,
        ModuleName::Table => state.table = enabled,
        ModuleName::Variables => state.variables = enabled,
        ModuleName::Style => state.style = enabled,
        ModuleName::CrossNote => state.cross_note = enabled,
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
            cross_note: true,
        };
        let plan =
            EditorEngine::plan_module_command(CommandId::ModuleOnMath, current).expect("plan");
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
            cross_note: true,
        };
        let plan =
            EditorEngine::plan_module_command(CommandId::ModuleToggleStyle, current).expect("plan");
        assert!(plan.changed);
        assert!(!plan.next.style);
        assert_eq!(
            plan.message,
            "modules math=on table=on variables=on style=off cross_note=on"
        );
    }

    #[test]
    fn module_status_plan_is_non_mutating() {
        let current = ModuleState {
            math: false,
            table: true,
            variables: false,
            style: true,
            cross_note: true,
        };
        let plan =
            EditorEngine::plan_module_command(CommandId::ModuleStatus, current).expect("plan");
        assert!(!plan.changed);
        assert_eq!(plan.next, current);
        assert_eq!(
            plan.message,
            "modules math=off table=on variables=off style=on cross_note=on"
        );
    }

    #[test]
    fn command_dispatch_classifies_host_and_core_commands() {
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "sum row"),
            Some(CommandDispatchKind::Core)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "remind"),
            Some(CommandDispatchKind::HostRemind)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "remind toggle"),
            Some(CommandDispatchKind::HostRemindToggle)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(
                CommandMode::Editor,
                "export txt /tmp/note.txt"
            ),
            Some(CommandDispatchKind::HostExport)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(
                CommandMode::Editor,
                "backup export /tmp/slate.zip"
            ),
            Some(CommandDispatchKind::HostBackupExport)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(
                CommandMode::Editor,
                "backup load /tmp/slate.zip"
            ),
            Some(CommandDispatchKind::HostBackupLoad)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "module math on"),
            Some(CommandDispatchKind::HostModule)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(
                CommandMode::Editor,
                "collection choose Inbox",
            ),
            Some(CommandDispatchKind::HostCollection)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "fold"),
            Some(CommandDispatchKind::HostFold)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "clip-watch on"),
            Some(CommandDispatchKind::HostClipWatch)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Editor, "note lock pass"),
            Some(CommandDispatchKind::HostNoteSecurity)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Vim, "q"),
            Some(CommandDispatchKind::Quit)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Vim, "w"),
            Some(CommandDispatchKind::HostWrite)
        );
        assert_eq!(
            EditorEngine::classify_command_dispatch(CommandMode::Vim, "wq"),
            Some(CommandDispatchKind::HostWrite)
        );
    }

    #[test]
    fn host_command_plan_parses_note_security_and_q_force() {
        let note = EditorEngine::plan_host_command(CommandMode::Editor, "note lock pass123")
            .expect("note plan");
        assert_eq!(
            note,
            HostCommandPlan::NoteSecurity {
                action: NoteSecurityAction::Lock,
                password: "pass123".to_string(),
            }
        );

        let quit_force =
            EditorEngine::plan_host_command(CommandMode::Vim, "q!").expect("quit force");
        assert_eq!(quit_force, HostCommandPlan::Quit { force: true });

        let quit_normal =
            EditorEngine::plan_host_command(CommandMode::Vim, "q").expect("quit normal");
        assert_eq!(quit_normal, HostCommandPlan::Quit { force: false });

        let write = EditorEngine::plan_host_command(CommandMode::Vim, "w").expect("write");
        assert_eq!(
            write,
            HostCommandPlan::Write {
                quit: false,
                force: false
            }
        );

        let write_force = EditorEngine::plan_host_command(CommandMode::Vim, "w!").expect("force");
        assert_eq!(
            write_force,
            HostCommandPlan::Write {
                quit: false,
                force: true
            }
        );

        let write_quit =
            EditorEngine::plan_host_command(CommandMode::Vim, "wq").expect("write+quit");
        assert_eq!(
            write_quit,
            HostCommandPlan::Write {
                quit: true,
                force: false
            }
        );

        let write_quit_force =
            EditorEngine::plan_host_command(CommandMode::Vim, "wq!").expect("write+quit force");
        assert_eq!(
            write_quit_force,
            HostCommandPlan::Write {
                quit: true,
                force: true
            }
        );

        let export_pdf =
            EditorEngine::plan_host_command(CommandMode::Editor, "export pdf /tmp/a.pdf")
                .expect("export plan");
        assert_eq!(
            export_pdf,
            HostCommandPlan::Export {
                format: ExportFormat::Pdf,
                path: Some("/tmp/a.pdf".to_string()),
            }
        );

        let export_md =
            EditorEngine::plan_host_command(CommandMode::Editor, "export md").expect("export md");
        assert_eq!(
            export_md,
            HostCommandPlan::Export {
                format: ExportFormat::Md,
                path: None,
            }
        );
    }

    #[test]
    fn host_command_plan_maps_fold_clip_watch_and_module() {
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "fold"),
            Some(HostCommandPlan::Fold {
                action: HostFoldAction::Fold,
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "zo"),
            Some(HostCommandPlan::Fold {
                action: HostFoldAction::Unfold,
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "clip-watch stop"),
            Some(HostCommandPlan::ClipWatch {
                action: HostClipWatchAction::Stop,
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "module table toggle"),
            Some(HostCommandPlan::Module {
                command_id: CommandId::ModuleToggleTable,
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection choose inbox"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Choose,
                collection: Some("inbox".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection choose none"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Clear,
                collection: None,
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection join work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Add,
                collection: Some("work".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection leave work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Remove,
                collection: Some("work".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection create work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Create,
                collection: Some("work".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection delete work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Delete,
                collection: Some("work".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection update work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Update,
                collection: Some("work".to_string()),
            })
        );
        assert_eq!(
            EditorEngine::plan_host_command(CommandMode::Editor, "collection purge work"),
            Some(HostCommandPlan::Collection {
                action: HostCollectionAction::Purge,
                collection: Some("work".to_string()),
            })
        );
    }
}
