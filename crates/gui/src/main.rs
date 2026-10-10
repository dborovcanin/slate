//! `slate-gui`: the desktop front end. See docs/gui.md.
mod editor_lines;
mod keys;
mod note_view;
mod theme;
mod window;

use gpui::{
    px, size, App, AppContext, Application, Bounds, SharedString, WindowBounds, WindowOptions,
};
use note_view::NoteHost;

/// First installed family wins; GPUI falls back to its default otherwise.
const SANS_FONTS: [&str; 4] = ["IBM Plex Sans", "Inter", "Cantarell", "DejaVu Sans"];
const MONO_FONTS: [&str; 4] = [
    "IBM Plex Mono",
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
];

struct Args {
    note_id: Option<String>,
    light: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        note_id: None,
        light: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--id" => args.note_id = Some(it.next().ok_or("--id needs a note id")?),
            "--light" => args.light = true,
            "-h" | "--help" => {
                println!("usage: slate-gui [--id <note-id>] [--light]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn pick_font(installed: &[String], wanted: &[&str]) -> SharedString {
    wanted
        .iter()
        .find(|name| installed.iter().any(|f| f == *name))
        .unwrap_or(&wanted[0])
        .to_string()
        .into()
}

fn main() {
    // GPUI reports platform and renderer failures through `log`; RUST_LOG=info shows them.
    env_logger::init();
    let result = parse_args().and_then(|args| {
        let db = app_core::storage::Db::open(app_core::data_dir()?.join("notes.db"))?;
        let host = NoteHost::open(db, args.note_id.as_deref())?;
        Ok((args, host))
    });
    let (args, host) = match result {
        Ok(ok) => ok,
        Err(err) => {
            eprintln!("slate-gui: {err}");
            std::process::exit(1);
        }
    };
    let theme = if args.light {
        theme::Theme::light()
    } else {
        theme::Theme::dark()
    };
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let installed = window.text_system().all_font_names();
                let fonts = window::Fonts {
                    sans: pick_font(&installed, &SANS_FONTS),
                    mono: pick_font(&installed, &MONO_FONTS),
                };
                cx.new(|_| window::SlateWindow::new(host, theme, fonts))
            },
        )
        .expect("open window");
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
