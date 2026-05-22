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
    Remind,
    RemindToggle,
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
    ModuleOnCrossNote,
    ModuleOffCrossNote,
    ModuleToggleCrossNote,
    ChooseCollection,
    ClearCollection,
    CreateCollection,
    DeleteCollection,
    UpdateCollection,
    PurgeCollection,
    AddToCollection,
    RemoveFromCollection,
    Format,
    ParagraphTitle,
    FormatClear,
    Checklist,
    UnorderedList,
    OrderedList,
    FormatBold,
    FormatItalic,
    FormatStrike,
    FormatCode,
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
    Backup,
    BackupLoad,
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
pub enum BackupAction {
    Export,
    Load,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBackupCommand {
    pub action: BackupAction,
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

pub fn parse_backup_command(input: &str) -> Option<ParsedBackupCommand> {
    let normalized = input.trim_start().trim_start_matches(':').trim_start();
    if normalized.is_empty() {
        return None;
    }

    let head_end = normalized
        .find(char::is_whitespace)
        .unwrap_or(normalized.len());
    if !normalized[..head_end].eq_ignore_ascii_case("backup") {
        return None;
    }

    let after_backup = normalized[head_end..].trim_start();
    if after_backup.is_empty() {
        return Some(ParsedBackupCommand {
            action: BackupAction::Export,
            path: None,
        });
    }

    let first_token_end = after_backup
        .find(char::is_whitespace)
        .unwrap_or(after_backup.len());
    let first_token = &after_backup[..first_token_end];

    // `backup load <path>` — restore from a backup zip
    if first_token.eq_ignore_ascii_case("load") {
        let path_str = after_backup[first_token_end..].trim_start();
        return Some(ParsedBackupCommand {
            action: BackupAction::Load,
            path: if path_str.is_empty() {
                None
            } else {
                Some(path_str.to_string())
            },
        });
    }

    // `backup export <path>` — explicit export subcommand
    if first_token.eq_ignore_ascii_case("export") {
        let path_str = after_backup[first_token_end..].trim_start();
        return Some(ParsedBackupCommand {
            action: BackupAction::Export,
            path: if path_str.is_empty() {
                None
            } else {
                Some(path_str.to_string())
            },
        });
    }

    // Legacy: `backup notes <path>` and `backup <path>`
    let path = if first_token.eq_ignore_ascii_case("notes") {
        let after_notes = after_backup[first_token_end..].trim_start();
        if after_notes.is_empty() {
            None
        } else {
            Some(after_notes.to_string())
        }
    } else {
        Some(after_backup.to_string())
    };

    Some(ParsedBackupCommand {
        action: BackupAction::Export,
        path,
    })
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

const COMMAND_DEFINITIONS: [CommandDefinition; 65] = [
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
        id: CommandId::Remind,
        value: "remind",
        aliases: &["alarm"],
        description: "set reminder for current line",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::RemindToggle,
        value: "remind toggle",
        aliases: &["remind-toggle", "remind_toggle"],
        description: "toggle reminder for current line",
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
        id: CommandId::ModuleOnCrossNote,
        value: "module cross_note on",
        aliases: &[
            "module on cross_note",
            "modules cross_note on",
            "modules on cross_note",
        ],
        description: "enable cross-note variables module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleOffCrossNote,
        value: "module cross_note off",
        aliases: &[
            "module off cross_note",
            "modules cross_note off",
            "modules off cross_note",
        ],
        description: "disable cross-note variables module",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ModuleToggleCrossNote,
        value: "module cross_note toggle",
        aliases: &[
            "module toggle cross_note",
            "modules cross_note toggle",
            "modules toggle cross_note",
        ],
        description: "toggle cross-note variables module",
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
        id: CommandId::FormatClear,
        value: "format clear",
        aliases: &["clear-format", "unformat", "plain"],
        description: "strip inline formatting (* ** ~~ `) from selection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::ParagraphTitle,
        value: "paragraph title",
        aliases: &["title", "paragraph heading", "paragraph-title"],
        description: "convert selected lines to heading title",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Checklist,
        value: "paragraph clist",
        aliases: &[
            "clist",
            "checklist",
            "checkbox",
            "checkboxes",
            "todo",
            "format checklist",
            "format clist",
            "paragraph checklist",
        ],
        description: "convert selected lines to checklist",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::OrderedList,
        value: "paragraph olist",
        aliases: &[
            "olist",
            "ordered-list",
            "ordered",
            "format ordered",
            "format olist",
            "paragraph ordered",
        ],
        description: "convert selected lines to ordered list",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::UnorderedList,
        value: "paragraph ulist",
        aliases: &[
            "ulist",
            "unordered-list",
            "unordered",
            "format unordered",
            "format ulist",
            "paragraph unordered",
        ],
        description: "convert selected lines to unordered list",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::FormatBold,
        value: "format bold",
        aliases: &["bold"],
        description: "toggle bold (**) around selection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::FormatCode,
        value: "format code",
        aliases: &["icode", "inline-code"],
        description: "toggle inline code (`) around selection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::FormatItalic,
        value: "format italic",
        aliases: &["italic"],
        description: "toggle italic (*) around selection",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::FormatStrike,
        value: "format strike",
        aliases: &["strike", "strikethrough"],
        description: "toggle strikethrough (~~) around selection",
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
        id: CommandId::Backup,
        value: "backup export",
        aliases: &["backup", "backup notes"],
        description: "export all notes to a backup zip file",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::BackupLoad,
        value: "backup load",
        aliases: &[],
        description: "load notes from a backup zip file (restarts after confirmation)",
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
    if let Some(parsed) = parse_backup_command(normalized_input) {
        return match parsed.action {
            BackupAction::Export => def.id == CommandId::Backup,
            BackupAction::Load => def.id == CommandId::BackupLoad,
        };
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
        .enumerate()
        .filter_map(|(index, command)| {
            let lower_value = command.value.to_lowercase();
            let score = if lower_value.starts_with(&query) {
                0
            } else if lower_value.contains(&query) {
                1
            } else {
                2
            };

            if score < 2 {
                Some((score, index, command))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let namespace_prefix = format!("{query} ");
    matches.sort_by(|left, right| {
        let left_value = left.2.value;
        let right_value = right.2.value;
        let left_in_namespace =
            left_value == query || left_value.starts_with(namespace_prefix.as_str());
        let right_in_namespace =
            right_value == query || right_value.starts_with(namespace_prefix.as_str());

        left.0
            .cmp(&right.0)
            .then_with(|| match (left_in_namespace, right_in_namespace) {
                (true, true) => left.1.cmp(&right.1),
                _ => left_value.cmp(right_value),
            })
    });

    matches
        .into_iter()
        .map(|(_, _, command)| CommandSuggestion {
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
            resolve_command(CommandMode::Editor, "remind").map(|cmd| cmd.id),
            Some(CommandId::Remind)
        );
        assert!(resolve_command(CommandMode::Editor, "notify").is_none());
        assert_eq!(
            resolve_command(CommandMode::Editor, "remind-toggle").map(|cmd| cmd.id),
            Some(CommandId::RemindToggle)
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
            resolve_command(CommandMode::Editor, "backup /tmp/slate.zip").map(|cmd| cmd.id),
            Some(CommandId::Backup)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "backup notes /tmp/slate.zip").map(|cmd| cmd.id),
            Some(CommandId::Backup)
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
    fn parse_backup_command_supports_direct_and_notes_prefixed_paths() {
        let direct = parse_backup_command("backup /tmp/slate.zip").expect("parse backup path");
        assert_eq!(direct.path.as_deref(), Some("/tmp/slate.zip"));
        assert_eq!(direct.action, BackupAction::Export);

        let prefixed =
            parse_backup_command(":backup notes ~/backups/slate.zip").expect("parse backup notes");
        assert_eq!(prefixed.path.as_deref(), Some("~/backups/slate.zip"));
        assert_eq!(prefixed.action, BackupAction::Export);

        let missing = parse_backup_command("backup").expect("parse backup missing path");
        assert_eq!(missing.path, None);
        assert_eq!(missing.action, BackupAction::Export);

        let multi_space =
            parse_backup_command("backup  notes  /tmp/slate.zip").expect("parse with extra spaces");
        assert_eq!(multi_space.path.as_deref(), Some("/tmp/slate.zip"));

        let spaced_path =
            parse_backup_command("backup /tmp/my backup.zip").expect("parse path with space");
        assert_eq!(spaced_path.path.as_deref(), Some("/tmp/my backup.zip"));
    }

    #[test]
    fn parse_backup_command_supports_export_and_load_subcommands() {
        let export =
            parse_backup_command("backup export /tmp/slate.zip").expect("parse backup export");
        assert_eq!(export.action, BackupAction::Export);
        assert_eq!(export.path.as_deref(), Some("/tmp/slate.zip"));

        let load = parse_backup_command("backup load /tmp/slate.zip").expect("parse backup load");
        assert_eq!(load.action, BackupAction::Load);
        assert_eq!(load.path.as_deref(), Some("/tmp/slate.zip"));

        let load_no_path = parse_backup_command("backup load").expect("parse backup load no path");
        assert_eq!(load_no_path.action, BackupAction::Load);
        assert_eq!(load_no_path.path, None);
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

    #[test]
    fn suggestions_keep_namespace_order_for_paragraph_and_format() {
        let paragraph_values = list_command_suggestions(CommandMode::Editor, "paragraph")
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert_eq!(
            paragraph_values,
            vec![
                "paragraph title",
                "paragraph clist",
                "paragraph olist",
                "paragraph ulist",
            ]
        );

        let format_values = list_command_suggestions(CommandMode::Editor, "format")
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert_eq!(
            format_values,
            vec![
                "format",
                "format clear",
                "format bold",
                "format code",
                "format italic",
                "format strike",
            ]
        );
    }
}
