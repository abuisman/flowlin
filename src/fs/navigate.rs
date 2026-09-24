//! FlowVision-style folder jumps: previous / next folder *that contains
//! images*, in disk-wide depth-first, natural-alphabetical order.

use std::path::{Path, PathBuf};

use super::formats;
use crate::util::{is_hidden_name, natural_cmp};

/// Pseudo filesystems that are never interesting and can be huge.
const SKIP_AT_ROOT: &[&str] = &["/proc", "/sys", "/dev", "/run"];

/// Sorted subfolders of `dir` (symlinked folders excluded to avoid cycles).
pub fn subdirs(dir: &Path, show_hidden: bool) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && is_hidden_name(&name) {
                return None;
            }
            let p = e.path();
            if SKIP_AT_ROOT.iter().any(|s| p == Path::new(s)) {
                return None;
            }
            Some((name, p))
        })
        .collect();
    v.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// Does `dir` directly contain at least one image?
pub fn has_images(dir: &Path, show_hidden: bool) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    rd.filter_map(|e| e.ok()).any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !show_hidden && is_hidden_name(&name) {
            return false;
        }
        let p = e.path();
        let is_file = match e.file_type() {
            Ok(t) if t.is_symlink() => p.is_file(),
            Ok(t) => t.is_file(),
            Err(_) => false,
        };
        is_file && formats::is_image(&p, false)
    })
}

fn preorder_next(p: &Path, show_hidden: bool) -> Option<PathBuf> {
    if let Some(first) = subdirs(p, show_hidden).into_iter().next() {
        return Some(first);
    }
    let mut cur = p.to_path_buf();
    loop {
        let parent = cur.parent()?.to_path_buf();
        let sibs = subdirs(&parent, show_hidden);
        if let Some(i) = sibs.iter().position(|s| *s == cur) {
            if let Some(n) = sibs.get(i + 1) {
                return Some(n.clone());
            }
        }
        cur = parent;
    }
}

fn preorder_prev(p: &Path, show_hidden: bool) -> Option<PathBuf> {
    let parent = p.parent()?.to_path_buf();
    let sibs = subdirs(&parent, show_hidden);
    match sibs.iter().position(|s| s == p) {
        Some(i) if i > 0 => {
            let mut q = sibs[i - 1].clone();
            while let Some(last) = subdirs(&q, show_hidden).pop() {
                q = last;
            }
            Some(q)
        }
        _ => Some(parent),
    }
}

/// Walk from `start` in the given direction until a folder with images is
/// found. `budget` limits how many folders are inspected.
pub fn find_image_folder(start: &Path, forward: bool, show_hidden: bool, budget: usize) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    for _ in 0..budget {
        cur = if forward { preorder_next(&cur, show_hidden)? } else { preorder_prev(&cur, show_hidden)? };
        if has_images(&cur, show_hidden) {
            return Some(cur);
        }
    }
    None
}

/// Next *sibling* folder (same parent) that contains images, used by the
/// up-right gesture.
pub fn next_sibling_with_images(start: &Path, show_hidden: bool) -> Option<PathBuf> {
    let parent = start.parent()?;
    let sibs = subdirs(parent, show_hidden);
    let i = sibs.iter().position(|s| s == start)?;
    sibs.into_iter().skip(i + 1).find(|s| has_images(s, show_hidden))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"not decoded here").unwrap();
    }

    #[test]
    fn depth_first_order_skips_empty_folders() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("root");
        // root/a (img), root/a/a1 (img), root/b (empty), root/b/b1 (img), root/c10, root/c2 (img)
        img(&r.join("a/x.jpg"));
        img(&r.join("a/a1/x.jpg"));
        std::fs::create_dir_all(r.join("b/b1")).unwrap();
        img(&r.join("b/b1/x.png"));
        img(&r.join("c2/x.png"));
        img(&r.join("c10/x.png"));
        std::fs::create_dir_all(r.join("d_empty")).unwrap();

        let n = |p: &Path| find_image_folder(p, true, false, 1000);
        assert_eq!(n(&r.join("a")), Some(r.join("a/a1")));
        assert_eq!(n(&r.join("a/a1")), Some(r.join("b/b1")));
        assert_eq!(n(&r.join("b/b1")), Some(r.join("c2")));
        assert_eq!(n(&r.join("c2")), Some(r.join("c10")));

        let p = |p: &Path| find_image_folder(p, false, false, 1000);
        assert_eq!(p(&r.join("c10")), Some(r.join("c2")));
        assert_eq!(p(&r.join("c2")), Some(r.join("b/b1")));
        assert_eq!(p(&r.join("b/b1")), Some(r.join("a/a1")));
        assert_eq!(p(&r.join("a/a1")), Some(r.join("a")));
    }

    #[test]
    fn sibling_jump() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("a")).unwrap();
        std::fs::create_dir_all(r.join("b")).unwrap();
        img(&r.join("c/x.gif"));
        assert_eq!(next_sibling_with_images(&r.join("a"), false), Some(r.join("c")));
        assert_eq!(next_sibling_with_images(&r.join("c"), false), None);
    }
}
