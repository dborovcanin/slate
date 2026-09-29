use crate::config::{load_terminal_images_config, TerminalImagesConfig, TerminalImagesMode};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::env;
use std::io::IsTerminal as _;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

const IMAGE_RECHECK_INTERVAL: Duration = Duration::from_secs(2);
const IMAGE_CACHE_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ImageKey {
    note_id: String,
    src: String,
    width: u16,
    max_rows: u16,
}

#[derive(Clone)]
struct ImageRequest {
    key: ImageKey,
    db: app_core::storage::Db,
    picker: Picker,
    max_rows: u16,
    previous_stamp: Option<u64>,
}

struct ImageResponse {
    key: ImageKey,
    result: Result<Option<(Protocol, u16, u64)>, String>,
}

struct CachedImage {
    protocol: Protocol,
    rows: u16,
    stamp: u64,
    checked_at: std::time::Instant,
}

/// Async decode/resize cache. The worker owns file/DB resolution and image
/// decoding so neither key handling nor frame rendering performs that work.
pub struct ImageRenderer {
    tx: Option<SyncSender<ImageRequest>>,
    response_tx: mpsc::Sender<ImageResponse>,
    rx: Receiver<ImageResponse>,
    cache: FxHashMap<ImageKey, CachedImage>,
    pending: HashSet<ImageKey>,
    failures: FxHashMap<ImageKey, std::time::Instant>,
}

impl ImageRenderer {
    pub fn new() -> Self {
        let (response_tx, rx) = mpsc::channel();
        Self {
            tx: None,
            response_tx,
            rx,
            cache: FxHashMap::default(),
            pending: HashSet::new(),
            failures: FxHashMap::default(),
        }
    }

    pub fn request(
        &mut self,
        db: &app_core::storage::Db,
        note_id: &str,
        src: &str,
        width: u16,
        max_rows: u16,
        picker: &Picker,
    ) {
        if width == 0 || max_rows == 0 {
            return;
        }
        let key = ImageKey {
            note_id: note_id.to_owned(),
            src: src.to_owned(),
            width,
            max_rows,
        };
        if self.pending.contains(&key) {
            return;
        }
        let now = std::time::Instant::now();
        if self.failures.get(&key).is_some_and(|until| *until > now) {
            return;
        }
        self.failures.remove(&key);
        let previous_stamp = match self.cache.get(&key) {
            Some(cached) if now.duration_since(cached.checked_at) < IMAGE_RECHECK_INTERVAL => {
                return;
            }
            Some(cached) => Some(cached.stamp),
            None => None,
        };
        let request = ImageRequest {
            key: key.clone(),
            db: db.clone(),
            picker: picker.clone(),
            max_rows,
            previous_stamp,
        };
        if self.tx.is_none() {
            let (tx, requests) = mpsc::sync_channel::<ImageRequest>(8);
            let responses = self.response_tx.clone();
            if std::thread::Builder::new()
                .name("slate-image-decode".into())
                .spawn(move || {
                    while let Ok(request) = requests.recv() {
                        if responses.send(decode_image_request(request)).is_err() {
                            break;
                        }
                    }
                })
                .is_ok()
            {
                self.tx = Some(tx);
            } else {
                return;
            }
        }
        if self
            .tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(request).is_ok())
        {
            self.pending.insert(key);
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(response) = self.rx.try_recv() {
            self.pending.remove(&response.key);
            match response.result {
                Ok(Some((protocol, rows, stamp))) => {
                    self.failures.remove(&response.key);
                    if self.cache.len() >= IMAGE_CACHE_LIMIT {
                        if let Some(oldest) = self
                            .cache
                            .iter()
                            .min_by_key(|(_, image)| image.checked_at)
                            .map(|(key, _)| key.clone())
                        {
                            self.cache.remove(&oldest);
                        }
                    }
                    self.cache.insert(
                        response.key,
                        CachedImage {
                            protocol,
                            rows,
                            stamp,
                            checked_at: std::time::Instant::now(),
                        },
                    );
                    changed = true;
                }
                Ok(None) => {
                    if let Some(cached) = self.cache.get_mut(&response.key) {
                        cached.checked_at = std::time::Instant::now();
                    }
                }
                Err(_) => {
                    self.cache.remove(&response.key);
                    changed = true;
                    if self.failures.len() >= IMAGE_CACHE_LIMIT {
                        if let Some(oldest) = self
                            .failures
                            .iter()
                            .min_by_key(|(_, until)| *until)
                            .map(|(key, _)| key.clone())
                        {
                            self.failures.remove(&oldest);
                        }
                    }
                    self.failures.insert(
                        response.key,
                        std::time::Instant::now() + IMAGE_RECHECK_INTERVAL,
                    );
                }
            }
        }
        changed
    }

