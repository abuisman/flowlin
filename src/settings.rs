//! GSettings access. The schema is looked up system-wide first and falls back
//! to the copy compiled by build.rs for uninstalled development builds.

use gtk::gio;
use gtk::prelude::*;

use crate::config::{APP_ID, DEV_SCHEMA_DIR};

thread_local! {
    static SETTINGS: gio::Settings = create();
}

fn create() -> gio::Settings {
    let default = gio::SettingsSchemaSource::default();
    if let Some(schema) = default.as_ref().and_then(|s| s.lookup(APP_ID, true)) {
        return gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
    }
    if let Some(dir) = DEV_SCHEMA_DIR {
        if let Ok(src) = gio::SettingsSchemaSource::from_directory(dir, default.as_ref(), false) {
            if let Some(schema) = src.lookup(APP_ID, false) {
                return gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
            }
        }
    }
    panic!("GSettings schema {APP_ID} not found; run install.sh or build with cargo");
}

/// The application settings (main thread).
pub fn settings() -> gio::Settings {
    SETTINGS.with(|s| s.clone())
}

pub fn thumb_size() -> i32 {
    let i = settings().int("thumbnail-size").clamp(0, 4) as usize;
    crate::config::THUMB_SIZES[i]
}

/// Favourite folders. Stored as `file://` URIs so any byte sequence in a
/// name survives; plain paths from older settings are accepted too.
pub fn favourites() -> Vec<std::path::PathBuf> {
    settings().strv("favourites").iter().filter_map(|s| entry_to_path(s.as_str())).collect()
}

fn entry_to_path(s: &str) -> Option<std::path::PathBuf> {
    if s.starts_with("file://") {
        gtk::gio::File::for_uri(s).path()
    } else {
        Some(std::path::PathBuf::from(s))
    }
}

pub fn is_favourite(path: &std::path::Path) -> bool {
    favourites().iter().any(|f| f == path)
}

pub fn toggle_favourite(path: &std::path::Path) -> bool {
    let mut favs = favourites();
    let now = if let Some(i) = favs.iter().position(|f| f == path) {
        favs.remove(i);
        false
    } else {
        favs.push(path.to_path_buf());
        true
    };
    let uris: Vec<String> =
        favs.iter().map(|p| gtk::prelude::FileExt::uri(&gtk::gio::File::for_path(p)).to_string()).collect();
    let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
    let _ = settings().set_strv("favourites", refs.as_slice());
    now
}
