//! File operations via GIO, run asynchronously on the main loop. Nothing is
//! ever deleted with `rm`: "delete" means trash unless explicitly permanent.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

/// Move files to the trash. Returns the paths that were trashed.
pub async fn trash(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<glib::Error>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for p in paths {
        match gio::File::for_path(p).trash_future(glib::Priority::DEFAULT).await {
            Ok(()) => ok.push(p.clone()),
            Err(e) => errs.push(e),
        }
    }
    (ok, errs)
}

/// Put previously trashed files back where they were (used by Undo).
/// Implements the freedesktop.org Trash spec directly (the format GIO
/// writes), so it works without GVfs' `trash://` backend.
pub async fn restore_from_trash(originals: &[PathBuf]) -> usize {
    let originals = originals.to_vec();
    gio::spawn_blocking(move || trash_spec::restore(&originals)).await.unwrap_or(0)
}

mod trash_spec {
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    /// Trash directories that may hold `orig`: the home trash plus the
    /// per-volume ones on `orig`'s filesystem.
    fn trash_dirs(orig: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
        let mut out = vec![(gtk::glib::user_data_dir().join("Trash"), None)];
        if let Some(top) = mount_top(orig) {
            let uid = owner_uid();
            out.push((top.join(".Trash").join(uid.to_string()), Some(top.clone())));
            out.push((top.join(format!(".Trash-{uid}")), Some(top)));
        }
        out
    }

    fn owner_uid() -> u32 {
        std::fs::metadata(gtk::glib::home_dir()).map(|m| m.uid()).unwrap_or(0)
    }

    /// Topmost directory of the filesystem containing `p`.
    fn mount_top(p: &Path) -> Option<PathBuf> {
        let parent = p.parent()?;
        let dev = std::fs::metadata(parent).ok()?.dev();
        let mut top = parent.to_path_buf();
        while let Some(up) = top.parent() {
            match std::fs::metadata(up) {
                Ok(m) if m.dev() == dev => top = up.to_path_buf(),
                _ => break,
            }
        }
        Some(top)
    }

    fn parse_info(text: &str) -> Option<(String, String)> {
        let mut path = None;
        let mut date = String::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("Path=") {
                path = gtk::glib::Uri::unescape_string(v, None::<&str>).map(|s| s.to_string());
            } else if let Some(v) = line.strip_prefix("DeletionDate=") {
                date = v.to_string();
            }
        }
        Some((path?, date))
    }

    pub fn restore(originals: &[PathBuf]) -> usize {
        let mut restored = 0;
        for orig in originals {
            if orig.exists() {
                continue;
            }
            // (deletion date, info file, trashed file); newest wins.
            let mut best: Option<(String, PathBuf, PathBuf)> = None;
            for (dir, top) in trash_dirs(orig) {
                let Ok(rd) = std::fs::read_dir(dir.join("info")) else { continue };
                for e in rd.flatten() {
                    let info = e.path();
                    if info.extension().is_none_or(|x| x != "trashinfo") {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&info) else { continue };
                    let Some((path, date)) = parse_info(&text) else { continue };
                    let full = match &top {
                        Some(t) if !path.starts_with('/') => t.join(&path),
                        _ => PathBuf::from(&path),
                    };
                    if full != *orig {
                        continue;
                    }
                    let Some(stem) = info.file_stem() else { continue };
                    let file = dir.join("files").join(stem);
                    if best.as_ref().is_none_or(|(d, _, _)| date > *d) {
                        best = Some((date, info.clone(), file));
                    }
                }
            }
            if let Some((_, info, file)) = best {
                if std::fs::rename(&file, orig).is_ok() {
                    let _ = std::fs::remove_file(info);
                    restored += 1;
                }
            }
        }
        restored
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn info_parsing() {
            let (p, d) =
                parse_info("[Trash Info]\nPath=/home/u/My%20Pics/a.jpg\nDeletionDate=2026-09-24T10:00:00\n").unwrap();
            assert_eq!(p, "/home/u/My Pics/a.jpg");
            assert_eq!(d, "2026-09-24T10:00:00");
        }
    }
}

