//! Image decoding. Runs on worker threads and produces [`RawImage`] buffers;
//! GDK textures are created from them on the main thread (GDK objects may
//! only be constructed there).
//!
//! Formats the `image` crate handles (JPEG, PNG, WebP, GIF, TIFF, BMP, …)
//! are decoded in-process, JPEG thumbnails with DCT scaling. Everything else
//! (SVG, HEIF, AVIF, JPEG XL, RAW previews) goes through gdk-pixbuf, which
//! also serves as the fallback. gdk-pixbuf ≥ 2.44 delegates to glycin's
//! sandboxed loaders, which costs a process round-trip per image — far too
//! slow to be the first choice for thumbnails of common formats.

pub mod exif;

use std::io::BufReader;
use std::path::Path;

use gtk::gdk;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use gtk::prelude::*;
use image::{AnimationDecoder, DynamicImage, ImageDecoder};

use crate::fs::formats;

/// Sources larger than this are pre-scaled on load to bound memory use.
const MAX_FULL_PIXELS: u64 = 120_000_000;
const MAX_FULL_SIDE: i32 = 16384;
/// Hard cap on decoded animation memory.
const MAX_ANIMATION_BYTES: usize = 384 * 1024 * 1024;

/// Decoded pixels, straight (non-premultiplied) RGB(A) 8-bit.
#[derive(Clone)]
pub struct RawImage {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub has_alpha: bool,
    pub bytes: glib::Bytes,
}

impl std::fmt::Debug for RawImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RawImage({}x{})", self.width, self.height)
    }
}

impl RawImage {
    fn from_pixbuf(p: &Pixbuf) -> RawImage {
        RawImage {
            width: p.width(),
            height: p.height(),
            stride: p.rowstride() as usize,
            has_alpha: p.has_alpha(),
            bytes: p.read_pixel_bytes(),
        }
    }

    fn from_dynamic(img: DynamicImage) -> RawImage {
        if img.color().has_alpha() {
            Self::from_rgba(img.into_rgba8())
        } else {
            let rgb = img.into_rgb8();
            RawImage {
                width: rgb.width() as i32,
                height: rgb.height() as i32,
                stride: rgb.width() as usize * 3,
                has_alpha: false,
                bytes: glib::Bytes::from_owned(rgb.into_raw()),
            }
        }
    }

    fn from_rgba(img: image::RgbaImage) -> RawImage {
        RawImage {
            width: img.width() as i32,
            height: img.height() as i32,
            stride: img.width() as usize * 4,
            has_alpha: true,
            bytes: glib::Bytes::from_owned(img.into_raw()),
        }
    }

    pub fn byte_size(&self) -> usize {
        self.bytes.len()
    }

    /// Wrap the pixels in a texture without copying. Main thread only.
    pub fn texture(&self) -> gdk::Texture {
        let format = if self.has_alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}

pub struct Thumb {
    pub image: RawImage,
    /// Dimensions of the source after EXIF orientation.
    pub width: u32,
    pub height: u32,
}

pub enum Full {
    Static { image: RawImage, width: u32, height: u32 },
    Animated { frames: Vec<(RawImage, u32)>, width: u32, height: u32 },
}

impl Full {
    pub fn byte_size(&self) -> usize {
        match self {
            Full::Static { image, .. } => image.byte_size(),
            Full::Animated { frames, .. } => frames.iter().map(|(f, _)| f.byte_size()).sum(),
        }
    }
}

fn pixbuf_orientation_swaps(p: &Pixbuf) -> bool {
    matches!(p.option("orientation").as_deref(), Some("5" | "6" | "7" | "8"))
}

fn oriented(p: Pixbuf) -> Pixbuf {
    p.apply_embedded_orientation().unwrap_or(p)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    /// Decoded by the `image` crate.
    Native,
    /// Needs gdk-pixbuf (glycin) loaders.
    Pixbuf,
}

/// Decide by content (magic bytes), falling back to the extension.
fn kind(path: &Path) -> Kind {
    let sniffed = infer::get_from_path(path).ok().flatten().map(|t| t.mime_type());
    match sniffed {
        Some("image/jpeg") => Kind::Jpeg,
        Some(
            "image/png"
            | "image/apng"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/tiff"
            | "image/vnd.microsoft.icon"
            | "image/x-icon"
            | "image/qoi",
        ) => Kind::Native,
        Some(_) => Kind::Pixbuf,
        None => match formats::extension(path).as_deref() {
            Some("tga" | "pnm" | "pbm" | "pgm" | "ppm" | "pam" | "qoi") => Kind::Native,
            _ => Kind::Pixbuf,
        },
    }
}

/// A thumbnail that fits in `size`×`size`, EXIF orientation applied.
pub fn thumbnail(path: &Path, size: i32) -> Result<Thumb, String> {
    let first = match kind(path) {
        Kind::Jpeg => thumbnail_jpeg(path, size as u32),
        Kind::Native => thumbnail_image_crate(path, size as u32),
        Kind::Pixbuf => thumbnail_pixbuf(path, size),
    };
    first.or_else(|e| {
        thumbnail_pixbuf(path, size)
            .or_else(|_| thumbnail_image_crate(path, size as u32))
            .map_err(|e2| format!("{e}; fallback: {e2}"))
    })
}

/// JPEG thumbnail using DCT scaling (decodes at 1/2, 1/4 or 1/8 size).
fn thumbnail_jpeg(path: &Path, size: u32) -> Result<Thumb, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    dec.read_info().map_err(|e| e.to_string())?;
    let info = dec.info().ok_or("no jpeg info")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let req = |v: u32| v.min(u16::MAX as u32) as u16;
    // Ask for at least `size` on the shorter side of the box.
    let (rw, rh) = if w >= h { (req(size * w / h.max(1)), req(size)) } else { (req(size), req(size * h / w.max(1))) };
    dec.scale(rw.max(1), rh.max(1)).map_err(|e| e.to_string())?;
    let pixels = dec.decode().map_err(|e| e.to_string())?;
    let info = dec.info().ok_or("no jpeg info")?;
    let (sw, sh) = (info.width as u32, info.height as u32);
    let img = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            DynamicImage::ImageRgb8(image::RgbImage::from_raw(sw, sh, pixels).ok_or("bad buffer")?)
        }
        jpeg_decoder::PixelFormat::L8 => {
            DynamicImage::ImageLuma8(image::GrayImage::from_raw(sw, sh, pixels).ok_or("bad buffer")?)
        }
        other => return Err(format!("unsupported jpeg pixel format {other:?}")),
    };
    let img = if sw > size || sh > size { img.thumbnail(size, size) } else { img };
    let orientation = exif::orientation(path).unwrap_or(1);
    let img = apply_exif_orientation(img, orientation);
    let (ow, oh) = if orientation >= 5 { (h, w) } else { (w, h) };
    Ok(Thumb { image: RawImage::from_dynamic(img), width: ow, height: oh })
}

