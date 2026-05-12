mod pdf_layout;
mod pdf_style;

use pdf_layout::{
    collect_image_sources, render_markdown_to_pages, DrawOp, Page, PdfImageObject,
    PDF_PAGE_HEIGHT_PT, PDF_PAGE_WIDTH_PT,
};
use pdf_style::*;

use app_core::note_sources::NoteSourceService;
#[cfg(feature = "gui")]
use app_core::AppCore;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use flate2::{write::ZlibEncoder, Compression};
use image::GenericImageView as _;
use rustc_hash::FxHashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(feature = "gui")]
use tauri::State;
use ttf_parser::Face;
use url::Url;

#[allow(unused_imports)]
pub use pdf_style::{PdfExportPalette, PdfRgbColor};

const MAX_PDF_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PDF_TOTAL_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const PDF_TAB_WIDTH: usize = 4;

fn resolve_export_path(path: &str) -> Result<PathBuf, String> {
    let raw = path.trim().trim_matches(|c| c == '"' || c == '\'');
    if raw.is_empty() {
        return Err("export path is empty".to_string());
    }

    let resolved = if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let home = resolve_home_dir().ok_or_else(|| {
            "home directory is not set; cannot expand '~' in export path".to_string()
        })?;
        if raw == "~" {
            home
        } else {
            let rest = &raw[2..];
            home.join(rest)
        }
    } else if raw.len() >= "file://".len() && raw[.."file://".len()].eq_ignore_ascii_case("file://")
    {
        let uri =
            Url::parse(raw).map_err(|e| format!("invalid file URL export path '{raw}': {e}"))?;
        uri.to_file_path()
            .map_err(|_| format!("invalid file URL export path '{raw}'"))?
    } else {
        PathBuf::from(raw)
    };

    if resolved.is_dir() {
        return Err(format!(
            "export path points to a directory: {}",
            resolved.display()
        ));
    }

    let parent = resolved
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Err(format!(
            "export directory does not exist: {}",
            parent.display()
        ));
    }
    if !parent.is_dir() {
        return Err(format!(
            "export parent is not a directory: {}",
            parent.display()
        ));
    }

    Ok(resolved)
}

fn resolve_home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty());
    if home.is_some() {
        return home.map(PathBuf::from);
    }
    let profile = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty());
    if profile.is_some() {
        return profile.map(PathBuf::from);
    }
    let drive = std::env::var_os("HOMEDRIVE").filter(|v| !v.is_empty());
    let path = std::env::var_os("HOMEPATH").filter(|v| !v.is_empty());
    match (drive, path) {
        (Some(drive), Some(path)) => {
            let mut full = PathBuf::from(drive);
            full.push(path);
            Some(full)
        }
        _ => None,
    }
}

fn default_unicode_font_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        PathBuf::from("/usr/share/fonts/TTF/DejaVuSans.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/noto/NotoSans-Regular.ttf"),
        PathBuf::from("/Library/Fonts/Arial Unicode.ttf"),
        PathBuf::from("/Library/Fonts/Arial Unicode MS.ttf"),
        PathBuf::from("/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\arialuni.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\seguiemj.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\segoeui.ttf"),
    ]
}

fn load_unicode_pdf_font_asset() -> Option<PdfUnicodeFontAsset> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("SLATE_PDF_UNICODE_FONT") {
        let candidate = PathBuf::from(path);
        if !candidate.as_os_str().is_empty() {
            candidates.push(candidate);
        }
    }
    candidates.extend(default_unicode_font_candidates());

    for path in candidates {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(face) = Face::parse(&bytes, 0) else {
            continue;
        };
        let bbox = face.global_bounding_box();
        let mut flags = 32u32; // Nonsymbolic
        if face.is_monospaced() {
            flags |= 1;
        }
        if face.is_italic() {
            flags |= 64;
        }
        let fallback_gid = face.glyph_index('?').map(|id| id.0).unwrap_or(0);
        let units_per_em = face.units_per_em();
        let ascent = face.ascender();
        let descent = face.descender();
        let cap_height = face.capital_height().unwrap_or(face.ascender());

        let num_glyphs = face.number_of_glyphs() as usize;
        let mut glyph_advances = vec![0u16; num_glyphs];
        for gid in 0..num_glyphs {
            if let Some(adv) = face.glyph_hor_advance(ttf_parser::GlyphId(gid as u16)) {
                glyph_advances[gid] = adv;
            }
        }
        let mut bmp_glyph_ids = vec![0u16; 65536];
        for cp in 0x0020u32..=0xFFFFu32 {
            if let Some(ch) = char::from_u32(cp) {
                if let Some(gid) = face.glyph_index(ch) {
                    if gid.0 != 0 {
                        bmp_glyph_ids[cp as usize] = gid.0;
                    }
                }
            }
        }

        return Some(PdfUnicodeFontAsset {
            bytes,
            units_per_em,
            ascent,
            descent,
            cap_height,
            bbox_min_x: bbox.x_min,
            bbox_min_y: bbox.y_min,
            bbox_max_x: bbox.x_max,
            bbox_max_y: bbox.y_max,
            flags,
            stem_v: 80,
            fallback_gid,
            glyph_advances,
            bmp_glyph_ids,
        });
    }
    None
}

