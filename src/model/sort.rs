use std::cmp::Ordering;

use super::ImageItem;
use crate::util::natural_cmp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Created,
    Size,
    Type,
    Dimensions,
    Random,
    /// Folder then name (recursive mode).
    Folder,
}

impl SortKey {
    pub const ALL: [SortKey; 8] = [
        SortKey::Name,
        SortKey::Modified,
        SortKey::Created,
        SortKey::Size,
        SortKey::Type,
        SortKey::Dimensions,
        SortKey::Random,
        SortKey::Folder,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Modified => "modified",
            SortKey::Created => "created",
            SortKey::Size => "size",
            SortKey::Type => "type",
            SortKey::Dimensions => "dimensions",
            SortKey::Random => "random",
            SortKey::Folder => "folder",
        }
    }

    pub fn from_id(s: &str) -> SortKey {
        Self::ALL.into_iter().find(|k| k.id() == s).unwrap_or_default()
    }

    pub fn label(self) -> String {
        crate::i18n::tr(match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Date Modified",
            SortKey::Created => "Date Created",
            SortKey::Size => "Size",
            SortKey::Type => "Type",
            SortKey::Dimensions => "Dimensions",
            SortKey::Random => "Random",
            SortKey::Folder => "Folder, Then Name",
        })
    }

    /// Short label for the header bar button.
    pub fn short_label(self) -> String {
        crate::i18n::tr(match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Modified",
            SortKey::Created => "Created",
            SortKey::Size => "Size",
            SortKey::Type => "Type",
            SortKey::Dimensions => "Dimensions",
            SortKey::Random => "Random",
            SortKey::Folder => "Folder",
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct SortState {
    pub key: SortKey,
    pub descending: bool,
}

fn ext(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

impl SortState {
    pub fn compare(&self, a: &ImageItem, b: &ImageItem) -> Ordering {
        let by_name = || a.with_name(|an| b.with_name(|bn| natural_cmp(an, bn)));
        let o = match self.key {
            SortKey::Name => by_name(),
            SortKey::Modified => a.mtime().cmp(&b.mtime()).then_with(by_name),
            SortKey::Created => a.ctime().cmp(&b.ctime()).then_with(by_name),
            SortKey::Size => a.size().cmp(&b.size()).then_with(by_name),
            SortKey::Type => a
                .with_name(|an| b.with_name(|bn| ext(an).to_ascii_lowercase().cmp(&ext(bn).to_ascii_lowercase())))
                .then_with(by_name),
            SortKey::Dimensions => {
                let px = |i: &ImageItem| i.dimensions().map(|(w, h)| w as u64 * h as u64).unwrap_or(0);
                px(a).cmp(&px(b)).then_with(by_name)
            }
            SortKey::Random => a.random().cmp(&b.random()).then_with(by_name),
            SortKey::Folder => a.with_rel_dir(|ad| b.with_rel_dir(|bd| natural_cmp(ad, bd))).then_with(by_name),
        };
        let o = if self.descending { o.reverse() } else { o };
        // Keep the order total even for identical names in different folders.
        o.then_with(|| a.with_path(|ap| b.with_path(|bp| ap.cmp(bp))))
    }
}
