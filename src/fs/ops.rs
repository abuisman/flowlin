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
pub async fn restore_from_trash(originals: &[PathBuf]) -> usize {
    let trash = gio::File::for_uri("trash:///");
    let attrs = "standard::name,trash::orig-path,trash::deletion-date";
    let Ok(en) = trash.enumerate_children_future(attrs, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await
    else {
        return 0;
    };
    // orig path -> (deletion date, trash child), most recent wins.
    let mut found: std::collections::HashMap<PathBuf, (String, gio::File)> = Default::default();
    loop {
        let infos = match en.next_files_future(200, glib::Priority::DEFAULT).await {
            Ok(v) if !v.is_empty() => v,
            _ => break,
        };
        for info in infos {
            let Some(orig) = info.attribute_byte_string("trash::orig-path") else { continue };
            let orig = PathBuf::from(orig.as_str());
            if !originals.contains(&orig) {
                continue;
            }
            let date = info.attribute_string("trash::deletion-date").map(|s| s.to_string()).unwrap_or_default();
            let child = trash.child(info.name());
            match found.get(&orig) {
                Some((d, _)) if *d >= date => {}
                _ => {
                    found.insert(orig, (date, child));
                }
            }
        }
    }
    let mut n = 0;
    for (orig, (_, child)) in found {
        let dest = gio::File::for_path(&orig);
        let (fut, _progress) = child.move_future(&dest, gio::FileCopyFlags::NOFOLLOW_SYMLINKS, glib::Priority::DEFAULT);
        if fut.await.is_ok() {
            n += 1;
        }
    }
    n
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