pub(crate) fn resolve_unicode_pdf_font_asset() -> Option<PdfUnicodeFontAsset> {
    static CACHE: OnceLock<Option<PdfUnicodeFontAsset>> = OnceLock::new();
    CACHE.get_or_init(load_unicode_pdf_font_asset).clone()
}

#[cfg(feature = "gui")]
#[tauri::command]
pub async fn export_to_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_to_file_blocking(&path, &content))
        .await
        .map_err(|e| format!("export task join failed: {e}"))?
}

pub fn export_to_file_blocking(path: &str, content: &str) -> Result<(), String> {
    let resolved = resolve_export_path(path)?;
    fs::write(&resolved, content)
        .map_err(|e| format!("Failed to write file '{}': {e}", resolved.display()))
}

#[cfg(feature = "gui")]
#[tauri::command]
pub async fn export_to_pdf(
    core: State<'_, AppCore>,
    note_id: String,
    path: String,
    content: String,
    palette: PdfExportPalette,
) -> Result<(), String> {
    let note_sources = core.note_sources().clone();
    tauri::async_runtime::spawn_blocking(move || {
        export_markdown_to_pdf_file(&note_sources, &note_id, &path, &content, &palette)
    })
    .await
    .map_err(|e| format!("PDF export task join failed: {e}"))?
}

pub fn export_markdown_to_pdf_file(
    note_sources: &NoteSourceService,
    note_id: &str,
    path: &str,
    content: &str,
    palette: &PdfExportPalette,
) -> Result<(), String> {
    let resolved = resolve_export_path(path)?;
    let pdf_bytes = build_markdown_pdf(content, palette, |src| {
        resolve_markdown_image_bytes(note_sources, note_id, src)
            .ok()
            .flatten()
    })?;
    fs::write(&resolved, pdf_bytes)
        .map_err(|e| format!("Failed to write PDF '{}': {e}", resolved.display()))
}

fn resolve_markdown_image_bytes(
    note_sources: &NoteSourceService,
    note_id: &str,
    src: &str,
) -> Result<Option<Vec<u8>>, String> {
    let resolved = note_sources.resolve_image_markdown_source_by_id(note_id, src)?;
    let Some(value) = resolved else {
        return Ok(None);
    };

    if value.starts_with("data:") {
        let bytes = decode_data_url_image_bytes(&value)?;
        if bytes.len() as u64 > MAX_PDF_IMAGE_BYTES {
            return Ok(None);
        }
        return Ok(Some(bytes));
    }

    let size = fs::metadata(&value).map(|m| m.len()).unwrap_or(0);
    if size > MAX_PDF_IMAGE_BYTES {
        return Ok(None);
    }

    fs::read(&value)
        .map(Some)
        .map_err(|e| format!("Failed to read image '{value}': {e}"))
}

fn decode_data_url_image_bytes(data_url: &str) -> Result<Vec<u8>, String> {
    let Some(payload) = data_url.strip_prefix("data:") else {
        return Err("invalid data URL image prefix".to_string());
    };
    let Some((meta, encoded)) = payload.split_once(',') else {
        return Err("invalid data URL image payload".to_string());
    };
    if !meta.to_ascii_lowercase().contains(";base64") {
        return Err("unsupported non-base64 image data URL".to_string());
    }
    BASE64_STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("invalid base64 image payload: {e}"))
}

fn build_markdown_pdf(
    content: &str,
    palette: &PdfExportPalette,
    mut resolve_image: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let image_sources = collect_image_sources(content);
    let mut image_name_by_src: FxHashMap<String, String> = FxHashMap::default();
    let mut image_assets: Vec<(String, PdfImageObject)> = Vec::new();
    let mut total_image_bytes: u64 = 0;

    for src in image_sources {
        if total_image_bytes >= MAX_PDF_TOTAL_IMAGE_BYTES {
            break;
        }
        let Some(image_bytes) = resolve_image(src.as_str()) else {
            continue;
        };
        let byte_len = image_bytes.len() as u64;
        if total_image_bytes.saturating_add(byte_len) > MAX_PDF_TOTAL_IMAGE_BYTES {
            continue;
        }
        let Ok(image_obj) = decode_pdf_image(&image_bytes) else {
            continue;
        };
        total_image_bytes += byte_len;
        let name = format!("Im{}", image_assets.len() + 1);
        image_name_by_src.insert(src, name.clone());
        image_assets.push((name, image_obj));
    }

    let pages = render_markdown_to_pages(content, palette, &image_name_by_src, &image_assets);
    serialize_pdf(pages, image_assets)
}

