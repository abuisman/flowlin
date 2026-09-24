//! Small pure helpers: natural ordering, breadcrumb splitting, formatting.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Natural, case-insensitive comparison: `image2.jpg` < `image10.jpg`.
/// Ties are broken case-sensitively so the order is total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    match natural_cmp_folded(a, b) {
        Ordering::Equal => a.cmp(b),
        o => o,
    }
}

fn natural_cmp_folded(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let da = take_digits(&mut ai);
                    let db = take_digits(&mut bi);
                    let ta = da.trim_start_matches('0');
                    let tb = db.trim_start_matches('0');
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| da.len().cmp(&db.len()));
                    if o != Ordering::Equal {
                        return o;
                    }
                } else {
                    let la = fold(ca);
                    let lb = fold(cb);
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            s.push(c);
            it.next();
        } else {
            break;
        }
    }
    s
}

/// One clickable breadcrumb segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub path: PathBuf,
}

/// Split `path` into breadcrumb segments. Paths inside `home` start with a
/// "Home" segment; everything else starts at the filesystem root.
pub fn breadcrumbs(path: &Path, home: &Path, home_label: &str, root_label: &str) -> Vec<Crumb> {
    let mut out = Vec::new();
    let (base, rest) = match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() && home != Path::new("/") => {
            out.push(Crumb { label: home_label.to_string(), path: home.to_path_buf() });
            (home.to_path_buf(), rest.to_path_buf())
        }
        _ => {
            out.push(Crumb { label: root_label.to_string(), path: PathBuf::from("/") });
            let rest = path.strip_prefix("/").unwrap_or(path).to_path_buf();
            (PathBuf::from("/"), rest)
        }
    };
    let mut cur = base;
    for comp in rest.components() {
        cur.push(comp);
        out.push(Crumb { label: comp.as_os_str().to_string_lossy().into_owned(), path: cur.clone() });
    }
    out
}

/// "4 382" — groups of three separated by a thin no-break space.
pub fn format_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// Human readable byte size (decimal units, like GNOME).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut unit = "";
    for u in UNITS {
        v /= 1000.0;
        unit = u;
        if v < 1000.0 {
            break;
        }
    }
    format!("{v:.1} {unit}")
}

/// File name of a path as a displayable string.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_numbers() {
        let mut v = vec!["image10.jpg", "image2.jpg", "Image1.jpg", "image02.jpg", "a.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a.jpg", "Image1.jpg", "image2.jpg", "image02.jpg", "image10.jpg"]);
    }

    #[test]
    fn natural_case_insensitive_with_stable_tiebreak() {
        assert_eq!(natural_cmp("abc", "ABD"), Ordering::Less);
        assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
        assert_ne!(natural_cmp("a", "A"), Ordering::Equal);
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
    }

    #[test]
    fn natural_big_numbers_and_prefixes() {
        assert_eq!(natural_cmp("f99999999999999999999", "f100000000000000000000"), Ordering::Less);
        assert_eq!(natural_cmp("photo", "photo1"), Ordering::Less);
        assert_eq!(natural_cmp("été", "Zulu"), Ordering::Greater); // non-ascii after ascii
    }

    #[test]
    fn crumbs_inside_home() {
        let c = breadcrumbs(Path::new("/home/u/Pictures/2024"), Path::new("/home/u"), "Home", "Computer");
        let labels: Vec<_> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Home", "Pictures", "2024"]);
        assert_eq!(c[2].path, PathBuf::from("/home/u/Pictures/2024"));
    }

    #[test]
    fn crumbs_outside_home() {
        let c = breadcrumbs(Path::new("/usr/share"), Path::new("/home/u"), "Home", "Computer");
        let labels: Vec<_> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Computer", "usr", "share"]);
        assert_eq!(c[0].path, PathBuf::from("/"));
        let root = breadcrumbs(Path::new("/"), Path::new("/home/u"), "Home", "Computer");
        assert_eq!(root.len(), 1);
    }

    #[test]
    fn home_prefix_is_component_wise() {
        let c = breadcrumbs(Path::new("/home/user2/x"), Path::new("/home/user"), "Home", "Computer");
        assert_eq!(c[0].label, "Computer");
    }

    #[test]
    fn counts_and_sizes() {
        assert_eq!(format_count(4382), "4\u{202f}382");
        assert_eq!(format_count(200000), "200\u{202f}000");
        assert_eq!(format_count(12), "12");
        assert_eq!(format_size(999), "999 bytes");
        assert_eq!(format_size(1_500_000), "1.5 MB");
    }
}
