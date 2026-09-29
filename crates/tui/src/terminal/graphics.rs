use crate::config::{load_terminal_images_config, TerminalImagesConfig};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::env;
use std::io::{IsTerminal as _, Read as _};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::time::{Duration, UNIX_EPOCH};

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
    stamp: Option<u64>,
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
            Some(cached) => cached.stamp,
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
        loop {
            let response = match self.rx.try_recv() {
                Ok(response) => response,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
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
                            stamp: Some(stamp),
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
    let sources = app_core::note_sources::NoteSourceService::new(request.db);
    let resolved =
        sources.resolve_image_markdown_source_by_id(&request.key.note_id, &request.key.src);
    let result = resolved.and_then(|resolved| {
        let Some(resolved) = resolved else {
            return Err("image source is missing".to_string());
        };
        let (bytes, stamp) = if let Some(payload) = resolved.strip_prefix("data:") {
            use base64::Engine as _;
            let Some((header, encoded)) = payload.split_once(',') else {
                return Err("invalid image data URL".to_string());
            };
            if !header.ends_with(";base64") || encoded.len() > 24 * 1024 * 1024 {
                return Err("unsupported or oversized image data URL".to_string());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| format!("invalid image data: {error}"))?;
            let stamp = hash_image_source(&bytes);
            (bytes, Some(stamp))
        } else if resolved.contains("://") {
            return Err("remote images are not loaded".to_string());
        } else {
            let path = std::path::Path::new(&resolved);
            let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
            if !metadata.is_file()
                || metadata.len() > app_core::note_sources::MAX_NOTE_IMAGE_BYTES as u64
            {
                return Err(
                    "image is not a supported local file or exceeds the size limit".to_string(),
                );
            }
            let modified = metadata
                .modified()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            let stamp = modified ^ metadata.len().rotate_left(17);
            let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            file.take((app_core::note_sources::MAX_NOTE_IMAGE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > app_core::note_sources::MAX_NOTE_IMAGE_BYTES {
                return Err("image exceeds the size limit".to_string());
            }
            (bytes, Some(stamp))
        };
        if request.previous_stamp == stamp {
            return Ok(None);
        }
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| error.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION);
        limits.max_image_height = Some(app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION);
        limits.max_alloc = Some(420_000_000);
        reader.limits(limits);
        let image = reader.decode().map_err(|error| error.to_string())?;
        if image.width() > app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION
            || image.height() > app_core::note_sources::MAX_NOTE_IMAGE_DIMENSION
            || u64::from(image.width()).saturating_mul(u64::from(image.height()) as u64)
                > app_core::note_sources::MAX_NOTE_IMAGE_PIXELS
        {
            return Err("image dimensions exceed the supported limit".to_string());
        }
        let rows = image_rows_for(
            image.width(),
            image.height(),
            request.key.width,
            request.max_rows,
        );
        let protocol = request
            .picker
            .new_protocol(image, Size::new(request.key.width, rows), Resize::Fit(None))
            .map_err(|error| error.to_string())?;
        Ok(Some((protocol, rows, stamp.unwrap_or_default())))
    });
    ImageResponse {
        key: request.key,
        result,
    }
}

fn hash_image_source(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn image_rows_for(pixel_width: u32, pixel_height: u32, cell_width: u16, max_rows: u16) -> u16 {
    if pixel_width == 0 || cell_width == 0 || max_rows == 0 {
        return 0;
    }
    ((f64::from(pixel_height) / f64::from(pixel_width) * f64::from(cell_width) * 0.5).ceil() as u16)
        .clamp(1, max_rows)
}

#[derive(Clone, Debug)]
pub struct GraphicsContext {
    pub config: TerminalImagesConfig,
    pub protocol: Option<ProtocolType>,
    pub picker: Option<Picker>,
}

impl GraphicsContext {
    pub fn new() -> Self {
        let config = load_terminal_images_config();
        let inside_tmux = config.mode != "off" && tmux_detected();
        if inside_tmux && !tmux_passthrough_enabled() {
            return Self {
                config,
                protocol: None,
                picker: None,
            };
        }
        Self::from_config(config)
    }

    pub fn from_config(config: TerminalImagesConfig) -> Self {
        if config.mode == "off" {
            return Self {
                config,
                protocol: None,
                picker: None,
            };
        }

        let mut picker = if config.mode == "auto" {
            let queried = if std::io::stdin().is_terminal() {
                Picker::from_query_stdio().ok()
            } else {
                None
            };
            queried.unwrap_or_else(Picker::halfblocks)
        } else {
            Picker::halfblocks()
        };

        let detected_protocol = match config.mode.as_str() {
            "sixel" => Some(ProtocolType::Sixel),
            "kitty" => Some(ProtocolType::Kitty),
            "iterm2" => Some(ProtocolType::Iterm2),
            "halfblocks" => Some(ProtocolType::Halfblocks),
            "auto" => {
                let queried_proto = picker.protocol_type();
                if queried_proto != ProtocolType::Halfblocks {
                    Some(queried_proto)
                } else if let Some(env_proto) = detect_protocol_from_env() {
                    Some(env_proto)
                } else {
                    Some(ProtocolType::Halfblocks)
                }
            }
            _ => Some(ProtocolType::Halfblocks),
        };

        if let Some(proto) = detected_protocol {
            picker.set_protocol_type(proto);
        }

        Self {
            config,
            protocol: detected_protocol,
            picker: Some(picker),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.protocol.is_some() && self.picker.is_some()
    }

    #[cfg(test)]
    pub fn is_native_graphics(&self) -> bool {
        matches!(
            self.protocol,
            Some(ProtocolType::Sixel) | Some(ProtocolType::Kitty) | Some(ProtocolType::Iterm2)
        )
    }

    #[cfg(test)]
    pub fn protocol(&self) -> Option<ProtocolType> {
        self.protocol
    }

    #[cfg(test)]
    pub fn protocol_name(&self) -> &'static str {
        match self.protocol {
            Some(ProtocolType::Sixel) => "sixel",
            Some(ProtocolType::Kitty) => "kitty",
            Some(ProtocolType::Iterm2) => "iterm2",
            Some(ProtocolType::Halfblocks) => "halfblocks",
            None => "off",
        }
    }

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    pub fn max_rows(&self) -> usize {
        self.config.max_rows
    }
}

impl Default for GraphicsContext {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphicsContext {
    /// Returns a disabled context that does not query the terminal.
    /// Used as a placeholder in `TerminalApp` before the session is established.
    pub fn disabled() -> Self {
        Self {
            config: crate::config::TerminalImagesConfig {
                mode: "off".to_string(),
                max_rows: 0,
            },
            protocol: None,
            picker: None,
        }
    }
}

fn tmux_detected() -> bool {
    env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux")
}

fn tmux_passthrough_enabled() -> bool {
    std::process::Command::new("tmux")
        .args(["show-option", "-p", "-v", "allow-passthrough"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout.starts_with(b"on"))
}

pub fn detect_protocol_from_env() -> Option<ProtocolType> {
    if env::var("GHOSTTY_RESOURCES_DIR").is_ok() {
        return Some(ProtocolType::Kitty);
    }
    if let Ok(term) = env::var("TERM") {
        let lower = term.to_ascii_lowercase();
        if lower.contains("ghostty") || lower.contains("kitty") {
            return Some(ProtocolType::Kitty);
        }
        if lower.contains("foot") {
            return Some(ProtocolType::Sixel);
        }
    }
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return Some(ProtocolType::Kitty);
    }
    if env::var("FOOT_TERMINAL").is_ok() {
        return Some(ProtocolType::Sixel);
    }
    if let Ok(prog) = env::var("TERM_PROGRAM") {
        let lower = prog.to_ascii_lowercase();
        if lower.contains("iterm") {
            return Some(ProtocolType::Iterm2);
        }
        if lower.contains("wezterm") {
            return Some(ProtocolType::Kitty);
        }
        if lower.contains("ghostty") {
            return Some(ProtocolType::Kitty);
        }
        if lower.contains("foot") {
            return Some(ProtocolType::Sixel);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::io::Cursor;

    #[test]
    fn image_row_height_uses_aspect_ratio_and_respects_cap() {
        assert_eq!(image_rows_for(1600, 900, 80, 40), 23);
        assert_eq!(image_rows_for(400, 900, 80, 40), 40);
        assert_eq!(image_rows_for(400, 900, 80, 15), 15);
        assert_eq!(image_rows_for(0, 900, 80, 15), 0);
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

    #[test]
    fn mode_off_disables_graphics() {
        let ctx = GraphicsContext::from_config(TerminalImagesConfig {
            mode: "off".to_string(),
            max_rows: 15,
        });
        assert!(!ctx.is_enabled());
        assert_eq!(ctx.protocol(), None);
        assert_eq!(ctx.protocol_name(), "off");
    }

    #[test]
    fn mode_sixel_forces_sixel() {
        let ctx = GraphicsContext::from_config(TerminalImagesConfig {
            mode: "sixel".to_string(),
            max_rows: 15,
        });
        assert!(ctx.is_enabled());
        assert_eq!(ctx.protocol(), Some(ProtocolType::Sixel));
        assert_eq!(ctx.protocol_name(), "sixel");
        assert!(ctx.is_native_graphics());
    }

    #[test]
    fn mode_kitty_forces_kitty() {
        let ctx = GraphicsContext::from_config(TerminalImagesConfig {
            mode: "kitty".to_string(),
            max_rows: 15,
        });
        assert!(ctx.is_enabled());
        assert_eq!(ctx.protocol(), Some(ProtocolType::Kitty));
        assert_eq!(ctx.protocol_name(), "kitty");
        assert!(ctx.is_native_graphics());
    }

    #[test]
    fn mode_halfblocks_forces_halfblocks() {
        let ctx = GraphicsContext::from_config(TerminalImagesConfig {
            mode: "halfblocks".to_string(),
            max_rows: 15,
        });
        assert!(ctx.is_enabled());
        assert_eq!(ctx.protocol(), Some(ProtocolType::Halfblocks));
        assert_eq!(ctx.protocol_name(), "halfblocks");
        assert!(!ctx.is_native_graphics());
    }
}
