pub const APP_ID: &str = "com.ronkatil.Flowlin";
pub const APP_NAME: &str = "Flowlin";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GETTEXT_PACKAGE: &str = "flowlin";

/// Thumbnail cell widths (logical px) for the five size steps.
pub const THUMB_SIZES: [i32; 5] = [96, 144, 208, 320, 512];

/// Directory where the dev build compiled the GSettings schema (set by build.rs).
pub const DEV_SCHEMA_DIR: Option<&str> = option_env!("FLOWLIN_SCHEMA_DIR");
