use crate::types::{CommandMode, CommandSuggestion};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Sum,
    SumList,
    SumRow,
    SumColumn,
    SumDoc,
    Avg,
    AvgList,
    AvgRow,
    AvgColumn,
    AvgDoc,
    Date,
    Notify,
    NotifyDelete,
    ModuleStatus,
    ModuleOnMath,
    ModuleOffMath,
    ModuleToggleMath,
    ModuleOnTable,
    ModuleOffTable,
    ModuleToggleTable,
    ModuleOnVariables,
    ModuleOffVariables,
    ModuleToggleVariables,
    ModuleOnStyle,
    ModuleOffStyle,
    ModuleToggleStyle,
    ChooseCollection,
    ClearCollection,
    CreateCollection,
    DeleteCollection,
    UpdateCollection,
    PurgeCollection,
    AddToCollection,
    RemoveFromCollection,
    Format,
    Checklist,
    UnorderedList,
    OrderedList,
    ClipWatch,
    ClipWatchStop,
    Fold,
    Unfold,
    FoldToggle,
    NoteLock,
    NoteUnlock,
    NoteEncrypt,
    NoteDecrypt,
    NoteUnprotect,
    ExportPdf,
    ExportMd,
    ExportTxt,
    Write,
    WriteQuit,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSecurityAction {
    Lock,
    Unlock,
    Encrypt,
    Decrypt,
    Unprotect,
}

