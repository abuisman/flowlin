//! Which files count as images. Detection is by extension first; files
//! without an extension are sniffed by magic bytes.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// Extensions the `image` crate fallback decodes on its own.
const IMAGE_CRATE_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "tif", "tiff", "ico", "pnm", "pbm", "pgm",
    "ppm", "pam", "tga", "qoi",
];

/// Everything we are willing to show if some loader handles it.
const KNOWN_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "tif", "tiff", "svg", "svgz", "ico", "cur",
    "pnm", "pbm", "pgm", "ppm", "pam", "tga", "qoi", "jxl", "avif", "heif", "heic",
    // RAW formats, only via a system pixbuf loader that extracts the embedded preview.
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "raf", "pef", "srw",
];

static SUPPORTED: OnceLock<HashSet<String>> = OnceLock::new();

/// Detect available loaders. Call once at start-up (any thread).
pub fn init() -> &'static HashSet<String> {
    SUPPORTED.get_or_init(|| {
        let mut pixbuf_exts: HashSet<String> = HashSet::new();
        for f in gtk::gdk_pixbuf::Pixbuf::formats() {
            if f.is_disabled() {
                continue;
            }
            for e in f.extensions() {
                pixbuf_exts.insert(e.to_ascii_lowercase());
            }
        }
        let mut set = HashSet::new();
        for e in KNOWN_EXTS {
            if IMAGE_CRATE_EXTS.contains(e) || pixbuf_exts.contains(*e) {
                set.insert((*e).to_string());
            }
        }
        // `heif` loaders often only register "heic"/"avif"; accept both spellings.
        if set.contains("heic") {
            set.insert("heif".into());
        }
        let mut optional: Vec<_> = ["jxl", "avif", "heic", "svg", "cr2", "nef", "dng"]
            .iter()
            .map(|e| format!("{e}={}", if set.contains(*e) { "yes" } else { "no" }))
            .collect();
        optional.sort();
        tracing::info!("image loaders: {}", optional.join(" "));
        set
    })
}

fn supported() -> &'static HashSet<String> {
    SUPPORTED.get().unwrap_or_else(|| init())
}

/// Lower-cased extension of `path`, if any.
pub fn extension(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
}

/// Is this a file we display? `sniff` allows reading magic bytes for files
/// without an extension.
pub fn is_image(path: &Path, sniff: bool) -> bool {
    match extension(path) {
        Some(ext) => supported().contains(&ext),
        None if sniff => sniff_is_image(path),
        None => false,
    }
}

pub fn sniff_is_image(path: &Path) -> bool {
    matches!(infer::get_from_path(path), Ok(Some(t)) if t.matcher_type() == infer::MatcherType::Image)
}

/// Human-readable type label ("JPEG", "PNG", …).
pub fn type_label(path: &Path) -> String {
    match extension(path).as_deref() {
        Some("jpg" | "jpeg" | "jpe" | "jfif") => "JPEG".into(),
        Some("tif" | "tiff") => "TIFF".into(),
        Some("svg" | "svgz") => "SVG".into(),
        Some("heic" | "heif") => "HEIF".into(),
        Some(e) => e.to_ascii_uppercase(),
        None => match infer::get_from_path(path) {
            Ok(Some(t)) => t.extension().to_ascii_uppercase(),
            _ => "?".into(),
        },
    }
}

/// Formats that may contain several frames.
pub fn maybe_animated(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("gif" | "webp" | "png" | "apng"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_detection() {
        assert!(is_image(Path::new("/x/a.JPG"), false));
        assert!(is_image(Path::new("/x/b.png"), false));
        assert!(!is_image(Path::new("/x/c.txt"), false));
        assert!(!is_image(Path::new("/x/noext"), false));
    }

    #[test]
    fn sniffing_extensionless_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("picture");
        image::RgbImage::new(4, 4).save_with_format(&p, image::ImageFormat::Png).unwrap();
        assert!(is_image(&p, true));
        let t = dir.path().join("notes");
        std::fs::write(&t, b"hello world").unwrap();
        assert!(!is_image(&t, true));
    }
}