fn apply_exif_orientation(img: DynamicImage, o: u32) -> DynamicImage {
    match o {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn thumbnail_pixbuf(path: &Path, size: i32) -> Result<Thumb, String> {
    let (_, w, h) = Pixbuf::file_info(path).ok_or("unknown format")?;
    let pb = if w > 0 && h > 0 && w <= size && h <= size {
        Pixbuf::from_file(path)
    } else {
        Pixbuf::from_file_at_scale(path, size, size, true)
    }
    .map_err(|e| e.to_string())?;
    let (mut ow, mut oh) = if w > 0 && h > 0 { (w as u32, h as u32) } else { (pb.width() as u32, pb.height() as u32) };
    if pixbuf_orientation_swaps(&pb) {
        std::mem::swap(&mut ow, &mut oh);
    }
    let pb = oriented(pb);
    Ok(Thumb { image: RawImage::from_pixbuf(&pb), width: ow, height: oh })
}

fn open_decoder(path: &Path) -> Result<impl ImageDecoder, String> {
    image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())
}

fn decode_image_crate(path: &Path) -> Result<DynamicImage, String> {
    let mut dec = open_decoder(path)?;
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn thumbnail_image_crate(path: &Path, size: u32) -> Result<Thumb, String> {
    let img = decode_image_crate(path)?;
    let (w, h) = (img.width(), img.height());
    let t = if w <= size && h <= size { img } else { img.thumbnail(size, size) };
    Ok(Thumb { image: RawImage::from_dynamic(t), width: w, height: h })
}

/// Just the (oriented) dimensions, reading as little as possible.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if kind(path) != Kind::Pixbuf {
        let r = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
        if let Ok((w, h)) = r.into_dimensions() {
            let swap = exif::orientation(path).map(|o| o >= 5).unwrap_or(false);
            return Some(if swap { (h, w) } else { (w, h) });
        }
    }
    if let Some((_, w, h)) = Pixbuf::file_info(path) {
        if w > 0 && h > 0 {
            let swap = exif::orientation(path).map(|o| o >= 5).unwrap_or(false);
            return Some(if swap { (h as u32, w as u32) } else { (w as u32, h as u32) });
        }
    }
    let dec = open_decoder(path).ok()?;
    Some(dec.dimensions())
}

/// Decode an image at full size (animations as all frames).
pub fn load_full(path: &Path) -> Result<Full, String> {
    if formats::maybe_animated(path) {
        match load_animation(path) {
            Ok(Some(full)) => return Ok(full),
            Ok(None) => {}
            Err(e) => tracing::debug!("animation decode of {} failed: {e}", path.display()),
        }
    }
    if kind(path) == Kind::Pixbuf {
        return load_static_pixbuf(path)
            .or_else(|e| load_static_image_crate(path).map_err(|e2| format!("{e}; fallback: {e2}")));
    }
    load_static_image_crate(path).or_else(|e| load_static_pixbuf(path).map_err(|e2| format!("{e}; fallback: {e2}")))
}

