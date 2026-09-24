//! Folder enumeration and the recursive walker. Both run on a worker thread
//! and stream batches back to the UI through a channel.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::formats;
use crate::util::is_hidden_name;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    /// Folder relative to the scan root ("" for the root itself).
    pub rel_dir: String,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
}

impl FileEntry {
    pub fn from_path(path: &Path, root: &Path) -> Option<FileEntry> {
        let md = std::fs::metadata(path).ok()?;
        if !md.is_file() {
            return None;
        }
        Some(Self::new(path.to_path_buf(), root, &md))
    }

    fn new(path: PathBuf, root: &Path, md: &Metadata) -> FileEntry {
        let name = crate::util::file_name(&path);
        let rel_dir = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let secs = |t: std::io::Result<std::time::SystemTime>| {
            t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
        };
        FileEntry { name, rel_dir, size: md.len(), mtime: secs(md.modified()), ctime: secs(md.created()), path }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    pub recursive: bool,
    pub show_hidden: bool,
    pub same_device: bool,
    pub cap: usize,
    /// Include video files.
    pub videos: bool,
}

#[derive(Debug)]
pub enum ScanMsg {
    Batch(Vec<FileEntry>),
    Done { total: usize, truncated: bool },
    Error(String),
}

/// Cancellation token shared with the worker.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Batches grow so the first thumbnails show up immediately while large
/// folders still arrive in few model updates.
struct Batcher {
    tx: async_channel::Sender<ScanMsg>,
    buf: Vec<FileEntry>,
    limit: usize,
    last: Instant,
    total: usize,
}

impl Batcher {
    fn push(&mut self, e: FileEntry) -> bool {
        self.buf.push(e);
        self.total += 1;
        if self.buf.len() >= self.limit || self.last.elapsed() > Duration::from_millis(60) {
            return self.flush();
        }
        true
    }

    fn flush(&mut self) -> bool {
        self.last = Instant::now();
        if self.buf.is_empty() {
            return true;
        }
        self.limit = (self.limit * 4).min(2000);
        let batch = std::mem::take(&mut self.buf);
        self.tx.send_blocking(ScanMsg::Batch(batch)).is_ok()
    }
}

pub fn spawn_scan(root: PathBuf, opts: ScanOptions, cancel: Cancel) -> async_channel::Receiver<ScanMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("flowlin-scan".into())
        .spawn(move || {
            let started = Instant::now();
            let mut b = Batcher { tx: tx.clone(), buf: Vec::new(), limit: 48, last: Instant::now(), total: 0 };
            let result =
                if opts.recursive { walk(&root, opts, &cancel, &mut b) } else { list(&root, opts, &cancel, &mut b) };
            if cancel.is_cancelled() {
                return;
            }
            b.flush();
            match result {
                Ok(truncated) => {
                    tracing::debug!(
                        "scan of {} ({} files, recursive={}) took {:?}",
                        root.display(),
                        b.total,
                        opts.recursive,
                        started.elapsed()
                    );
                    let _ = tx.send_blocking(ScanMsg::Done { total: b.total, truncated });
                }
                Err(e) => {
                    let _ = tx.send_blocking(ScanMsg::Error(e.to_string()));
                }
            }
        })
        .expect("spawn scan thread");
    rx
}

/// Stat a directory entry, following symlinks to files but never to directories.
fn file_metadata(path: &Path, ft: std::fs::FileType) -> Option<Metadata> {
    if ft.is_symlink() {
        let md = std::fs::metadata(path).ok()?;
        md.is_file().then_some(md)
    } else if ft.is_file() {
        std::fs::symlink_metadata(path).ok()
    } else {
        None
    }
}

fn list(root: &Path, opts: ScanOptions, cancel: &Cancel, b: &mut Batcher) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(root)? {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !opts.show_hidden && is_hidden_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !formats::is_media(&path, true, opts.videos) {
            continue;
        }
        if let Some(md) = file_metadata(&path, ft) {
            if b.total >= opts.cap {
                return Ok(true);
            }
            if !b.push(FileEntry::new(path, root, &md)) {
                return Ok(false);
            }
        }
    }
    Ok(false)
}

