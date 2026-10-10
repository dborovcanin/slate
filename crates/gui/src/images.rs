//! Inline images: decoding, fitting and the per-note cache.
use gpui::RenderImage;
use image::{Frame, ImageFormat};
use std::sync::Arc;

/// Largest size an inline image is drawn at, in logical pixels.
pub const MAX_WIDTH: f32 = 640.0;
pub const MAX_HEIGHT: f32 = 280.0;

#[derive(Clone)]
pub enum ImageSlot {
    Loading,
    Ready {
        image: Arc<RenderImage>,
        width: f32,
        height: f32,
    },
    Failed(String),
}

/// `(w, h)` scaled down to fit the box, never up.
pub fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (max_w / w).min(max_h / h).min(1.0);
    (w * scale, h * scale)
}

/// Decode to the BGRA frame GPUI paints.
pub fn decode(bytes: &[u8]) -> Result<ImageSlot, String> {
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
    ) {
        return Err("unsupported image format".into());
    }
    let mut rgba = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = rgba.dimensions();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let (width, height) = fit(w as f32, h as f32, MAX_WIDTH, MAX_HEIGHT);
    Ok(ImageSlot::Ready {
        image: Arc::new(RenderImage::new(vec![Frame::new(rgba)])),
        width,
        height,
    })
}

/// Read and decode `src` of a note; runs off the UI thread.
pub fn load(db: app_core::storage::Db, note_id: &str, src: &str) -> Result<ImageSlot, String> {
    let sources = app_core::note_sources::NoteSourceService::new(db);
    let located = sources
        .locate_image_by_id(note_id, src)?
        .ok_or_else(|| "image not found".to_string())?;
    let bytes = sources
        .read_image(&located)?
        .ok_or_else(|| "image not found".to_string())?;
    decode(&bytes.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_scales_down_only() {
        assert_eq!(fit(100.0, 50.0, 640.0, 280.0), (100.0, 50.0));
        assert_eq!(fit(1280.0, 100.0, 640.0, 280.0), (640.0, 50.0));
        assert_eq!(fit(100.0, 560.0, 640.0, 280.0), (50.0, 280.0));
        assert_eq!(fit(0.0, 10.0, 640.0, 280.0), (0.0, 0.0));
    }

    #[test]
    fn imported_image_loads_by_its_markdown_path() {
        let dir = std::env::temp_dir().join(format!("slate-gui-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = app_core::storage::Db::open(dir.join("notes.db")).unwrap();
        let note = db
            .create_note_with_context("pics", Default::default(), None, None)
            .unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let imported = sources
            .import_image_bytes_by_id(&note.id, None, Some("image/png"), &png)
            .unwrap();
        let slot = load(db, &note.id, &imported.markdown_path).unwrap();
        assert!(matches!(slot, ImageSlot::Ready { width, .. } if width == 8.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decodes_png_and_rejects_garbage() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let Ok(ImageSlot::Ready { width, height, .. }) = decode(&png) else {
            panic!("png should decode");
        };
        assert_eq!((width, height), (4.0, 2.0));
        assert!(decode(b"not an image").is_err());
    }
}

#[cfg(test)]
mod seed {
    /// Fills the data dir with a demo note for screenshots:
    /// `cargo test seed_demo -- --ignored` with `XDG_DATA_HOME` set.
    #[test]
    #[ignore]
    fn seed_demo() {
        let db =
            app_core::storage::Db::open(app_core::data_dir().unwrap().join("notes.db")).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_fn(300, 120, |x, y| {
            image::Rgba([(x * 255 / 300) as u8, 80, (y * 2) as u8, 255])
        })
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        db.create_note_with_context("demo", Default::default(), None, None)
            .unwrap();
        let sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let img = sources
            .import_image_bytes_by_id("demo", None, Some("image/png"), &png)
            .unwrap();
        db.save_note(
            "demo",
            &format!("# Demo\nsalary := 4200\nrent := 1300\nsal + rent\n\n![Gradient]({})\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n10 EUR to USD\n", img.markdown_path),
        )
        .unwrap();
    }
}
