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

pub fn favourites() -> Vec<String> {
    settings().strv("favourites").iter().map(|s| s.to_string()).collect()
}

pub fn is_favourite(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy();
    favourites().iter().any(|f| *f == p)
}

pub fn toggle_favourite(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy().into_owned();
    let mut favs = favourites();
    let now = if let Some(i) = favs.iter().position(|f| *f == p) {
        favs.remove(i);
        false
    } else {
        favs.push(p);
        true
    };
    let refs: Vec<&str> = favs.iter().map(|s| s.as_str()).collect();
    let _ = settings().set_strv("favourites", refs.as_slice());
    now
}
