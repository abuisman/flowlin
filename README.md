# Flowlin

A fast local image browser for Linux, modelled on [FlowVision](https://flowvision.app)
for macOS. GTK 4 + libadwaita, written in Rust.

Pick a folder in the tree, see its images as thumbnails, press Enter to step
through them full size with the arrow keys. Press **R** to include every
subfolder in one grid.

## Privacy

Flowlin makes no network requests. It also leaves no trace of what you browse:

- Thumbnails are kept **in memory only** (a byte-capped LRU, 256 MB by
  default); nothing is written to `~/.cache/thumbnails` or anywhere else.
- No last-opened folder, no history on disk, no recent-files entries (GTK's
  recent-files list is disabled for the process).
- Stored settings are UI preferences only (layout, sort order, thumbnail
  size, window size) plus the favourites you star explicitly.

## Features

- Folder tree (favourites, Home, Pictures, Computer, mounted volumes), lazy
  and asynchronous; folders without images are dimmed.
- Tabs, each with its own folder, history, selection and recursive flag.
- Grid, waterfall (masonry) and list layouts, virtualised for folders with tens
  of thousands of files.
- Viewer: fit / fill / 100 % / free zoom around the cursor, pan, long-press for
  100 %, trilinear downscaling (no moiré), EXIF orientation, animated
  GIF/WebP/APNG, view-only rotation, slideshow (**P**), prefetching of neighbours.
- Recursive mode streams results while the walk runs (capped at 200 000 files
  by default), skips hidden and symlinked folders.
- Keyboard-first browsing: **W/A/S/D** work like the arrow keys everywhere
  (folder tree, thumbnails, viewer). **Tab** switches between the folder tree
  and the thumbnails. In the tree, moving to a folder opens it and **Space**
  focuses its first image; **Space** / **Enter** open and close the viewer.
  In the viewer, Up/Down move one grid row. **R** includes subfolders.
- Videos (mp4, mkv, webm, mov, avi): thumbnails via GStreamer (in memory),
  playback with media controls; arrows seek 5 s (Shift: 10 s), Up/Down
  volume, **K** play/pause, **M** mute, Page Up/Down previous/next file.
- Right-button drag gestures: up = parent, down = back, left/right =
  previous/next folder that contains images, up-right = next sibling with
  images, down-right = close tab.
- File management: trash with undo, permanent delete, rename, cut/copy/paste
  (compatible with Nautilus), drag and drop, new folder, open with, show in
  file manager, properties with EXIF summary.
- Ratings (0–5 stars) stored in `user.baloo.rating` xattrs; filter with
  `rating:>=3` in the filter bar (Ctrl+F).
- Live updates via file monitoring; F5 refreshes.

Press **Ctrl+?** in the app for all keyboard shortcuts.

## Building

The minimum Rust version is **1.92** (set by gtk-rs 0.11). Fedora ships a new
enough `cargo`; on Debian 13 and Ubuntu 24.04 install Rust with
[rustup](https://rustup.rs).

```sh
# Fedora
sudo dnf install rust cargo gtk4-devel libadwaita-devel gdk-pixbuf2-devel

# Ubuntu 24.04+ / Debian 13+ (plus rustup for the toolchain)
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  libgdk-pixbuf-2.0-dev libglib2.0-dev-bin

cargo build --release
./target/release/flowlin ~/Pictures
```

`cargo run` works without installing anything: the build compiles the
GSettings schema next to the binary. To install system-wide or per user:

```sh
./build-aux/install.sh              # ~/.local
sudo ./build-aux/install.sh /usr/local
```

On immutable systems (Silverblue, Bazzite, …) build inside a toolbox or
distrobox; the resulting binary runs on the host if it has GTK ≥ 4.14 and
libadwaita ≥ 1.5.

### Flatpak

```sh
flatpak-builder --user --install build-dir build-aux/eu.ronkatil.Flowlin.json
```

The manifest uses `--filesystem=host` on purpose: Flowlin is a file browser,
and the folder tree and recursive walks need direct access. Portals only give
access to individually chosen files or folders, which breaks browsing.

For local builds the manifest lets cargo fetch crates. A Flathub submission
needs offline sources generated with `flatpak-cargo-generator.py`.

## Formats

JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA and QOI are decoded
in-process (JPEG thumbnails with DCT scaling). SVG, HEIF/HEIC, AVIF, JPEG XL
and RAW previews use the system's gdk-pixbuf/glycin loaders when installed;
availability is logged at start-up (`RUST_LOG=flowlin=info`).

## Development

```sh
cargo test                                   # unit tests + GUI smoke test (needs xvfb-run)
cargo run --example gen_fixtures -- /tmp/fx  # fixture tree with every format
cargo run --release --bin flowlin-bench -- /path/to/folder   # timing checks
RUST_LOG=flowlin=debug cargo run -- /path
```

## License

GPL-3.0-or-later.