impl NoteSecurityAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::Encrypt => "encrypt",
            Self::Decrypt => "decrypt",
            Self::Unprotect => "unprotect",
        }
    }

    pub fn command_id(self) -> CommandId {
        match self {
            Self::Lock => CommandId::NoteLock,
            Self::Unlock => CommandId::NoteUnlock,
            Self::Encrypt => CommandId::NoteEncrypt,
            Self::Decrypt => CommandId::NoteDecrypt,
            Self::Unprotect => CommandId::NoteUnprotect,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNoteSecurityCommand {
    pub action: NoteSecurityAction,
    pub password: String,
    pub used_note_prefix: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Pdf,
    Md,
    Txt,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Md => "md",
            Self::Txt => "txt",
        }
    }

    pub fn command_id(self) -> CommandId {
        match self {
            Self::Pdf => CommandId::ExportPdf,
            Self::Md => CommandId::ExportMd,
            Self::Txt => CommandId::ExportTxt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedExportCommand {
    pub format: ExportFormat,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionCommandAction {
    Choose,
    Clear,
    Create,
    Delete,
    Update,
    Purge,
    Add,
    Remove,
}

impl CollectionCommandAction {
    pub fn command_id(self) -> CommandId {
        match self {
            Self::Choose => CommandId::ChooseCollection,
            Self::Clear => CommandId::ClearCollection,
            Self::Create => CommandId::CreateCollection,
            Self::Delete => CommandId::DeleteCollection,
            Self::Update => CommandId::UpdateCollection,
            Self::Purge => CommandId::PurgeCollection,
            Self::Add => CommandId::AddToCollection,
            Self::Remove => CommandId::RemoveFromCollection,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCollectionCommand {
    pub action: CollectionCommandAction,
    pub collection: Option<String>,
}

pub fn note_security_action_from_token(token: &str) -> Option<NoteSecurityAction> {
    match token.trim().to_ascii_lowercase().as_str() {
        "lock" | "note-lock" | "lock-note" => Some(NoteSecurityAction::Lock),
        "unlock" | "note-unlock" | "unlock-note" => Some(NoteSecurityAction::Unlock),
        "encrypt" | "note-encrypt" | "encrypt-note" => Some(NoteSecurityAction::Encrypt),
        "decrypt" | "note-decrypt" | "decrypt-note" => Some(NoteSecurityAction::Decrypt),
        "unprotect" | "unencrypt" | "note-unprotect" | "unprotect-note" | "note-unencrypt"
        | "unencrypt-note" => Some(NoteSecurityAction::Unprotect),
        _ => None,
    }
}

pub fn parse_note_security_command(input: &str) -> Option<ParsedNoteSecurityCommand> {
    let normalized = input.trim_start().trim_start_matches(':').trim_start();
    if normalized.is_empty() {
        return None;
    }

    let split_at = normalized
        .find(char::is_whitespace)
        .unwrap_or(normalized.len());
    let head = &normalized[..split_at];
    let mut rest = normalized[split_at..].trim_start();

    if head.eq_ignore_ascii_case("note") {
        if rest.is_empty() {
            return None;
        }
        let action_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let action = note_security_action_from_token(&rest[..action_end])?;
        rest = if action_end >= rest.len() {
            ""
        } else {
            rest[action_end..].trim_start()
        };
        return Some(ParsedNoteSecurityCommand {
            action,
            password: rest.to_string(),
            used_note_prefix: true,
        });
    }

    let action = note_security_action_from_token(head)?;
    Some(ParsedNoteSecurityCommand {
        action,
        password: rest.to_string(),
        used_note_prefix: false,
    })
}

pub fn parse_export_command(input: &str) -> Option<ParsedExportCommand> {
    let normalized = input.trim_start().trim_start_matches(':').trim_start();
    if normalized.is_empty() {
        return None;
    }

    let mut parts = normalized.splitn(3, char::is_whitespace);
    let head = parts.next()?.trim();
    if !head.eq_ignore_ascii_case("export") {
        return None;
    }
    let raw_format = parts.next()?.trim().to_ascii_lowercase();
    let format = match raw_format.as_str() {
        "pdf" => ExportFormat::Pdf,
        "md" => ExportFormat::Md,
        "txt" => ExportFormat::Txt,
        _ => return None,
    };
    let path = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Some(ParsedExportCommand { format, path })
}

pub fn parse_collection_command(input: &str) -> Option<ParsedCollectionCommand> {
    let normalized = input.trim_start().trim_start_matches(':').trim_start();
    if normalized.is_empty() {
        return None;
    }

    let mut tokens = normalized.split_whitespace();
    let head = tokens.next()?.to_ascii_lowercase();
    let (action, needs_arg, remainder) = if head == "collection" {
        let sub = tokens.next()?.to_ascii_lowercase();
        let remainder = tokens.collect::<Vec<_>>().join(" ").trim().to_string();
        let (action, needs_arg) = match sub.as_str() {
            "choose" => (CollectionCommandAction::Choose, true),
            "clear" => (CollectionCommandAction::Clear, false),
            "create" => (CollectionCommandAction::Create, true),
            "delete" => (CollectionCommandAction::Delete, true),
            "update" => (CollectionCommandAction::Update, true),
            "purge" => (CollectionCommandAction::Purge, true),
            "join" => (CollectionCommandAction::Add, true),
            "leave" => (CollectionCommandAction::Remove, true),
            _ => return None,
        };
        (action, needs_arg, remainder)
    } else {
        let remainder = tokens.collect::<Vec<_>>().join(" ").trim().to_string();
        let (action, needs_arg) = match head.as_str() {
            "choose_collection" | "choose-collection" => (CollectionCommandAction::Choose, true),
            "clear_collection" | "clear-collection" => (CollectionCommandAction::Clear, false),
            "add_to_collection" | "add-to-collection" => (CollectionCommandAction::Add, true),
            "remove_from_collection" | "remove-from-collection" => {
                (CollectionCommandAction::Remove, true)
            }
            _ => return None,
        };
        (action, needs_arg, remainder)
    };

    if action == CollectionCommandAction::Choose && remainder.eq_ignore_ascii_case("none") {
        return Some(ParsedCollectionCommand {
            action: CollectionCommandAction::Clear,
            collection: None,
        });
    }

    if needs_arg {
        return Some(ParsedCollectionCommand {
            action,
            collection: if remainder.is_empty() {
                None
            } else {
                Some(remainder)
            },
        });
    }

    Some(ParsedCollectionCommand {
        action,
        collection: None,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct CommandDefinition {
    pub id: CommandId,
    pub value: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub modes: &'static [CommandMode],
}

const MODES_BOTH: [CommandMode; 2] = [CommandMode::Vim, CommandMode::Editor];
const MODES_VIM: [CommandMode; 1] = [CommandMode::Vim];

const COMMAND_DEFINITIONS: [CommandDefinition; 54] = [
    CommandDefinition {
        id: CommandId::Sum,
        value: "sum",
        aliases: &[],
        description: "sum paragraph (default scope)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumList,
        value: "sum list",
        aliases: &[],
        description: "sum list at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumRow,
        value: "sum row",
        aliases: &["sum_row"],
        description: "sum markdown table per row at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumColumn,
        value: "sum column",
        aliases: &["sum_column"],
        description: "sum markdown table per column at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumDoc,
        value: "sum doc",
        aliases: &["sum_all", "sum all"],
        description: "sum whole document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Avg,
        value: "avg",
        aliases: &[],
        description: "average paragraph (default scope)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgList,
        value: "avg list",
        aliases: &[],
        description: "average list at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgRow,
        value: "avg row",
        aliases: &["avg_row"],
        description: "average markdown table per row at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgColumn,
        value: "avg column",
        aliases: &["avg_column"],
        description: "average markdown table per column at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgDoc,
        value: "avg doc",
        aliases: &["avg_all", "avg all"],
        description: "average whole document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Date,
        value: "date",
        aliases: &[],
        description: "insert picked date",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Notify,
        value: "notify",
        aliases: &["alarm", "remind"],
        description: "set reminder for current line",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NotifyDelete,
        value: "notify-delete",
        aliases: &["notify_delete", "notify-delte"],
        description: "delete reminder for current line",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleStatus,
        value: "module status",
        aliases: &["module", "modules", "modules status"],
        description: "show modules for current note",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOnMath,
        value: "module math on",
        aliases: &["module on math", "modules math on", "modules on math"],
        description: "enable math module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOffMath,
        value: "module math off",
        aliases: &["module off math", "modules math off", "modules off math"],
        description: "disable math module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleToggleMath,
        value: "module math toggle",
        aliases: &[
            "module toggle math",
            "modules math toggle",
            "modules toggle math",
        ],
        description: "toggle math module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOnTable,
        value: "module table on",
        aliases: &["module on table", "modules table on", "modules on table"],
        description: "enable table module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOffTable,
        value: "module table off",
        aliases: &["module off table", "modules table off", "modules off table"],
        description: "disable table module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleToggleTable,
        value: "module table toggle",
        aliases: &[
            "module toggle table",
            "modules table toggle",
            "modules toggle table",
        ],
        description: "toggle table module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOnVariables,
        value: "module variables on",
        aliases: &[
            "module on variables",
            "modules variables on",
            "modules on variables",
        ],
        description: "enable variables module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOffVariables,
        value: "module variables off",
        aliases: &[
            "module off variables",
            "modules variables off",
            "modules off variables",
        ],
        description: "disable variables module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleToggleVariables,
        value: "module variables toggle",
        aliases: &[
            "module toggle variables",
            "modules variables toggle",
            "modules toggle variables",
        ],
        description: "toggle variables module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOnStyle,
        value: "module style on",
        aliases: &["module on style", "modules style on", "modules on style"],
        description: "enable style module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOffStyle,
        value: "module style off",
        aliases: &["module off style", "modules style off", "modules off style"],
        description: "disable style module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleToggleStyle,
        value: "module style toggle",
        aliases: &[
            "module toggle style",
            "modules style toggle",
            "modules toggle style",
        ],
        description: "toggle style module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ChooseCollection,
        value: "collection choose",
        aliases: &[
            "choose_collection",
            "choose-collection",
            "collection-choose",
        ],
        description: "set working collection for new notes",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ClearCollection,
        value: "collection clear",
        aliases: &["clear_collection", "clear-collection", "collection-clear"],
        description: "clear working collection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::CreateCollection,
        value: "collection create",
        aliases: &["collection-create"],
        description: "create collection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::DeleteCollection,
        value: "collection delete",
        aliases: &["collection-delete"],
        description: "delete collection only",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::UpdateCollection,
        value: "collection update",
        aliases: &["collection-update"],
        description: "open collection update dialog",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::PurgeCollection,
        value: "collection purge",
        aliases: &["collection-purge"],
        description: "delete collection and associated notes",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AddToCollection,
        value: "collection join",
        aliases: &["add_to_collection", "add-to-collection", "collection-join"],
        description: "add active note to collection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::RemoveFromCollection,
        value: "collection leave",
        aliases: &[
            "remove_from_collection",
            "remove-from-collection",
            "collection-leave",
        ],
        description: "remove active note from collection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Format,
        value: "format",
        aliases: &["fmt"],
        description: "format markdown document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ClipWatch,
        value: "clip-watch on",
        aliases: &[
            "clip-watch",
            "clip_watch",
            "clip-watch start",
            "clip_watch_on",
        ],
        description: "watch clipboard and paste text at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ClipWatchStop,
        value: "clip-watch off",
        aliases: &[
            "clip-watch-stop",
            "clip_watch_stop",
            "clip-watch stop",
            "clip_watch_off",
        ],
        description: "stop clipboard watch",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Fold,
        value: "fold",
        aliases: &["closefold", "zc"],
        description: "fold block at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Unfold,
        value: "unfold",
        aliases: &["openfold", "zo"],
        description: "unfold block at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::FoldToggle,
        value: "fold-toggle",
        aliases: &["fold_toggle", "za"],
        description: "toggle fold block at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Checklist,
        value: "format clist",
        aliases: &["clist", "checklist", "checkbox", "checkboxes", "todo", "format checklist"],
        description: "convert selected lines to checklist",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::UnorderedList,
        value: "format ulist",
        aliases: &["ulist", "unordered-list", "unordered", "format unordered"],
        description: "convert selected lines to unordered list",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::OrderedList,
        value: "format olist",
        aliases: &["olist", "ordered-list", "ordered", "format ordered"],
        description: "convert selected lines to ordered list",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NoteLock,
        value: "note lock",
        aliases: &["note-lock", "lock-note"],
        description: "lock current note in app only (plaintext at rest; requires password)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NoteUnlock,
        value: "note unlock",
        aliases: &["note-unlock", "unlock-note"],
        description: "unlock app-level note lock (requires password)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NoteEncrypt,
        value: "note encrypt",
        aliases: &["note-encrypt", "encrypt-note"],
        description: "encrypt current note at rest (requires password)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NoteDecrypt,
        value: "note decrypt",
        aliases: &["note-decrypt", "decrypt-note"],
        description: "decrypt current note to plain text (requires password)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::NoteUnprotect,
        value: "note unprotect",
        aliases: &[
            "note-unprotect",
            "unprotect-note",
            "note unencrypt",
            "note-unencrypt",
            "unencrypt-note",
        ],
        description:
            "remove note protection (locked or encrypted) and password (requires password)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ExportPdf,
        value: "export pdf",
        aliases: &[],
        description: "export note as pdf to path",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ExportMd,
        value: "export md",
        aliases: &["export markdown"],
        description: "export markdown to path, or clipboard when path is omitted",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ExportTxt,
        value: "export txt",
        aliases: &["export text"],
        description: "export plain text to path, or clipboard when path is omitted",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Quit,
        value: "q",
        aliases: &["q!"],
        description: "quit",
        modes: &MODES_VIM,
    },
    CommandDefinition {
        id: CommandId::Write,
        value: "w",
        aliases: &["write", "w!"],
        description: "write (save)",
        modes: &MODES_VIM,
    },
    CommandDefinition {
        id: CommandId::WriteQuit,
        value: "wq",
        aliases: &["writequit", "wq!"],
        description: "write and quit",
        modes: &MODES_VIM,
    },
];

pub fn normalize_command(input: &str) -> String {
    let trimmed = input.trim();
    let without_colon = trimmed.strip_prefix(':').unwrap_or(trimmed);
    without_colon.to_lowercase()
}

fn command_matches(def: &CommandDefinition, normalized_input: &str) -> bool {
    let matches_exact =
        def.value == normalized_input || def.aliases.iter().any(|alias| *alias == normalized_input);
    if matches_exact {
        return true;
    }

    if let Some(parsed) = parse_note_security_command(normalized_input) {
        return parsed.action.command_id() == def.id;
    }
    if let Some(parsed) = parse_export_command(normalized_input) {
        return parsed.format.command_id() == def.id;
    }
    if let Some(parsed) = parse_collection_command(normalized_input) {
        return parsed.action.command_id() == def.id;
    }
    false
}

fn available_commands(mode: CommandMode) -> Vec<&'static CommandDefinition> {
    COMMAND_DEFINITIONS
        .iter()
        .filter(|def| def.modes.contains(&mode))
        .collect()
}

pub fn resolve_command(mode: CommandMode, raw_input: &str) -> Option<&'static CommandDefinition> {
    let normalized = normalize_command(raw_input);
    if normalized.is_empty() {
        return None;
    }
    available_commands(mode)
        .into_iter()
        .find(|def| command_matches(def, &normalized))
}

pub fn list_command_suggestions(mode: CommandMode, raw_input: &str) -> Vec<CommandSuggestion> {
    let query = normalize_command(raw_input);
    let available = available_commands(mode);

    if query.is_empty() {
        return available
            .into_iter()
            .map(|command| CommandSuggestion {
                value: command.value.to_string(),
                description: command.description.to_string(),
            })
            .collect();
    }

    let mut matches = available
        .into_iter()
        .filter_map(|command| {
            let lower_value = command.value.to_lowercase();
            let score = if lower_value.starts_with(&query) {
                0
            } else if lower_value.contains(&query) {
                1
            } else {
                2
            };

            if score < 2 {
                Some((score, command))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.value.cmp(right.1.value))
    });

    matches
        .into_iter()
        .map(|(_, command)| CommandSuggestion {
            value: command.value.to_string(),
            description: command.description.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_command_strips_colon_and_lowercases() {
        assert_eq!(normalize_command(":SUM LIST"), "sum list");
        assert_eq!(normalize_command("  format  "), "format");
    }

    #[test]
    fn resolve_command_is_mode_aware() {
        let editor_q = resolve_command(CommandMode::Editor, "q");
        assert!(editor_q.is_none());

        let vim_q = resolve_command(CommandMode::Vim, "q");
        assert_eq!(vim_q.map(|cmd| cmd.id), Some(CommandId::Quit));

        let editor_w = resolve_command(CommandMode::Editor, "w");
        assert!(editor_w.is_none());

        let vim_w = resolve_command(CommandMode::Vim, "w");
        assert_eq!(vim_w.map(|cmd| cmd.id), Some(CommandId::Write));
        let vim_w_force = resolve_command(CommandMode::Vim, "w!");
        assert_eq!(vim_w_force.map(|cmd| cmd.id), Some(CommandId::Write));

        let vim_wq = resolve_command(CommandMode::Vim, "wq");
        assert_eq!(vim_wq.map(|cmd| cmd.id), Some(CommandId::WriteQuit));
        let vim_wq_force = resolve_command(CommandMode::Vim, "wq!");
        assert_eq!(vim_wq_force.map(|cmd| cmd.id), Some(CommandId::WriteQuit));
    }

    #[test]
    fn resolve_command_handles_aliases() {
        assert_eq!(
            resolve_command(CommandMode::Editor, "sum_column").map(|cmd| cmd.id),
            Some(CommandId::SumColumn)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "avg_all").map(|cmd| cmd.id),
            Some(CommandId::AvgDoc)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "notify-delte").map(|cmd| cmd.id),
            Some(CommandId::NotifyDelete)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "checklist").map(|cmd| cmd.id),
            Some(CommandId::Checklist)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "ordered").map(|cmd| cmd.id),
            Some(CommandId::OrderedList)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "clip_watch_stop").map(|cmd| cmd.id),
            Some(CommandId::ClipWatchStop)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "clip-watch").map(|cmd| cmd.id),
            Some(CommandId::ClipWatch)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "clip-watch off").map(|cmd| cmd.id),
            Some(CommandId::ClipWatchStop)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "fold").map(|cmd| cmd.id),
            Some(CommandId::Fold)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "openfold").map(|cmd| cmd.id),
            Some(CommandId::Unfold)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "za").map(|cmd| cmd.id),
            Some(CommandId::FoldToggle)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "module").map(|cmd| cmd.id),
            Some(CommandId::ModuleStatus)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "module variables off").map(|cmd| cmd.id),
            Some(CommandId::ModuleOffVariables)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "module style toggle").map(|cmd| cmd.id),
            Some(CommandId::ModuleToggleStyle)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "modules off variables").map(|cmd| cmd.id),
            Some(CommandId::ModuleOffVariables)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "note lock").map(|cmd| cmd.id),
            Some(CommandId::NoteLock)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "note lock hunter2").map(|cmd| cmd.id),
            Some(CommandId::NoteLock)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "note decrypt hunter2").map(|cmd| cmd.id),
            Some(CommandId::NoteDecrypt)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "note unencrypt hunter2").map(|cmd| cmd.id),
            Some(CommandId::NoteUnprotect)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "note unprotect hunter2").map(|cmd| cmd.id),
            Some(CommandId::NoteUnprotect)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "lock pass123").map(|cmd| cmd.id),
            Some(CommandId::NoteLock)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "export pdf /tmp/out.pdf").map(|cmd| cmd.id),
            Some(CommandId::ExportPdf)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "export md").map(|cmd| cmd.id),
            Some(CommandId::ExportMd)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "export txt notes.txt").map(|cmd| cmd.id),
            Some(CommandId::ExportTxt)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "choose_collection Work").map(|cmd| cmd.id),
            Some(CommandId::ChooseCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "choose_collection none").map(|cmd| cmd.id),
            Some(CommandId::ClearCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "clear_collection").map(|cmd| cmd.id),
            Some(CommandId::ClearCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "collection create Work").map(|cmd| cmd.id),
            Some(CommandId::CreateCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "collection delete Work").map(|cmd| cmd.id),
            Some(CommandId::DeleteCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "collection update Work").map(|cmd| cmd.id),
            Some(CommandId::UpdateCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "collection purge Work").map(|cmd| cmd.id),
            Some(CommandId::PurgeCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "add_to_collection Work").map(|cmd| cmd.id),
            Some(CommandId::AddToCollection)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "remove_from_collection Work").map(|cmd| cmd.id),
            Some(CommandId::RemoveFromCollection)
        );
    }

    #[test]
    fn parse_note_security_command_supports_prefixed_and_alias_forms() {
        let note_prefixed =
            parse_note_security_command("note encrypt top secret").expect("parse note-prefixed");
        assert_eq!(note_prefixed.action, NoteSecurityAction::Encrypt);
        assert_eq!(note_prefixed.password, "top secret");
        assert!(note_prefixed.used_note_prefix);

        let alias = parse_note_security_command(":unencrypt-note pass123").expect("parse alias");
        assert_eq!(alias.action, NoteSecurityAction::Unprotect);
        assert_eq!(alias.password, "pass123");
        assert!(!alias.used_note_prefix);

        let no_password = parse_note_security_command("note lock").expect("parse usage form");
        assert_eq!(no_password.action, NoteSecurityAction::Lock);
        assert_eq!(no_password.password, "");
        assert!(no_password.used_note_prefix);
    }

    #[test]
    fn parse_export_command_supports_optional_path() {
        let with_path = parse_export_command("export pdf /tmp/out.pdf").expect("parse export path");
        assert_eq!(with_path.format, ExportFormat::Pdf);
        assert_eq!(with_path.path.as_deref(), Some("/tmp/out.pdf"));

        let no_path = parse_export_command(":export md").expect("parse export no path");
        assert_eq!(no_path.format, ExportFormat::Md);
        assert_eq!(no_path.path, None);

        let txt = parse_export_command("export txt notes.txt").expect("parse txt path");
        assert_eq!(txt.format, ExportFormat::Txt);
        assert_eq!(txt.path.as_deref(), Some("notes.txt"));
    }

    #[test]
    fn parse_collection_command_supports_collection_subcommands_and_legacy_aliases() {
        let choose = parse_collection_command("choose_collection Inbox").expect("choose parsed");
        assert_eq!(choose.action, CollectionCommandAction::Choose);
        assert_eq!(choose.collection.as_deref(), Some("Inbox"));

        let clear_alias = parse_collection_command("choose_collection none").expect("clear alias");
        assert_eq!(clear_alias.action, CollectionCommandAction::Clear);
        assert_eq!(clear_alias.collection, None);

        let clear = parse_collection_command(":clear_collection").expect("clear parsed");
        assert_eq!(clear.action, CollectionCommandAction::Clear);
        assert_eq!(clear.collection, None);

        let add = parse_collection_command("add_to_collection Team").expect("add parsed");
        assert_eq!(add.action, CollectionCommandAction::Add);
        assert_eq!(add.collection.as_deref(), Some("Team"));

        let remove = parse_collection_command("remove_from_collection Team").expect("remove");
        assert_eq!(remove.action, CollectionCommandAction::Remove);
        assert_eq!(remove.collection.as_deref(), Some("Team"));

        let create = parse_collection_command("collection create Team").expect("create parsed");
        assert_eq!(create.action, CollectionCommandAction::Create);
        assert_eq!(create.collection.as_deref(), Some("Team"));

        let delete = parse_collection_command("collection delete Team").expect("delete parsed");
        assert_eq!(delete.action, CollectionCommandAction::Delete);
        assert_eq!(delete.collection.as_deref(), Some("Team"));

        let update = parse_collection_command("collection update Team").expect("update parsed");
        assert_eq!(update.action, CollectionCommandAction::Update);
        assert_eq!(update.collection.as_deref(), Some("Team"));

        let purge = parse_collection_command("collection purge Team").expect("purge parsed");
        assert_eq!(purge.action, CollectionCommandAction::Purge);
        assert_eq!(purge.collection.as_deref(), Some("Team"));

        let join = parse_collection_command("collection join Team").expect("join parsed");
        assert_eq!(join.action, CollectionCommandAction::Add);
        assert_eq!(join.collection.as_deref(), Some("Team"));

        let leave = parse_collection_command("collection leave Team").expect("leave parsed");
        assert_eq!(leave.action, CollectionCommandAction::Remove);
        assert_eq!(leave.collection.as_deref(), Some("Team"));
    }

    #[test]
    fn suggestions_follow_prefix_then_contains_ordering() {
        let suggestions = list_command_suggestions(CommandMode::Editor, "sum");
        let values = suggestions
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert_eq!(values.first().map(|value| value.as_str()), Some("sum"));
        assert!(values.contains(&"sum row".to_string()));
        assert!(values.contains(&"sum column".to_string()));
    }
}