fn decode_pdf_image(bytes: &[u8]) -> Result<PdfImageObject, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| format!("failed to decode image bytes for PDF export: {e}"))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err("image has invalid dimensions".to_string());
    }
    let rgb = image.to_rgb8();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(rgb.as_raw())
        .map_err(|e| format!("failed to encode image pixels for PDF: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("failed to finalize image compression for PDF: {e}"))?;

    Ok(PdfImageObject {
        width,
        height,
        compressed_rgb: compressed,
    })
}

fn serialize_pdf(
    pages: Vec<Page>,
    image_assets: Vec<(String, PdfImageObject)>,
) -> Result<Vec<u8>, String> {
    let unicode_font = resolve_unicode_pdf_font_asset();
    let page_count = pages.len().max(1);
    let actual_pages = if pages.is_empty() {
        vec![Page::default()]
    } else {
        pages
    };

    let mut next_object_id = 8usize;
    let unicode_ids = if unicode_font.is_some() {
        let ids = (
            next_object_id,
            next_object_id + 1,
            next_object_id + 2,
            next_object_id + 3,
            next_object_id + 4,
        );
        next_object_id += 5;
        Some(ids)
    } else {
        None
    };

    let image_count = image_assets.len();
    let image_first_id = next_object_id;
    let page_first_id = image_first_id + image_count;

    let mut image_obj_id_by_name: FxHashMap<String, usize> = FxHashMap::default();
    for (idx, (name, _)) in image_assets.iter().enumerate() {
        image_obj_id_by_name.insert(name.clone(), image_first_id + idx);
    }

    let mut objects: Vec<Vec<u8>> = Vec::new();

    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>\n".to_vec());

    let mut kids = String::new();
    for i in 0..page_count {
        let page_obj_id = page_first_id + (i * 2);
        kids.push_str(&format!("{page_obj_id} 0 R "));
    }
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>\n",
            kids, page_count
        )
        .into_bytes(),
    );

    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Oblique >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-BoldOblique >>\n".to_vec());

    let mut unicode_cmap: FxHashMap<u16, char> = FxHashMap::default();
    let streams: Vec<String> = actual_pages
        .iter()
        .map(|page| page_stream(page, unicode_font.as_ref(), &mut unicode_cmap))
        .collect();

    if let (Some(font), Some((type0_id, cid_id, descriptor_id, font_file_id, to_unicode_id))) =
        (unicode_font.as_ref(), unicode_ids)
    {
        let base_font_name = "SlateUnicode";
        objects.push(
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /Identity-H /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>\n",
                base_font_name, cid_id, to_unicode_id
            )
            .into_bytes(),
        );
        let default_width = 1000i32;
        objects.push(
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R /CIDToGIDMap /Identity /DW {} >>\n",
                base_font_name, descriptor_id, default_width
            )
            .into_bytes(),
        );
        let scale = 1000.0f32 / (font.units_per_em as f32);
        let ascent = (font.ascent as f32 * scale).round() as i32;
        let descent = (font.descent as f32 * scale).round() as i32;
        let cap_height = (font.cap_height as f32 * scale).round() as i32;
        let bbox_min_x = (font.bbox_min_x as f32 * scale).round() as i32;
        let bbox_min_y = (font.bbox_min_y as f32 * scale).round() as i32;
        let bbox_max_x = (font.bbox_max_x as f32 * scale).round() as i32;
        let bbox_max_y = (font.bbox_max_y as f32 * scale).round() as i32;
        objects.push(
            format!(
                "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {} 0 R >>\n",
                base_font_name,
                font.flags,
                bbox_min_x,
                bbox_min_y,
                bbox_max_x,
                bbox_max_y,
                ascent,
                descent,
                cap_height,
                font.stem_v,
                font_file_id
            )
            .into_bytes(),
        );
        let mut font_stream = format!(
            "<< /Length {} /Length1 {} >>\nstream\n",
            font.bytes.len(),
            font.bytes.len()
        )
        .into_bytes();
        font_stream.extend_from_slice(&font.bytes);
        font_stream.extend_from_slice(b"\nendstream\n");
        objects.push(font_stream);
        let to_unicode_stream = build_to_unicode_cmap(&unicode_cmap);
        let mut to_unicode_obj =
            format!("<< /Length {} >>\nstream\n", to_unicode_stream.len()).into_bytes();
        to_unicode_obj.extend_from_slice(&to_unicode_stream);
        to_unicode_obj.extend_from_slice(b"\nendstream\n");
        objects.push(to_unicode_obj);

        debug_assert_eq!(type0_id, 8);
        debug_assert_eq!(cid_id, 9);
    }

    for (_, image) in &image_assets {
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            image.width,
            image.height,
            image.compressed_rgb.len()
        )
        .into_bytes();
        body.extend_from_slice(&image.compressed_rgb);
        body.extend_from_slice(b"\nendstream\n");
        objects.push(body);
    }

    for (idx, page) in actual_pages.iter().enumerate() {
        let page_obj_id = page_first_id + (idx * 2);
        let content_obj_id = page_obj_id + 1;

        let mut xobjects = String::new();
        for name in &page.used_images {
            if let Some(id) = image_obj_id_by_name.get(name) {
                xobjects.push_str(&format!("/{name} {id} 0 R "));
            }
        }
        let xobject_section = if xobjects.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {xobjects} >>")
        };
        let unicode_font_section = if let Some((type0_id, _, _, _, _)) = unicode_ids {
            format!(" /F6 {} 0 R", type0_id)
        } else {
            String::new()
        };

        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_WIDTH_PT:.0} {PDF_PAGE_HEIGHT_PT:.0}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R /F5 7 0 R{unicode_font_section} >>{xobject_section} >> /Contents {content_obj_id} 0 R >>\n"
            )
            .into_bytes(),
        );

        let stream = &streams[idx];
        let mut content_obj = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        content_obj.extend_from_slice(stream.as_bytes());
        content_obj.extend_from_slice(b"\nendstream\n");
        objects.push(content_obj);
    }

    debug_assert_eq!(actual_pages.len(), page_count);
    serialize_objects(objects)
}

