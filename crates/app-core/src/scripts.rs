//! Explicitly invoked external scripts. No shell interpolation or automatic hooks.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptInput {
    #[default]
    None,
    Selection,
    Note,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptOutput {
    #[default]
    Insert,
    ReplaceSelection,
    Message,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptDefinition {
    pub argv: Vec<String>,
    #[serde(default)]
    pub input: ScriptInput,
    #[serde(default)]
    pub output: ScriptOutput,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    30
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScriptConfig {
    #[serde(default)]
    pub scripts: BTreeMap<String, ScriptDefinition>,
    /// Mode -> key sequence -> command. Key interpretation belongs to the host.
    #[serde(default)]
    pub keybindings: BTreeMap<String, BTreeMap<String, String>>,
}
impl ScriptConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for (name, script) in &config.scripts {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("invalid script name: {name}"));
            }
            if script.argv.first().is_none_or(|s| s.trim().is_empty()) {
                return Err(format!("script {name}: argv needs an executable"));
            }
            if !(1..=3600).contains(&script.timeout_seconds) {
                return Err(format!("script {name}: timeout_seconds must be 1..3600"));
            }
        }
        for mode in config.keybindings.keys() {
            if !matches!(mode.as_str(), "normal" | "editor" | "visual") {
                return Err(format!("invalid keybinding mode: {mode}"));
            }
        }
        Ok(config)
    }
}
#[derive(Debug, Serialize)]
pub struct ScriptRequest {
    pub version: u8,
    pub args: Vec<String>,
    pub text: String,
    pub note_id: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptResponse {
    pub text: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// Parse command-bar arguments without invoking a shell. Quotes and backslash
/// escapes preserve spaces and empty arguments; expansion is deliberately absent.
pub fn parse_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in input.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            started = true;
        } else if ch.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    if quote.is_some() || escaped {
        return Err("unfinished quote or escape".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // The child starts a new process group; also stop descendants holding pipes.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
#[cfg(unix)]
use std::os::fd::AsRawFd as Pipe;
#[cfg(not(unix))]
trait Pipe {}
#[cfg(not(unix))]
impl<T> Pipe for T {}

/// Waits until `pipe` is ready, polling so that `stop` also ends a wait on a
/// pipe that a detached descendant keeps open.
#[cfg(unix)]
fn wait_ready(pipe: &impl Pipe, write: bool, stop: &AtomicBool) -> std::io::Result<()> {
    let events = if write { libc::POLLOUT } else { libc::POLLIN };
    let mut fd = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events,
        revents: 0,
    };
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(std::io::Error::other("script I/O stopped"));
        }
        // SAFETY: `fd` is one valid pollfd for the duration of the call.
        match unsafe { libc::poll(&mut fd, 1, 20) } {
            0 => {}
            n if n > 0 => return Ok(()),
            _ => {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}
#[cfg(not(unix))]
fn wait_ready(_: &impl Pipe, _: bool, _: &AtomicBool) -> std::io::Result<()> {
    Ok(())
}
fn write_input(
    mut stdin: std::process::ChildStdin,
    input: &[u8],
    stop: &AtomicBool,
) -> std::io::Result<()> {
    #[cfg(unix)]
    // SAFETY: fcntl on a pipe fd owned by `stdin`. Non-blocking writes keep a
    // full pipe from blocking past `stop`.
    unsafe {
        let fd = stdin.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut written = 0;
    while written < input.len() {
        wait_ready(&stdin, true, stop)?;
        match stdin.write(&input[written..]) {
            Ok(n) => written += n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn read_bounded(
    mut reader: impl Read + Pipe,
    overflow: &AtomicBool,
    stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        wait_ready(&reader, false, stop).map_err(|e| e.to_string())?;
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
        if bytes.len() > MAX_OUTPUT_BYTES {
            overflow.store(true, Ordering::Release);
            return Err("script output exceeds 4 MiB".into());
        }
    }
}

/// Blocking worker entry. Call on a background thread in interactive hosts.
pub fn run_script(
    script: &ScriptDefinition,
    request: &ScriptRequest,
    cancel: Arc<AtomicBool>,
) -> Result<ScriptResponse, String> {
    let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if input.len() > MAX_INPUT_BYTES {
        return Err("script input exceeds 16 MiB".into());
    }
    if cancel.load(Ordering::Acquire) {
        return Err("script cancelled".into());
    }
    let executable = script.argv.first().ok_or("script argv is empty")?;
    let mut command = Command::new(executable);
    command
        .args(&script.argv[1..])
        .args(&request.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {executable}: {e}"))?;
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let overflow = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        std::thread::spawn(move || write_input(stdin, &input, &stop))
    };
    let out = {
        let (overflow, stop) = (overflow.clone(), stop.clone());
        std::thread::spawn(move || read_bounded(stdout, &overflow, &stop))
    };
    let err = {
        let (overflow, stop) = (overflow.clone(), stop.clone());
        std::thread::spawn(move || read_bounded(stderr, &overflow, &stop))
    };
    let started = Instant::now();
    let result = loop {
        if cancel.load(Ordering::Acquire) {
            break Err("script cancelled".to_string());
        }
        if overflow.load(Ordering::Acquire) {
            break Err("script output exceeds 4 MiB".to_string());
        }
        if started.elapsed() >= Duration::from_secs(script.timeout_seconds) {
            break Err("script timed out".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) if out.is_finished() && err.is_finished() && writer.is_finished() => {
                break Ok(status)
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(e.to_string()),
        }
    };
    if result.is_err() {
        terminate(&mut child);
        // Detached descendants can retain pipes even after the process group
        // dies. On Unix the workers poll and stop; elsewhere they may block in
        // pipe I/O, so never let those prevent cancellation from returning.
        stop.store(true, Ordering::Release);
        #[cfg(not(unix))]
        {
            let cleanup_started = Instant::now();
            while !(writer.is_finished() && out.is_finished() && err.is_finished())
                && cleanup_started.elapsed() < Duration::from_millis(100)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            if !(writer.is_finished() && out.is_finished() && err.is_finished()) {
                if let Err(error) = result {
                    return Err(error);
                }
            }
        }
    }
    let written = writer.join().map_err(|_| "script input worker failed")?;
    let stdout = out.join().map_err(|_| "script output worker failed")?;
    let stderr = err.join().map_err(|_| "script error worker failed")?;
    let status = result?;
    let stderr = stderr?;
    if !status.success() {
        return Err(format!(
            "script exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    match written {
        // A successful script may answer without consuming its input.
        Err(e) if e.kind() != ErrorKind::BrokenPipe => {
            return Err(format!("cannot write script input: {e}"))
        }
        _ => {}
    }
    serde_json::from_slice(&stdout?).map_err(|e| format!("invalid script response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_arguments_are_literal() {
        assert_eq!(
            parse_arguments(r#"report 'two words' "" "$(touch /tmp/nope)" a\ b"#).unwrap(),
            vec!["report", "two words", "", "$(touch /tmp/nope)", "a b"]
        );
        assert!(parse_arguments("'unfinished").is_err());
    }
    #[test]
    fn config_validates_registration() {
        assert!(ScriptConfig::parse("[scripts.bad]\nargv=[]").is_err());
        assert!(ScriptConfig::parse("[keybindings.typo]\nx='run foo'").is_err());
        let cfg = ScriptConfig::parse("[scripts.upper]\nargv=['python3','upper.py']\ninput='selection'\noutput='replace-selection'\n[keybindings.visual]\n'<C-r>'='run upper'").unwrap();
        assert_eq!(cfg.scripts["upper"].input, ScriptInput::Selection);
    }
    #[cfg(unix)]
    fn shell(code: &str, timeout_seconds: u64) -> ScriptDefinition {
        ScriptDefinition {
            argv: vec!["sh".into(), "-c".into(), code.into()],
            input: ScriptInput::None,
            output: ScriptOutput::Insert,
            timeout_seconds,
        }
    }
    fn request() -> ScriptRequest {
        ScriptRequest {
            version: 1,
            args: vec![],
            text: "héllo".into(),
            note_id: None,
        }
    }
    #[test]
    #[cfg(unix)]
    fn process_success_failure_and_invalid_json() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            run_script(
                &shell("cat >/dev/null; printf '{\"text\":\"héllo\"}'", 2),
                &request(),
                cancel.clone()
            )
            .unwrap()
            .text,
            "héllo"
        );
        assert!(run_script(
            &shell("cat >/dev/null; echo failure >&2; exit 7", 2),
            &request(),
            cancel.clone()
        )
        .unwrap_err()
        .contains("failure"));
        assert!(
            run_script(&shell("cat >/dev/null; echo nope", 2), &request(), cancel)
                .unwrap_err()
                .contains("invalid script response")
        );
    }
    #[test]
    #[cfg(unix)]
    fn script_may_answer_without_reading_large_input() {
        let mut request = request();
        request.text = "a".repeat(1024 * 1024);
        let response = run_script(
            &shell("printf '{\"text\":\"ok\"}'", 2),
            &request,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(response.text, "ok");
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn detached_descendant_holding_pipes_does_not_block_workers() {
        // setsid leaves the process group, so only the poll-based workers can
        // stop waiting on the pipes it inherited; joining them must not hang.
        let mut request = request();
        request.text = "a".repeat(1024 * 1024);
        let start = Instant::now();
        assert!(run_script(
            &shell("setsid sleep 3 & exit 0", 1),
            &request,
            Arc::new(AtomicBool::new(false))
        )
        .unwrap_err()
        .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    #[cfg(unix)]
    fn timeout_includes_descendants_holding_pipes() {
        let start = Instant::now();
        assert!(run_script(
            &shell("sleep 20 &", 1),
            &request(),
            Arc::new(AtomicBool::new(false))
        )
        .unwrap_err()
        .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    #[cfg(unix)]
    fn cancellation_and_output_limit() {
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Release);
        });
        assert!(run_script(&shell("sleep 20", 3), &request(), cancel)
            .unwrap_err()
            .contains("cancelled"));
        assert!(run_script(
            &shell("cat >/dev/null; head -c 5000000 /dev/zero", 3),
            &request(),
            Arc::new(AtomicBool::new(false))
        )
        .unwrap_err()
        .contains("exceeds"));
    }
}