    pub fn rows(&self, note_id: &str, src: &str, width: u16, max_rows: u16) -> Option<usize> {
        let key = ImageKey {
            note_id: note_id.to_owned(),
            src: src.to_owned(),
            width,
            max_rows,
        };
        self.cache.get(&key).map(|image| usize::from(image.rows))
    }

    pub fn failed(&self, note_id: &str, src: &str, width: u16, max_rows: u16) -> bool {
        let key = ImageKey {
            note_id: note_id.to_owned(),
            src: src.to_owned(),
            width,
            max_rows,
        };
        self.failures
            .get(&key)
            .is_some_and(|until| *until > std::time::Instant::now())
    }

    pub fn render(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        note_id: &str,
        src: &str,
        width: u16,
        max_rows: u16,
        x: u16,
        y: u16,
    ) -> bool {
        use ratatui::widgets::Widget as _;
        let key = ImageKey {
            note_id: note_id.to_owned(),
            src: src.to_owned(),
            width,
            max_rows,
        };
        let Some(image) = self.cache.get(&key) else {
            return false;
        };
        let image_size = image.protocol.size();
        let available_size = ratatui::layout::Size::new(width, image.rows);
        let (offset_x, offset_y) = centered_image_offset(available_size, image_size);
        ratatui_image::Image::new(&image.protocol).render(
            ratatui::layout::Rect::new(
                buf.area.x.saturating_add(x).saturating_add(offset_x),
                buf.area.y.saturating_add(y).saturating_add(offset_y),
                image_size.width.min(width),
                image_size.height.min(image.rows),
            ),
            buf,
        );
        true
    }
}

fn centered_image_offset(
    available: ratatui::layout::Size,
    image: ratatui::layout::Size,
) -> (u16, u16) {
    (
        available
            .width
            .saturating_sub(image.width.min(available.width))
            / 2,
        available
            .height
            .saturating_sub(image.height.min(available.height))
            / 2,
    )
}

impl Default for ImageRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_image_request(request: ImageRequest) -> ImageResponse {
    let result = load_image(&request);
    ImageResponse {
        key: request.key,
        result,
    }
}

fn load_image(request: &ImageRequest) -> Result<Option<(Protocol, u16, u64)>, String> {
    let missing = || "image source is missing".to_string();
    let sources = app_core::note_sources::NoteSourceService::new(request.db.clone());
    let source = sources
        .locate_image_by_id(&request.key.note_id, &request.key.src)?
        .ok_or_else(missing)?;
    // The stamp comes from file metadata or the image row, so an unchanged
    // image is rechecked without reading or decoding its bytes.
    let stamp = sources.image_stamp(&source)?.ok_or_else(missing)?;
    if request.previous_stamp == Some(stamp) {
        return Ok(None);
    }
    let bytes = sources.read_image(&source)?.ok_or_else(missing)?.bytes;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION);
    limits.max_image_height = Some(app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION);
    limits.max_alloc = Some(420_000_000);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    if u64::from(image.width()).saturating_mul(u64::from(image.height()))
        > app_core::note_sources::MAX_NOTE_IMAGE_PIXELS
    {
        return Err("image dimensions exceed the supported limit".to_string());
    }
    let rows = image_rows_for(
        image.width(),
        image.height(),
        request.key.width,
        request.max_rows,
        request.picker.font_size(),
    );
    let protocol = request
        .picker
        .new_protocol(image, Size::new(request.key.width, rows), Resize::Fit(None))
        .map_err(|error| error.to_string())?;
    Ok(Some((protocol, rows, stamp)))
}