fn page_stream(
    page: &Page,
    unicode_font: Option<&PdfUnicodeFontAsset>,
    unicode_cmap: &mut FxHashMap<u16, char>,
) -> String {
    let mut out = String::new();
    for op in &page.ops {
        match op {
            DrawOp::Text {
                font,
                size,
                x,
                y,
                color,
                text,
            } => {
                let (r, g, b) = color.as_pdf_rgb();
                let (font_resource, encoded) = if let Some(unicode) = unicode_font {
                    if text_requires_unicode_font(text) {
                        (
                            "F6",
                            encode_pdf_unicode_text_bytes(text, unicode, unicode_cmap),
                        )
                    } else {
                        (font.resource_name(), encode_pdf_text_bytes(text))
                    }
                } else {
                    (font.resource_name(), encode_pdf_text_bytes(text))
                };
                if encoded.is_empty() {
                    continue;
                }
                let hex = encode_pdf_hex_string(&encoded);
                out.push_str("BT\n");
                out.push_str(&format!("/{} {:.2} Tf\n", font_resource, size));
                out.push_str(&format!("{r:.4} {g:.4} {b:.4} rg\n"));
                out.push_str(&format!("1 0 0 1 {:.2} {:.2} Tm <{}> Tj\n", x, y, hex));
                out.push_str("ET\n");
            }
            DrawOp::Line {
                width,
                x1,
                y1,
                x2,
                y2,
                color,
            } => {
                let (r, g, b) = color.as_pdf_rgb();
                out.push_str(&format!(
                    "{r:.4} {g:.4} {b:.4} RG\n{width:.2} w\n{x1:.2} {y1:.2} m\n{x2:.2} {y2:.2} l\nS\n"
                ));
            }
            DrawOp::Image { name, x, y, w, h } => {
                out.push_str("q\n");
                out.push_str(&format!("{w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm\n/{name} Do\n"));
                out.push_str("Q\n");
            }
            DrawOp::Rect {
                x,
                y,
                w,
                h,
                fill,
                stroke_width,
                stroke,
            } => {
                if let Some(fill_color) = fill {
                    let (r, g, b) = fill_color.as_pdf_rgb();
                    out.push_str("q\n");
                    out.push_str(&format!(
                        "{r:.4} {g:.4} {b:.4} rg\n{x:.2} {y:.2} {w:.2} {h:.2} re f\n"
                    ));
                    out.push_str("Q\n");
                }
                if let (Some(width), Some(stroke_color)) = (stroke_width, stroke) {
                    let (r, g, b) = stroke_color.as_pdf_rgb();
                    out.push_str("q\n");
                    out.push_str(&format!(
                        "{r:.4} {g:.4} {b:.4} RG\n{width:.2} w\n{x:.2} {y:.2} {w:.2} {h:.2} re S\n"
                    ));
                    out.push_str("Q\n");
                }
            }
        }
    }
    out
}

fn serialize_objects(objects: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let mut offsets: Vec<usize> = Vec::with_capacity(objects.len() + 1);
    offsets.push(0);

    for (idx, object_body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        let object_id = idx + 1;
        bytes.extend_from_slice(format!("{object_id} 0 obj\n").as_bytes());
        bytes.extend_from_slice(object_body);
        bytes.extend_from_slice(b"endobj\n");
    }

    let xref_offset = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    bytes.extend_from_slice(b"0000000000 65535 f \n");

    for offset in offsets.iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );

    Ok(bytes)
}

fn encode_pdf_hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn encode_pdf_text_bytes(input: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for ch in input.chars() {
        if let Some(byte) = unicode_to_winansi_byte(ch) {
            out.push(byte);
            continue;
        }
        match ch {
            '\t' => out.extend_from_slice(b"    "),
            '\u{00a0}' => out.push(b' '), // nbsp
            '☐' => out.extend_from_slice(b"[ ]"),
            '☑' | '☒' | '✅' => out.extend_from_slice(b"[x]"),
            _ => out.push(b'?'),
        }
    }
    out
}