fn load_static_image_crate(path: &Path) -> Result<Full, String> {
    let img = decode_image_crate(path)?;
    let (w, h) = (img.width(), img.height());
    let img = if (w as u64) * (h as u64) > MAX_FULL_PIXELS {
        img.resize(MAX_FULL_SIDE as u32 / 2, MAX_FULL_SIDE as u32 / 2, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(Full::Static { image: RawImage::from_dynamic(img), width: w, height: h })
}

fn load_static_pixbuf(path: &Path) -> Result<Full, String> {
    let (_, w, h) = Pixbuf::file_info(path).ok_or("unknown format")?;
    let too_big = (w as u64) * (h as u64) > MAX_FULL_PIXELS || w > MAX_FULL_SIDE || h > MAX_FULL_SIDE;
    let pb = if too_big {
        let side = if (w as u64) * (h as u64) > MAX_FULL_PIXELS { MAX_FULL_SIDE / 2 } else { MAX_FULL_SIDE };
        Pixbuf::from_file_at_scale(path, side, side, true)
    } else {
        Pixbuf::from_file(path)
    }
    .map_err(|e| e.to_string())?;
    let (mut ow, mut oh) = if w > 0 && h > 0 { (w as u32, h as u32) } else { (pb.width() as u32, pb.height() as u32) };
    if pixbuf_orientation_swaps(&pb) {
        std::mem::swap(&mut ow, &mut oh);
    }
    let pb = oriented(pb);
    Ok(Full::Static { image: RawImage::from_pixbuf(&pb), width: ow, height: oh })
}

fn collect_frames<'a>(frames: image::Frames<'a>) -> Result<Option<Full>, String> {
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for f in frames {
        let f = f.map_err(|e| e.to_string())?;
        let (num, den) = f.delay().numer_denom_ms();
        let delay = num.checked_div(den).unwrap_or(100);
        // Browsers treat very short delays as 100 ms; so do we.
        let delay = if delay <= 10 { 100 } else { delay };
        let buf = f.into_buffer();
        bytes += buf.len();
        out.push((RawImage::from_rgba(buf), delay));
        if bytes > MAX_ANIMATION_BYTES {
            break;
        }
    }
    if out.len() < 2 {
        return Ok(None);
    }
    let (w, h) = (out[0].0.width as u32, out[0].0.height as u32);
    Ok(Some(Full::Animated { frames: out, width: w, height: h }))
}

fn load_animation(path: &Path) -> Result<Option<Full>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let r = BufReader::new(file);
    let kind = infer::get_from_path(path).ok().flatten().map(|t| t.mime_type());
    match kind {
        Some("image/gif") => {
            let d = image::codecs::gif::GifDecoder::new(r).map_err(|e| e.to_string())?;
            collect_frames(d.into_frames())
        }
        Some("image/webp") => {
            let d = image::codecs::webp::WebPDecoder::new(r).map_err(|e| e.to_string())?;
            if !d.has_animation() {
                return Ok(None);
            }
            collect_frames(d.into_frames())
        }
        Some("image/png") | Some("image/apng") => {
            let d = image::codecs::png::PngDecoder::new(r).map_err(|e| e.to_string())?;
            if !d.is_apng().map_err(|e| e.to_string())? {
                return Ok(None);
            }
            collect_frames(d.apng().map_err(|e| e.to_string())?.into_frames())
        }
        _ => Ok(None),
    }
}

/// Colour type / bit depth description for the properties dialog.
pub fn color_description(path: &Path) -> Option<String> {
    let dec = open_decoder(path).ok()?;
    Some(format!("{:?}", dec.color_type()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_files_error_instead_of_panicking() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("broken.jpg");
        std::fs::write(&p, b"\xff\xd8\xff\xe0garbage").unwrap();
        assert!(thumbnail(&p, 128).is_err());
        assert!(load_full(&p).is_err());
    }

    #[test]
    fn wrong_extension_still_decodes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("actually_png.jpg");
        image::RgbImage::new(300, 100).save_with_format(&p, image::ImageFormat::Png).unwrap();
        let t = thumbnail(&p, 128).unwrap();
        assert_eq!((t.width, t.height), (300, 100));
        assert!(t.image.width <= 128);
    }

    #[test]
    fn animated_gif_has_frames() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, RgbaImage};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("anim.gif");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut enc = GifEncoder::new(f);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..3u8 {
                let img = RgbaImage::from_pixel(8, 8, image::Rgba([i * 80, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
            }
        }
        match load_full(&p).unwrap() {
            Full::Animated { frames, width, height } => {
                assert_eq!(frames.len(), 3);
                assert_eq!((width, height), (8, 8));
            }
            _ => panic!("expected animation"),
        }
    }
}
