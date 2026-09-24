//! Thin gettext wrapper. English strings only in v1; catalogues are looked up
//! in the standard locale directory so translations can be dropped in later.

use crate::config::GETTEXT_PACKAGE;

pub fn init() {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    let dir = option_env!("FLOWLIN_LOCALEDIR").unwrap_or("/usr/share/locale");
    let _ = gettextrs::bindtextdomain(GETTEXT_PACKAGE, dir);
    let _ = gettextrs::bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8");
    let _ = gettextrs::textdomain(GETTEXT_PACKAGE);
}

pub fn tr(msgid: &str) -> String {
    gettextrs::gettext(msgid)
}

/// Plural-aware translation.
pub fn trn(singular: &str, plural: &str, n: u32) -> String {
    gettextrs::ngettext(singular, plural, n)
}