fn unicode_to_winansi_byte(ch: char) -> Option<u8> {
    let code = ch as u32;
    if (0x20..=0x7e).contains(&code) {
        return Some(code as u8);
    }
    if (0xa0..=0xff).contains(&code) {
        return Some(code as u8);
    }
    match code {
        0x20ac => Some(0x80), // €
        0x201a => Some(0x82), // ‚
        0x0192 => Some(0x83), // ƒ
        0x201e => Some(0x84), // „
        0x2026 => Some(0x85), // …
        0x2020 => Some(0x86), // †
        0x2021 => Some(0x87), // ‡
        0x02c6 => Some(0x88), // ˆ
        0x2030 => Some(0x89), // ‰
        0x0160 => Some(0x8a), // Š
        0x2039 => Some(0x8b), // ‹
        0x0152 => Some(0x8c), // Œ
        0x017d => Some(0x8e), // Ž
        0x2018 => Some(0x91), // '
        0x2019 => Some(0x92), // '
        0x201c => Some(0x93), // "
        0x201d => Some(0x94), // "
        0x2022 => Some(0x95), // •
        0x2013 => Some(0x96), // –
        0x2014 => Some(0x97), // —
        0x02dc => Some(0x98), // ˜
        0x2122 => Some(0x99), // ™
        0x0161 => Some(0x9a), // š
        0x203a => Some(0x9b), // ›
        0x0153 => Some(0x9c), // œ
        0x017e => Some(0x9e), // ž
        0x0178 => Some(0x9f), // Ÿ
        _ => None,
    }
}

fn text_requires_unicode_font(text: &str) -> bool {
    text.chars()
        .any(|ch| ch != '\t' && unicode_to_winansi_byte(ch).is_none() && !ch.is_ascii_control())
}

fn encode_pdf_unicode_text_bytes(
    input: &str,
    font: &PdfUnicodeFontAsset,
    unicode_cmap: &mut FxHashMap<u16, char>,
) -> Vec<u8> {
    let Ok(face) = Face::parse(&font.bytes, 0) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(input.chars().count() * 2);
    for ch in input.chars() {
        if ch == '\t' {
            for _ in 0..PDF_TAB_WIDTH {
                let gid = face
                    .glyph_index(' ')
                    .map(|id| id.0)
                    .or_else(|| face.glyph_index('?').map(|id| id.0))
                    .unwrap_or(font.fallback_gid);
                out.push((gid >> 8) as u8);
                out.push((gid & 0xff) as u8);
                unicode_cmap.entry(gid).or_insert(' ');
            }
            continue;
        }
        if ch.is_ascii_control() && ch != '\t' && ch != ' ' {
            continue;
        }
        let normalized = if ch == '\u{00a0}' { ' ' } else { ch };
        let gid = face
            .glyph_index(normalized)
            .map(|id| id.0)
            .or_else(|| face.glyph_index('?').map(|id| id.0))
            .unwrap_or(font.fallback_gid);
        out.push((gid >> 8) as u8);
        out.push((gid & 0xff) as u8);
        unicode_cmap.entry(gid).or_insert(normalized);
    }
    out
}

fn utf16be_hex_for_char(ch: char) -> String {
    let mut out = String::new();
    let code = ch as u32;
    if code <= 0xFFFF {
        out.push_str(&format!("{:04X}", code));
    } else {
        let scalar = code - 0x10000;
        let high = 0xD800 + ((scalar >> 10) as u16);
        let low = 0xDC00 + ((scalar & 0x3FF) as u16);
        out.push_str(&format!("{:04X}{:04X}", high, low));
    }
    out
}

