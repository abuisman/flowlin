//! EXIF summary for the properties dialog.

use std::path::Path;

fn read(path: &Path) -> Option<exif::Exif> {
    let file = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(file);
    exif::Reader::new().read_from_container(&mut r).ok()
}

/// EXIF orientation tag value (1–8), if present.
pub fn orientation(path: &Path) -> Option<u32> {
    let e = read(path)?;
    e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?.value.get_uint(0)
}

/// (label, value) pairs: camera, lens, exposure, GPS presence, date.
pub fn summary(path: &Path) -> Vec<(&'static str, String)> {
    let Some(e) = read(path) else { return Vec::new() };
    let get = |t: exif::Tag| {
        e.get_field(t, exif::In::PRIMARY)
            .map(|f| f.display_value().with_unit(&e).to_string().trim_matches('"').trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mut out = Vec::new();
    let camera = [get(exif::Tag::Make), get(exif::Tag::Model)].into_iter().flatten().collect::<Vec<_>>().join(" ");
    if !camera.is_empty() {
        out.push(("Camera", camera));
    }
    if let Some(l) = get(exif::Tag::LensModel) {
        out.push(("Lens", l));
    }
    let exposure = [
        get(exif::Tag::ExposureTime),
        get(exif::Tag::FNumber),
        get(exif::Tag::PhotographicSensitivity).map(|s| format!("ISO {s}")),
        get(exif::Tag::FocalLength),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    if !exposure.is_empty() {
        out.push(("Exposure", exposure));
    }
    if let Some(d) = get(exif::Tag::DateTimeOriginal) {
        out.push(("Taken", d));
    }
    let gps = e.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY).is_some();
    out.push(("GPS location", if gps { "Yes".into() } else { "No".into() }));
    out
}
