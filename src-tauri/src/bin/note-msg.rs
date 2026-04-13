#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process;
    use std::time::Duration;

    fn socket_path() -> PathBuf {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/tmp/note-{}", unsafe { libc::getuid() }));
        PathBuf::from(runtime_dir).join("note.sock")
    }

    pub fn run() {
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 2 {
            eprintln!("Usage: note-msg <ping|show|hide|toggle>");
            process::exit(2);
        }

        let cmd = &args[1];
        match cmd.as_str() {
            "ping" | "show" | "hide" | "toggle" => {}
            _ => {
                eprintln!("Unknown command: {cmd}");
                eprintln!("Usage: note-msg <ping|show|hide|toggle>");
                process::exit(2);
            }
        }

        let path = socket_path();
        let mut stream = match UnixStream::connect(&path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("note is not running (no socket at {})", path.display());
                process::exit(1);
            }
        };

        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        if let Err(e) = writeln!(stream, "{cmd}") {
            eprintln!("Failed to send command: {e}");
            process::exit(1);
        }

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        match reader.read_line(&mut response) {
            Ok(_) => {
                let resp = response.trim();
                println!("{resp}");
                if resp.starts_with("error") {
                    process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("No response from note: {e}");
                process::exit(1);
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn run() {
        eprintln!("note-msg is currently supported only on Unix-like platforms.");
        std::process::exit(1);
    }
}

fn main() {
    imp::run();
}
