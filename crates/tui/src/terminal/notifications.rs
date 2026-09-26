use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_epoch_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

pub fn send_system_notification(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("notify-send")
            .args(["--", title, body])
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
            escape_applescript(body),
            escape_applescript(title)
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
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('slate').Show($toast);"
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