fn build_to_unicode_cmap(unicode_cmap: &FxHashMap<u16, char>) -> Vec<u8> {
    let mut entries: Vec<(u16, char)> = unicode_cmap.iter().map(|(k, v)| (*k, *v)).collect();
    entries.sort_by_key(|(gid, _)| *gid);
    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n");
    out.push_str("12 dict begin\n");
    out.push_str("begincmap\n");
    out.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n");
    out.push_str("/CMapName /SlateUnicodeToUnicode def\n");
    out.push_str("/CMapType 2 def\n");
    out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    if entries.is_empty() {
        out.push_str("0 beginbfchar\nendbfchar\n");
    } else {
        for chunk in entries.chunks(100) {
            out.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, ch) in chunk {
                out.push_str(&format!("<{:04X}> <{}>\n", gid, utf16be_hex_for_char(*ch)));
            }
            out.push_str("endbfchar\n");
        }
    }
    out.push_str("endcmap\n");
    out.push_str("CMapName currentdict /CMap defineresource pop\n");
    out.push_str("end\nend\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::pdf_layout::*;
    use super::pdf_style::*;
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::{ColorType, ImageEncoder, Rgb, RgbImage};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    }

    fn tiny_png_bytes() -> Vec<u8> {
        let image = RgbImage::from_pixel(1, 1, Rgb([255, 0, 0]));
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(image.as_raw(), 1, 1, ColorType::Rgb8.into())
            .expect("png encode");
        out
    }

    #[test]
    fn markdown_pdf_contains_headings_table_and_text() {
        let source =
            "# Title\n\n| Name | Score |\n| --- | ---: |\n| Ana | 5 |\n\n- item one\n- item two";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("%PDF-1.4"));
        assert!(pdf_contains_text(&text, "Title"));
        assert!(pdf_contains_text(&text, "Name"));
        assert!(pdf_contains_text(&text, "Score"));
        assert!(pdf_contains_text(&text, "item one"));
        assert!(text.contains("xref"));
    }

    #[test]
    fn markdown_pdf_renders_unordered_list_markers_as_ascii_dash() {
        let source = "- one\n- two";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);

        assert!(pdf_contains_text(&text, "- "));
        assert!(pdf_contains_text(&text, "one"));
        assert!(pdf_contains_text(&text, "two"));
    }

    #[test]
    fn markdown_pdf_embeds_images_when_resolver_returns_bytes() {
        let source = "![tiny](./assets/tiny.png)";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |src| {
            if src == "./assets/tiny.png" {
                Some(tiny_png_bytes())
            } else {
                None
            }
        })
        .expect("pdf with image should build");

        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Subtype /Image"));
        assert!(text.contains("/Im1 Do"));
    }

    #[test]
    fn parse_image_only_line_extracts_alt_and_source() {
        let parsed = parse_image_only_line("![Alt Text](./assets/a.png)").expect("image line");
        assert_eq!(parsed.0, "Alt Text");
        assert_eq!(parsed.1, "./assets/a.png");
    }

    #[test]
    fn decode_data_url_image_bytes_rejects_non_base64() {
        let err = decode_data_url_image_bytes("data:image/png,abc").expect_err("expected error");
        assert!(err.contains("non-base64"));
    }

    #[test]
    fn resolve_export_path_expands_tilde_prefix() {
        let raw = format!("~/slate-export-{}.txt", unique_suffix());
        let resolved = resolve_export_path(&raw).expect("tilde path should resolve");
        let home = resolve_home_dir().expect("home should be set");
        assert!(resolved.starts_with(home));
        assert!(resolved.to_string_lossy().contains("slate-export-"));
    }

    #[test]
    fn export_to_file_supports_tilde_paths() {
        let relative = format!("slate-export-{}.txt", unique_suffix());
        let raw = format!("~/{}", relative);
        let home = resolve_home_dir().expect("home should be set");
        let full_path = home.join(relative);
        match export_to_file_blocking(&raw, "hello") {
            Ok(()) => {
                let read_back =
                    fs::read_to_string(&full_path).expect("exported file should be readable");
                assert_eq!(read_back, "hello");
                let _ = fs::remove_file(full_path);
            }
            Err(error) => {
                let lowered = error.to_ascii_lowercase();
                assert!(error.contains(full_path.to_string_lossy().as_ref()));
                assert!(
                    lowered.contains("read-only file system")
                        || lowered.contains("permission denied"),
                    "unexpected export error: {error}"
                );
            }
        }
    }

    #[test]
    fn resolve_export_path_accepts_file_uri_paths() {
        let target = std::env::temp_dir().join(format!("slate export {}.pdf", unique_suffix()));
        let uri = Url::from_file_path(&target)
            .expect("temp path should convert to file URI")
            .to_string();
        let resolved = resolve_export_path(&uri).expect("file URI should resolve");
        assert_eq!(resolved, target);
    }

    #[test]
    fn resolve_export_path_strips_wrapping_quotes() {
        let target = std::env::temp_dir().join(format!("slate-export-{}.pdf", unique_suffix()));
        let raw = format!("\"{}\"", target.display());
        let resolved = resolve_export_path(&raw).expect("quoted path should resolve");
        assert_eq!(resolved, target);
    }

    #[test]
    fn resolve_export_path_rejects_missing_parent_directory() {
        let missing_dir = std::env::temp_dir().join(format!("slate-no-dir-{}", unique_suffix()));
        let target = missing_dir.join("out.pdf");
        let err = resolve_export_path(target.to_string_lossy().as_ref())
            .expect_err("missing parent should fail");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn markdown_pdf_applies_inline_and_code_theme_styles_and_renders_checklist() {
        let source = "- [x] **Done** *soon*\n\ncount := 1\ncount + 2\n\n```rust\nlet x = 1;\n```";
        let palette = PdfExportPalette {
            fg: PdfRgbColor { r: 1, g: 2, b: 3 },
            fg_dim: PdfRgbColor { r: 4, g: 5, b: 6 },
            accent: PdfRgbColor { r: 7, g: 8, b: 9 },
            variable: PdfRgbColor { r: 255, g: 0, b: 0 },
            code_keyword: PdfRgbColor { r: 0, g: 255, b: 0 },
            code_string: PdfRgbColor { r: 0, g: 0, b: 255 },
            code_number: PdfRgbColor {
                r: 120,
                g: 80,
                b: 40,
            },
            code_comment: PdfRgbColor {
                r: 20,
                g: 30,
                b: 40,
            },
            code_function: PdfRgbColor {
                r: 80,
                g: 90,
                b: 100,
            },
            code_type: PdfRgbColor {
                r: 110,
                g: 120,
                b: 130,
            },
        };
        let bytes = build_markdown_pdf(source, &palette, |_| None).expect("pdf generation");
        let text = String::from_utf8_lossy(&bytes);

        assert!(pdf_contains_text(&text, "Done"));
        assert!(text.contains("/F2"));
        assert!(text.contains("/F4"));
        assert!(text.contains("1.0000 0.0000 0.0000 rg"));
        assert!(text.contains("0.0000 1.0000 0.0000 rg"));
        assert!(!pdf_contains_text(&text, "[x]"));
    }

    #[test]
    fn strikethrough_lines_are_split_across_whitespace_gaps() {
        let mut pages = vec![Page::default()];
        let style = TextStyle {
            mono: false,
            bold: false,
            italic: false,
            strikethrough: true,
            color: pdf_black(),
        };
        let line = vec![
            StyledChar { ch: 'a', style },
            StyledChar { ch: ' ', style },
            StyledChar { ch: 'b', style },
        ];
        render_styled_line(&mut pages, 10.0, 20.0, BODY_FONT_SIZE_PT, &line);
        let page = pages.last().expect("page");
        let strike_count = page
            .ops
            .iter()
            .filter(|op| matches!(op, DrawOp::Line { .. }))
            .count();
        assert_eq!(strike_count, 2);
    }

    #[test]
    fn styled_chars_from_inline_preserves_spacing_around_code_and_links() {
        let palette = PdfExportPalette::default();
        let variable_names: Vec<String> = Vec::new();
        let source = "Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab). Link handling polish for [text](url) display behavior.";
        let styled = styled_chars_from_inline(
            source,
            &variable_names,
            &palette,
            TextStyle::body(pdf_black()),
        );
        let flattened: String = styled.into_iter().map(|entry| entry.ch).collect();
        assert_eq!(
            flattened,
            "Add assignment-trailer evaluation support (val := a - b = 44 style reconciliation on tab). Link handling polish for text display behavior."
        );
    }

    #[test]
    fn styled_chars_from_inline_highlights_variables_case_insensitively() {
        let palette = PdfExportPalette {
            fg: PdfRgbColor {
                r: 30,
                g: 32,
                b: 36,
            },
            fg_dim: PdfRgbColor {
                r: 109,
                g: 102,
                b: 91,
            },
            accent: PdfRgbColor {
                r: 122,
                g: 90,
                b: 58,
            },
            variable: PdfRgbColor {
                r: 220,
                g: 80,
                b: 40,
            },
            code_keyword: PdfRgbColor {
                r: 96,
                g: 112,
                b: 181,
            },
            code_string: PdfRgbColor {
                r: 91,
                g: 158,
                b: 111,
            },
            code_number: PdfRgbColor {
                r: 214,
                g: 120,
                b: 67,
            },
            code_comment: PdfRgbColor {
                r: 126,
                g: 138,
                b: 149,
            },
            code_function: PdfRgbColor {
                r: 52,
                g: 122,
                b: 165,
            },
            code_type: PdfRgbColor {
                r: 134,
                g: 99,
                b: 202,
            },
        };
        let styled = styled_chars_from_inline(
            "MoN := 41",
            &["mon".to_string()],
            &palette,
            TextStyle::body(pdf_black()),
        );
        let var_chars = styled
            .iter()
            .filter(|entry| matches!(entry.ch, 'M' | 'o' | 'N'))
            .collect::<Vec<_>>();
        assert_eq!(var_chars.len(), 3);
        for entry in var_chars {
            assert!(entry.style.bold);
            assert_eq!(entry.style.color, palette.variable);
        }
    }

    #[test]
    fn markdown_pdf_does_not_collapse_code_space_before_following_text() {
        let source =
            "- [ ] Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab).";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("44style"),
            "inline code boundary collapsed required whitespace"
        );
    }

    #[test]
    fn encode_pdf_text_bytes_preserves_common_unicode_punctuation() {
        let encoded = encode_pdf_text_bytes("• \u{201c}quote\u{201d} \u{2013} \u{2026}");
        assert_eq!(
            encoded,
            vec![0x95, 0x20, 0x93, 0x71, 0x75, 0x6f, 0x74, 0x65, 0x94, 0x20, 0x96, 0x20, 0x85]
        );
    }

    #[test]
    fn encode_pdf_text_bytes_keeps_checkbox_fallbacks() {
        let encoded = encode_pdf_text_bytes("☐ ☑");
        assert_eq!(encoded, b"[ ] [x]");
    }

    #[test]
    fn styled_chars_for_code_line_expands_tabs() {
        let chars =
            styled_chars_for_code_line("\tif x {\t}", Some("go"), &PdfExportPalette::default());
        let text: String = chars.into_iter().map(|entry| entry.ch).collect();
        assert_eq!(text, "    if x {  }");
    }

    #[test]
    fn pdf_uses_hex_encoded_text_runs() {
        let source = "Title";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("<5469746C65> Tj"));
    }

    #[test]
    fn text_requires_unicode_font_flags_non_winansi() {
        assert!(!text_requires_unicode_font("hello"));
        assert!(text_requires_unicode_font("\u{0427}\u{0435}\u{0442}"));
        assert!(text_requires_unicode_font("\u{4F60}\u{597D}"));
    }

    #[test]
    fn markdown_pdf_uses_unicode_font_for_non_winansi_when_available() {
        if resolve_unicode_pdf_font_asset().is_none() {
            return;
        }
        let source = "Unicode: \u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{4F60}\u{597D}";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Subtype /Type0"));
        assert!(text.contains("/Subtype /CIDFontType2"));
        assert!(text.contains("/F6"));
    }

    #[test]
    fn preserve_hard_linebreaks_for_assignment_dense_paragraphs() {
        let dense = vec![
            "a := 1".to_string(),
            "b := a + 2".to_string(),
            "c := b + 3".to_string(),
        ];
        assert!(should_preserve_hard_linebreaks(&dense));

        let prose = vec![
            "This is a paragraph.".to_string(),
            "It should still wrap naturally.".to_string(),
        ];
        assert!(!should_preserve_hard_linebreaks(&prose));
    }

    #[test]
    fn markdown_pdf_keeps_assignment_lines_separate() {
        let source = "a := 1\nb := 2\nc := 3";
        let pages = render_markdown_to_pages(
            source,
            &PdfExportPalette::default(),
            &FxHashMap::default(),
            &[],
        );

        let mut y_by_var = FxHashMap::<String, f32>::default();
        for page in pages {
            for op in page.ops {
                if let DrawOp::Text { y, text, .. } = op {
                    let token = text.trim();
                    if matches!(token, "a" | "b" | "c") {
                        y_by_var.insert(token.to_string(), y);
                    }
                }
            }
        }

        let a_y = y_by_var.get("a").expect("a should render");
        let b_y = y_by_var.get("b").expect("b should render");
        let c_y = y_by_var.get("c").expect("c should render");
        assert!(a_y > b_y, "b should render on a lower line than a");
        assert!(b_y > c_y, "c should render on a lower line than b");
    }

    #[test]
    fn markdown_pdf_code_blocks_have_background_and_border_rect() {
        let source = "```go\nx := 1\n```";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(" re f"), "code block fill rect missing");
        assert!(text.contains(" re S"), "code block border rect missing");
    }

    #[test]
    fn markdown_pdf_table_uses_evaluated_formula_values() {
        let source = "| v |\n| --- |\n| 7 |\n| 11 |\n| :=sum_col() |";
        let lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
        let values = collect_table_formula_display_values(&lines);
        assert!(
            values.values().any(|value| value.starts_with("18")),
            "expected at least one evaluated table formula value starting with 18, got {values:?}"
        );

        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!pdf_contains_text(&text, "sum_col()"));
    }

    #[test]
    fn markdown_pdf_table_does_not_double_stroke_shared_horizontal_borders() {
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |";
        let pages = render_markdown_to_pages(
            source,
            &PdfExportPalette::default(),
            &FxHashMap::default(),
            &[],
        );
        let page = pages.first().expect("first page");
        let table_left = PDF_MARGIN_LEFT_PT;
        let table_right = PDF_MARGIN_LEFT_PT + content_width();
        let mut horizontal_table_lines = 0usize;

        for op in &page.ops {
            if let DrawOp::Line {
                width,
                x1,
                y1,
                x2,
                y2,
                ..
            } = op
            {
                let is_horizontal = (y1 - y2).abs() < 0.01;
                let spans_full_table =
                    (x1 - table_left).abs() < 0.01 && (x2 - table_right).abs() < 0.01;
                let is_table_border_width = (*width - TABLE_BORDER_WIDTH_PT).abs() < 0.01;
                if is_horizontal && spans_full_table && is_table_border_width {
                    horizontal_table_lines += 1;
                }
            }
        }

        // header+2 data rows => 3 row bands, which should produce exactly
        // 4 horizontal borders (top + 3 bottoms), not 6 with doubled middle lines.
        assert_eq!(horizontal_table_lines, 4);
    }

    fn pdf_contains_text(pdf_text: &str, text: &str) -> bool {
        let encoded = encode_pdf_text_bytes(text);
        if encoded.is_empty() {
            return false;
        }
        let marker = format!("<{}> Tj", encode_pdf_hex_string(&encoded));
        pdf_text.contains(&marker)
    }
}
