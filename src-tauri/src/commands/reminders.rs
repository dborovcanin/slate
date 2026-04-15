use app_core::storage::Reminder;
use app_core::AppCore;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

fn now_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[tauri::command]
pub fn list_note_reminders(
    core: State<'_, AppCore>,
    note_id: String,
) -> Result<Vec<Reminder>, String> {
    core.db().list_reminders(&note_id)
}

#[tauri::command]
pub fn upsert_note_reminder(
    core: State<'_, AppCore>,
    note_id: String,
    line_number: i64,
    remind_at_ms: i64,
    display_at: String,
    line_text: String,
) -> Result<Reminder, String> {
    core.db()
        .upsert_reminder(&note_id, line_number, remind_at_ms, &display_at, &line_text)
}

#[tauri::command]
pub fn delete_note_reminder(
    core: State<'_, AppCore>,
    note_id: String,
    line_number: i64,
) -> Result<bool, String> {
    core.db().delete_reminder(&note_id, line_number)
}

#[tauri::command]
pub fn mark_note_reminder_notified(
    core: State<'_, AppCore>,
    note_id: String,
    line_number: i64,
    notified_at_ms: Option<i64>,
) -> Result<Option<Reminder>, String> {
    core.db()
        .mark_reminder_notified(&note_id, line_number, notified_at_ms.unwrap_or_else(now_ms))
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

#[tauri::command]
pub fn send_system_notification(title: String, body: String) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("notify-send")
            .args([title.as_str(), body.as_str()])
            .status()
            .map_err(|e| format!("notify-send unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("notify-send exited with status {status}"));
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(&body),
            escape_applescript(&title)
        );
        let status = Command::new("osascript")
            .args(["-e", script.as_str()])
            .status()
            .map_err(|e| format!("osascript unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("osascript exited with status {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_title = title.replace('\'', "''");
        let escaped_body = body.replace('\'', "''");
        let command = format!(
            "$null=[Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime];\
             $null=[Windows.Data.Xml.Dom.XmlDocument,Windows.Data.Xml.Dom.XmlDocument,ContentType=WindowsRuntime];\
             $xml=New-Object Windows.Data.Xml.Dom.XmlDocument;\
             $xml.LoadXml(\"<toast><visual><binding template='ToastGeneric'><text>{escaped_title}</text><text>{escaped_body}</text></binding></visual></toast>\");\
             $toast=[Windows.UI.Notifications.ToastNotification]::new($xml);\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('note').Show($toast);"
        );
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command.as_str(),
            ])
            .status()
            .map_err(|e| format!("powershell unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("powershell exited with status {status}"));
    }

    #[allow(unreachable_code)]
    Err("system notifications are not supported on this platform".to_string())
}
