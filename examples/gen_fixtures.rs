//! Generate a fixture tree for tests and manual checks:
//! `cargo run --example gen_fixtures -- <dir> [per_format]`
//!
//! Creates images in every format the `image` crate can write, nested three
//! levels deep, plus an animated GIF, a corrupt JPEG and a PNG with a `.jpg`
//! extension.

use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgb, RgbImage};

fn picture(w: u32, h: u32, seed: u32) -> RgbImage {
    let (a, b) = ((seed * 37) % 255, (seed * 91) % 255);
    RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let ring = (((fx - 0.5).powi(2) + (fy - 0.5).powi(2)).sqrt() * 12.0).sin() * 0.5 + 0.5;
        Rgb([(fx * 255.0) as u8 ^ a as u8, (ring * 255.0) as u8, (fy * 255.0) as u8 ^ b as u8])
    })
}

const SIZES: [(u32, u32); 6] = [(640, 400), (400, 640), (800, 800), (1200, 500), (300, 900), (1024, 768)];

pub fn generate(root: &Path, per_format: u32) -> usize {
    let formats = [
        (ImageFormat::Jpeg, "jpg"),
        (ImageFormat::Png, "png"),
        (ImageFormat::WebP, "webp"),
        (ImageFormat::Bmp, "bmp"),
        (ImageFormat::Tiff, "tiff"),
        (ImageFormat::Gif, "gif"),
        (ImageFormat::Qoi, "qoi"),
        (ImageFormat::Tga, "tga"),
        (ImageFormat::Pnm, "ppm"),
    ];
    let dirs: Vec<PathBuf> =
        vec![root.to_path_buf(), root.join("level1"), root.join("level1/level2"), root.join("level1/level2/level3")];
    for d in &dirs {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut n = 0;
    for (fi, (fmt, ext)) in formats.iter().enumerate() {
        for i in 0..per_format {
            let seed = fi as u32 * 1000 + i;
            let (w, h) = SIZES[(seed % SIZES.len() as u32) as usize];
            let dir = &dirs[(i % dirs.len() as u32) as usize];
            let p = dir.join(format!("image{i}_{ext}.{ext}"));
            picture(w / 2, h / 2, seed).save_with_format(&p, *fmt).unwrap();
            n += 1;
        }
    }
    // Animated GIF.
    {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, RgbaImage};
        let f = std::fs::File::create(root.join("animated.gif")).unwrap();
        let mut enc = GifEncoder::new(f);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for k in 0..8u8 {
            let img =
                RgbaImage::from_fn(120, 80, |x, _| image::Rgba([((x as u8).wrapping_add(k * 30)), k * 30, 200, 255]));
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(120, 1))).unwrap();
        }
        n += 1;
    }
    std::fs::write(root.join("corrupt.jpg"), b"\xff\xd8\xff\xe0 this is not a jpeg").unwrap();
    picture(200, 120, 7).save_with_format(root.join("really_a_png.jpg"), ImageFormat::Png).unwrap();
    std::fs::write(root.join("notes.txt"), b"not an image").unwrap();
    n + 2
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: gen_fixtures <dir> [per_format]");
    let per: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(50);
    let n = generate(Path::new(&dir), per);
    println!("generated {n} image files in {dir}");
}