/// Rows needed to show the image at `cell_width` columns, from the pixel
/// aspect ratio and the terminal cell size.
fn image_rows_for(
    pixel_width: u32,
    pixel_height: u32,
    cell_width: u16,
    max_rows: u16,
    font: FontSize,
) -> u16 {
    if pixel_width == 0 || cell_width == 0 || max_rows == 0 || font.height == 0 {
        return 0;
    }
    let cell_aspect = f64::from(font.width) / f64::from(font.height);
    ((f64::from(pixel_height) / f64::from(pixel_width) * f64::from(cell_width) * cell_aspect).ceil()
        as u16)
        .clamp(1, max_rows)
}

/// Terminal graphics capabilities. Detection queries the terminal, so it
/// runs on the first image preview rather than at startup.
#[derive(Clone, Debug)]
pub struct GraphicsContext {
    pub config: TerminalImagesConfig,
    picker: Option<Picker>,
}

impl GraphicsContext {
    /// Loads the `[terminal]` config and queries the terminal. Call only
    /// while the terminal session is active (raw mode).
    pub fn detect() -> Self {
        let config = load_terminal_images_config();
        if config.mode == TerminalImagesMode::Off || (inside_tmux() && !tmux_passthrough_enabled())
        {
            return Self::disabled(config);
        }
        // The query also reports the cell size in pixels, which forced
        // protocols need to size images correctly.
        let picker = if std::io::stdin().is_terminal() {
            Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
        } else {
            Picker::halfblocks()
        };
        Self::with_picker(config, picker)
    }

    /// Applies the configured mode to an already-detected picker. In `auto`
    /// mode the picker's own choice stands.
    pub fn with_picker(config: TerminalImagesConfig, mut picker: Picker) -> Self {
        let forced = match config.mode {
            TerminalImagesMode::Off => return Self::disabled(config),
            TerminalImagesMode::Auto => None,
            TerminalImagesMode::Sixel => Some(ProtocolType::Sixel),
            TerminalImagesMode::Kitty => Some(ProtocolType::Kitty),
            TerminalImagesMode::Iterm2 => Some(ProtocolType::Iterm2),
            TerminalImagesMode::Halfblocks => Some(ProtocolType::Halfblocks),
        };
        if let Some(protocol) = forced {
            picker.set_protocol_type(protocol);
        }
        Self {
            config,
            picker: Some(picker),
        }
    }

