#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::thread;
    use tauri::{AppHandle, Manager};

    pub fn socket_path() -> PathBuf {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/tmp/slate-{}", unsafe { libc::getuid() }));
        PathBuf::from(runtime_dir).join("slate.sock")
    }

    pub fn start_ipc_server(app: AppHandle) {
        let path = socket_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // remove stale socket
        let _ = std::fs::remove_file(&path);

        thread::spawn(move || {
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("IPC: failed to bind {}: {e}", path.display());
                    return;
                }
            };

            for incoming in listener.incoming() {
                let Ok(stream) = incoming else {
                    continue;
                };
                let app = app.clone();
                thread::spawn(move || {
                    handle_stream(stream, &app);
                });
            }
        });
    }

    fn handle_stream(stream: UnixStream, app: &AppHandle) {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        if reader.read_line(&mut line).is_err() {
            return;
        }

        let response = handle_command(line.trim(), app);
        let mut stream = reader.into_inner();
        let _ = writeln!(stream, "{response}");
    }

    fn handle_command(cmd: &str, app: &AppHandle) -> String {
        match cmd {
            "ping" => "pong".to_string(),
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                    "ok".to_string()
                } else {
                    "error: no window".to_string()
                }
            }
            "hide" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.hide();
                    "ok".to_string()
                } else {
                    "error: no window".to_string()
                }
            }
            "toggle" => {
                if let Some(win) = app.get_webview_window("main") {
                    match win.is_visible() {
                        Ok(true) => {
                            let _ = win.hide();
                            "hidden".to_string()
                        }
                        _ => {
                            let _ = win.show();
                            let _ = win.set_focus();
                            "shown".to_string()
                        }
                    }
                } else {
                    "error: no window".to_string()
                }
            }
            _ => format!("error: unknown command '{cmd}'"),
        }
    }

    pub fn cleanup_socket() {
        let _ = std::fs::remove_file(socket_path());
    }
}

#[cfg(not(unix))]
mod imp {
    use tauri::AppHandle;

    pub fn start_ipc_server(_app: AppHandle) {}

    pub fn cleanup_socket() {}
}

pub use imp::{cleanup_socket, start_ipc_server};