pub async fn delete_permanently(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<glib::Error>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for p in paths {
        match gio::File::for_path(p).delete_future(glib::Priority::DEFAULT).await {
            Ok(()) => ok.push(p.clone()),
            Err(e) => errs.push(e),
        }
    }
    (ok, errs)
}

/// A destination path in `dir` for `name` that does not exist yet:
/// `a.jpg`, `a (copy).jpg`, `a (copy 2).jpg`, …
pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for i in 1.. {
        let n = if i == 1 { format!("{stem} (copy){ext}") } else { format!("{stem} (copy {i}){ext}") };
        let c = dir.join(n);
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

/// Copy or move `sources` into `dest_dir`. Returns (source, destination)
/// pairs that succeeded so the caller can offer undo.
pub async fn transfer(
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: Transfer,
) -> (Vec<(PathBuf, PathBuf)>, Vec<glib::Error>) {
    let mut done = Vec::new();
    let mut errs = Vec::new();
    for src in sources {
        if mode == Transfer::Move && src.parent() == Some(dest_dir) {
            continue; // moving onto itself is a no-op
        }
        let name = crate::util::file_name(src);
        let dest = unique_destination(dest_dir, &name);
        let sf = gio::File::for_path(src);
        let df = gio::File::for_path(&dest);
        let res = match mode {
            Transfer::Copy => sf.copy_future(&df, gio::FileCopyFlags::ALL_METADATA, glib::Priority::DEFAULT).0.await,
            Transfer::Move => sf.move_future(&df, gio::FileCopyFlags::ALL_METADATA, glib::Priority::DEFAULT).0.await,
        };
        match res {
            Ok(()) => done.push((src.clone(), dest)),
            Err(e) => errs.push(e),
        }
    }
    (done, errs)
}

pub async fn rename(path: &Path, new_name: &str) -> Result<PathBuf, glib::Error> {
    let f = gio::File::for_path(path).set_display_name_future(new_name, glib::Priority::DEFAULT).await?;
    Ok(f.path().unwrap_or_else(|| path.with_file_name(new_name)))
}

pub async fn new_folder(parent: &Path, name: &str) -> Result<PathBuf, glib::Error> {
    let dest = unique_destination(parent, name);
    gio::File::for_path(&dest).make_directory_future(glib::Priority::DEFAULT).await?;
    Ok(dest)
}

/// Reveal `path` in the system file manager (FileManager1 via GTK/portal).
pub fn show_in_file_manager(path: &Path, parent: Option<&gtk::Window>) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.open_containing_folder(parent, gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!("open containing folder: {e}");
        }
    });
}

/// Open a folder itself in the file manager.
pub fn open_folder_in_file_manager(path: &Path, parent: Option<&gtk::Window>) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.launch(parent, gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!("open folder: {e}");
        }
    });
}

/// "Open with…": always show the application chooser.
pub fn open_with(path: &Path, parent: Option<&gtk::Window>) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.set_always_ask(true);
    launcher.launch(parent, gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!("open with: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(unique_destination(d.path(), "a.jpg"), d.path().join("a.jpg"));
        std::fs::write(d.path().join("a.jpg"), b"").unwrap();
        assert_eq!(unique_destination(d.path(), "a.jpg"), d.path().join("a (copy).jpg"));
        std::fs::write(d.path().join("a (copy).jpg"), b"").unwrap();
        assert_eq!(unique_destination(d.path(), "a.jpg"), d.path().join("a (copy 2).jpg"));
        std::fs::write(d.path().join("noext"), b"").unwrap();
        assert_eq!(unique_destination(d.path(), "noext"), d.path().join("noext (copy)"));
    }
}