fn walk(root: &Path, opts: ScanOptions, cancel: &Cancel, b: &mut Batcher) -> std::io::Result<bool> {
    let walker =
        walkdir::WalkDir::new(root).follow_links(false).same_file_system(opts.same_device).into_iter().filter_entry(
            |e| {
                e.depth() == 0
                    || !e.file_type().is_dir()
                    || opts.show_hidden
                    || !is_hidden_name(&e.file_name().to_string_lossy())
            },
        );
    for entry in walker {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if e.depth() == 0 {
                    return Err(e.into());
                }
                continue; // unreadable subfolder: skip
            }
        };
        let ft = entry.file_type();
        if ft.is_dir() {
            continue;
        }
        if !opts.show_hidden && is_hidden_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        if !formats::is_media(path, true, opts.videos) {
            continue;
        }
        if let Some(md) = file_metadata(path, ft) {
            if b.total >= opts.cap {
                return Ok(true);
            }
            if !b.push(FileEntry::new(entry.into_path(), root, &md)) {
                return Ok(false);
            }
        }
    }
    Ok(false)
}

/// Blocking helper used by tests and the bench binary.
pub fn scan_blocking(root: &Path, opts: ScanOptions) -> (Vec<FileEntry>, bool) {
    let rx = spawn_scan(root.to_path_buf(), opts, Cancel::default());
    let mut all = Vec::new();
    while let Ok(msg) = rx.recv_blocking() {
        match msg {
            ScanMsg::Batch(v) => all.extend(v),
            ScanMsg::Done { truncated, .. } => return (all, truncated),
            ScanMsg::Error(_) => break,
        }
    }
    (all, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn png(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        image::RgbImage::new(2, 2).save_with_format(p, image::ImageFormat::Png).unwrap();
    }

    fn opts(recursive: bool) -> ScanOptions {
        ScanOptions { recursive, show_hidden: false, same_device: false, cap: 200_000, videos: false }
    }

    #[test]
    fn flat_and_recursive_counts() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        png(&r.join("a.png"));
        png(&r.join("sub/b.png"));
        png(&r.join("sub/deeper/c.png"));
        png(&r.join(".hidden/d.png"));
        png(&r.join(".e.png"));
        std::fs::write(r.join("notes.txt"), "x").unwrap();
        assert_eq!(scan_blocking(r, opts(false)).0.len(), 1);
        let (all, truncated) = scan_blocking(r, opts(true));
        assert!(!truncated);
        let mut rel: Vec<_> = all.iter().map(|e| (e.rel_dir.clone(), e.name.clone())).collect();
        rel.sort();
        assert_eq!(
            rel,
            [("".into(), "a.png".into()), ("sub".into(), "b.png".into()), ("sub/deeper".into(), "c.png".into())]
        );
        let mut o = opts(true);
        o.show_hidden = true;
        assert_eq!(scan_blocking(r, o).0.len(), 5);
    }

    #[test]
    fn symlink_cycle_is_not_followed_but_file_links_are() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        png(&r.join("x/a.png"));
        symlink(r, r.join("x/loop")).unwrap(); // directory cycle
        symlink(r.join("x/a.png"), r.join("link.png")).unwrap(); // file link
        symlink(r.join("missing.png"), r.join("dangling.png")).unwrap();
        let (all, _) = scan_blocking(r, opts(true));
        let mut names: Vec<_> = all.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["a.png", "link.png"]);
    }

    #[test]
    fn cap_truncates() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..10 {
            png(&d.path().join(format!("{i}.png")));
        }
        let mut o = opts(true);
        o.cap = 4;
        let (all, truncated) = scan_blocking(d.path(), o);
        assert_eq!(all.len(), 4);
        assert!(truncated);
    }
}
