//! Tags and ratings stored as extended attributes so they travel with the
//! file: `user.xdg.tags` (comma separated) and `user.baloo.rating` (0–10).

use std::path::Path;

pub const TAGS_ATTR: &str = "user.xdg.tags";
pub const RATING_ATTR: &str = "user.baloo.rating";

pub fn parse_tags(raw: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(raw).split(',').map(|t| t.trim()).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

pub fn format_tags(tags: &[String]) -> String {
    tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(",")
}

/// Parse a baloo rating (0–10) into stars (0–5).
pub fn parse_rating(raw: &[u8]) -> Option<u8> {
    let v: u32 = String::from_utf8_lossy(raw).trim().parse().ok()?;
    Some((v.min(10) as u8).div_ceil(2))
}

pub fn read_rating(path: &Path) -> u8 {
    xattr::get(path, RATING_ATTR).ok().flatten().and_then(|v| parse_rating(&v)).unwrap_or(0)
}

/// Write a star rating (0 removes the attribute).
pub fn write_rating(path: &Path, stars: u8) -> std::io::Result<()> {
    if stars == 0 {
        match xattr::remove(path, RATING_ATTR) {
            Err(e) if e.raw_os_error() == Some(libc_enodata()) => Ok(()),
            r => r,
        }
    } else {
        xattr::set(path, RATING_ATTR, (stars.min(5) * 2).to_string().as_bytes())
    }
}

pub fn read_tags(path: &Path) -> Vec<String> {
    xattr::get(path, TAGS_ATTR).ok().flatten().map(|v| parse_tags(&v)).unwrap_or_default()
}

pub fn supported() -> bool {
    xattr::SUPPORTED_PLATFORM
}

const fn libc_enodata() -> i32 {
    61 // ENODATA on Linux
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_roundtrip() {
        assert_eq!(parse_tags(b"holiday, beach ,,2024"), ["holiday", "beach", "2024"]);
        assert_eq!(format_tags(&["a".into(), " b ".into(), "".into()]), "a,b");
        assert!(parse_tags(b"").is_empty());
    }

    #[test]
    fn ratings() {
        assert_eq!(parse_rating(b"10"), Some(5));
        assert_eq!(parse_rating(b"7"), Some(4));
        assert_eq!(parse_rating(b"0"), Some(0));
        assert_eq!(parse_rating(b"99"), Some(5));
        assert_eq!(parse_rating(b"x"), None);
    }

    #[test]
    fn rating_on_disk_if_supported() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.jpg");
        std::fs::write(&p, b"x").unwrap();
        if write_rating(&p, 3).is_err() {
            return; // tmpfs without user xattrs
        }
        assert_eq!(read_rating(&p), 3);
        write_rating(&p, 0).unwrap();
        assert_eq!(read_rating(&p), 0);
        write_rating(&p, 0).unwrap();
    }
}