    fn disabled(config: TerminalImagesConfig) -> Self {
        Self {
            config,
            picker: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.picker.is_some()
    }

    #[cfg(test)]
    pub fn protocol(&self) -> Option<ProtocolType> {
        self.picker.as_ref().map(Picker::protocol_type)
    }

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    pub fn max_rows(&self) -> usize {
        self.config.max_rows
    }
}

fn inside_tmux() -> bool {
    env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

fn tmux_passthrough_enabled() -> bool {
    // `-A` includes values inherited from the global/session options, which
    // is where `set -g allow-passthrough on` puts it.
    std::process::Command::new("tmux")
        .args(["show-options", "-A", "-p", "-v", "allow-passthrough"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success() && matches!(output.stdout.trim_ascii(), b"on" | b"all")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::io::Cursor;

    #[test]
    fn image_row_height_uses_aspect_ratio_and_respects_cap() {
        let font = FontSize::new(10, 20);
        assert_eq!(image_rows_for(1600, 900, 80, 40, font), 23);
        assert_eq!(image_rows_for(400, 900, 80, 40, font), 40);
        assert_eq!(image_rows_for(400, 900, 80, 15, font), 15);
        assert_eq!(image_rows_for(0, 900, 80, 15, font), 0);
        // Narrower cells fit fewer pixels per column, so the same image
        // needs fewer rows at the same column width.
        assert_eq!(image_rows_for(1600, 900, 80, 40, FontSize::new(8, 20)), 18);
    }

    #[test]
    fn image_is_centered_in_available_preview_area() {
        assert_eq!(
            centered_image_offset(
                ratatui::layout::Size::new(80, 20),
                ratatui::layout::Size::new(30, 12),
            ),
            (25, 4)
        );
        assert_eq!(
            centered_image_offset(
                ratatui::layout::Size::new(30, 12),
                ratatui::layout::Size::new(30, 12),
            ),
            (0, 0)
        );
    }

    #[test]
    fn image_worker_decodes_and_caches_protocol_off_the_caller() {
        let path =
            std::env::temp_dir().join(format!("slate-image-worker-{}.db", ulid::Ulid::new()));
        let db = app_core::storage::Db::open(path.clone()).expect("open image test db");
        db.save_note("image-note", "test image").expect("save note");
        let encode_png = |width, height| {
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(width, height)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .expect("encode test image");
            bytes
        };
        let bytes = encode_png(10, 10);
        let sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let imported = sources
            .import_image_bytes_by_id(
                "image-note",
                Some("test.png"),
                Some("image/png"),
                bytes.get_ref(),
            )
            .expect("import test image");

        let mut renderer = ImageRenderer::new();
        renderer.request(
            &db,
            "image-note",
            &imported.markdown_path,
            20,
            15,
            &Picker::halfblocks(),
        );
        for _ in 0..200 {
            if renderer.poll() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            renderer.rows("image-note", &imported.markdown_path, 20, 15),
            Some(10)
        );
        let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 30, 20));
        assert!(renderer.render(
            &mut buffer,
            "image-note",
            &imported.markdown_path,
            20,
            15,
            0,
            0,
        ));

        let replacement = encode_png(10, 2);
        db.write_note_image_bytes(
            "image-note",
            &imported.image_id,
            Some("test.png"),
            Some("image/png"),
            replacement.get_ref(),
        )
        .expect("replace stored image");
        std::thread::sleep(IMAGE_RECHECK_INTERVAL + Duration::from_millis(20));
        renderer.request(
            &db,
            "image-note",
            &imported.markdown_path,
            20,
            15,
            &Picker::halfblocks(),
        );
        for _ in 0..200 {
            if renderer.poll()
                && renderer.rows("image-note", &imported.markdown_path, 20, 15) == Some(2)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            renderer.rows("image-note", &imported.markdown_path, 20, 15),
            Some(2)
        );

        drop(renderer);
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    fn config(mode: TerminalImagesMode) -> TerminalImagesConfig {
        TerminalImagesConfig { mode, max_rows: 15 }
    }

    #[test]
    fn mode_off_disables_graphics() {
        let ctx =
            GraphicsContext::with_picker(config(TerminalImagesMode::Off), Picker::halfblocks());
        assert!(!ctx.is_enabled());
        assert_eq!(ctx.protocol(), None);
    }

    #[test]
    fn forced_modes_override_protocol_but_keep_detected_cell_size() {
        for (mode, protocol) in [
            (TerminalImagesMode::Sixel, ProtocolType::Sixel),
            (TerminalImagesMode::Kitty, ProtocolType::Kitty),
            (TerminalImagesMode::Iterm2, ProtocolType::Iterm2),
            (TerminalImagesMode::Halfblocks, ProtocolType::Halfblocks),
        ] {
            // Stands in for a picker from a terminal query that reported 8x16 cells.
            #[allow(deprecated)]
            let detected = Picker::from_fontsize(FontSize::new(8, 16));
            let ctx = GraphicsContext::with_picker(config(mode), detected);
            assert!(ctx.is_enabled());
            assert_eq!(ctx.protocol(), Some(protocol));
            let font = ctx.picker().map(Picker::font_size).expect("picker");
            assert_eq!((font.width, font.height), (8, 16));
        }
    }

    #[test]
    fn auto_mode_keeps_the_detected_protocol() {
        let mut detected = Picker::halfblocks();
        detected.set_protocol_type(ProtocolType::Iterm2);
        let ctx = GraphicsContext::with_picker(config(TerminalImagesMode::Auto), detected);
        assert_eq!(ctx.protocol(), Some(ProtocolType::Iterm2));
    }
}
