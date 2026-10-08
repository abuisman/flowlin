# Flowlin

A fast, private, keyboard-driven image browser for Linux.

Pick a folder in the tree, see its images as thumbnails, and step through them
full size with the arrow keys (or W/A/S/D). Press **R** to see every image in
a folder *and all its subfolders* in one grid. Flowlin stays smooth with tens
of thousands of files and never touches the network.

Built with GTK 4 and libadwaita, written in Rust. Works on Wayland and X11.

https://github.com/user-attachments/assets/904e579f-aea3-4c84-ac4b-e7b98d1fa4d1

- [Features](#features)
- [Installation](#installation)
- [Getting started](#getting-started)
- [Using Flowlin](#using-flowlin)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Preferences](#preferences)
- [Privacy](#privacy)
- [Supported formats](#supported-formats)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)

## Features

- **Folder tree** with favourites, Home, Pictures, the whole file system and
  mounted drives. Folders load lazily; folders without images are dimmed.
- **Three layouts**: grid, waterfall (masonry, keeps aspect ratios) and a
  sortable list with dimensions, size, date and type.
- **Full-size viewer** with fit / fill / 100 % / free zoom around the cursor,
  panning, high-quality downscaling, EXIF orientation, animated GIF / WebP /
  APNG, view-only rotation and a slideshow. Neighbouring images are decoded
  ahead of time, so stepping is instant.
- **Recursive mode**: all images below a folder in one grid, streamed in while
  the folder tree is walked.
- **Keyboard first**: W/A/S/D work like the arrow keys everywhere, Tab jumps
  between the folder tree and the thumbnails, Space opens and closes images.
- **Videos** (mp4, mkv, webm, mov, avi) with thumbnails and a player.
- **Tabs**, each with its own folder, history and recursive setting.
- **File management**: move to trash with undo, rename, copy / cut / paste
  (compatible with GNOME Files), drag and drop, new folder, open with another
  app, show in file manager, properties with an EXIF summary.
- **Star ratings** stored with the file (extended attributes, compatible with
  KDE), with filtering by rating.
- **Live updates** when files change on disk.
- **Private by design**: no network access, no thumbnail files on disk, no
  history of what you opened.

## Installation

Flowlin has no prebuilt packages yet; build it from source. It takes a few
minutes the first time.

### 1. Install the build dependencies

You need Rust **1.92 or newer**, GTK **4.14+**, libadwaita **1.5+** and
GStreamer (for video).

**Fedora**

```sh
sudo dnf install rust cargo gcc gtk4-devel libadwaita-devel gdk-pixbuf2-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel
```

**Ubuntu 24.04+ / Debian 13+**

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  libgdk-pixbuf-2.0-dev libglib2.0-dev-bin \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
```

The Rust compiler shipped by Debian and Ubuntu is too old. Install a current
one with [rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Immutable systems** (Fedora Silverblue / Kinoite, Bazzite, …): build inside a
[toolbox](https://containertoolbx.org/) or
[distrobox](https://distrobox.it/) container with the Fedora packages above.
The resulting program runs directly on the host, since these systems already
ship GTK 4 and libadwaita.

### 2. Build and install

```sh
git clone https://github.com/abuisman/flowlin.git
cd flowlin
cargo build --release
./build-aux/install.sh
```

`install.sh` installs into `~/.local` (no root needed): the `flowlin` program,
the app-menu entry, the icon and the settings schema. To install for all users
instead:

```sh
sudo ./build-aux/install.sh /usr/local
```

After installing, Flowlin appears in your application menu. If `~/.local/bin`
is on your `PATH`, you can also start it from a terminal with `flowlin`.

To try it without installing anything:

```sh
cargo run --release -- ~/Pictures
```

### Optional: more image formats

HEIF/HEIC, AVIF, JPEG XL and camera RAW previews are decoded by your system's
image loaders. Install them if you want those formats:

```sh
# Fedora
sudo dnf install glycin-loaders libheif webp-pixbuf-loader
# Ubuntu / Debian
sudo apt install heif-gdk-pixbuf webp-pixbuf-loader
```

Video playback depends on the GStreamer plugins installed on your system (for
example `gstreamer1-plugins-good`, `-bad-free` and `-libav` on Fedora,
`gstreamer1.0-plugins-good` / `-bad` / `-libav` on Debian and Ubuntu).

### Flatpak (experimental)

```sh
flatpak-builder --user --install build-dir build-aux/com.ronkatil.Flowlin.json
flatpak run com.ronkatil.Flowlin
```

The Flatpak has full access to your files (`--filesystem=host`). This is
deliberate: Flowlin is a file browser, and the folder tree and recursive mode
need to read folders directly. Portals only grant access to files you pick one
by one.

### Uninstalling

Remove the installed files (replace `~/.local` with the prefix you used):

```sh
rm ~/.local/bin/flowlin \
   ~/.local/share/applications/com.ronkatil.Flowlin.desktop \
   ~/.local/share/metainfo/com.ronkatil.Flowlin.metainfo.xml \
   ~/.local/share/icons/hicolor/*x*/apps/com.ronkatil.Flowlin.png \
   ~/.local/share/icons/hicolor/symbolic/apps/com.ronkatil.Flowlin-symbolic.svg \
   ~/.local/share/glib-2.0/schemas/com.ronkatil.Flowlin.gschema.xml
glib-compile-schemas ~/.local/share/glib-2.0/schemas
```

To also reset your preferences: `dconf reset -f /com/ronkatil/Flowlin/`.

## Getting started

Start Flowlin from the application menu, or from a terminal:

```sh
flowlin                      # opens your home folder
flowlin ~/Pictures/Holiday   # opens a folder
flowlin photo.jpg            # opens the folder and shows that image
```

You can also right-click a folder or image in your file manager and choose
**Open With → Flowlin**. Opening something while Flowlin is already running
adds a new tab to the existing window.

A typical session, keyboard only:

1. Use **W/S** (or ↑/↓) in the folder tree to walk through your folders. Each
   folder you land on is shown on the right.
2. Press **Tab** to jump into the thumbnails (Tab again jumps back).
3. Move with **W/A/S/D** or the arrow keys and press **Space** or **Enter** to
   view an image full size.
4. Step through images with **A/D** (or ←/→); **W/S** (↑/↓) move to the image
   one row above or below, exactly as in the grid.
5. Press **Space**, **Enter** or **Esc** to return to the thumbnails.

Press **Ctrl+?** at any time for the list of all shortcuts.

## Using Flowlin

### The window

- **Header bar**: sidebar toggle, favourite star, back / forward, the path of
  the current folder (click a part to go there; the last part lists sibling
  folders), the number of images, the include-subfolders toggle, layout
  buttons, open viewer, sort menu, main menu and "open in file manager".
- **Tabs**: Ctrl+T opens a tab, Ctrl+W closes it. Middle-click a folder in the
  tree to open it in a new tab.
- **Sidebar**: the folder tree. Right-click a folder for Open, Open with
  Subfolders, Open in New Tab, Add to Favourites, New Folder, Rename and Move
  to Trash. F9 hides or shows it.

### Browsing folders

- Click a folder in the tree, or move to it with the keyboard.
- **Space** on a folder in the tree selects its first image, ready to open.
- **Backspace** / Alt+↑ goes to the parent folder; Alt+←/→ go back and
  forward.
- **Ctrl+L** lets you type or paste a path.
- **Ctrl+D** adds the current folder to your favourites at the top of the tree.
- Typing letters in the thumbnails jumps to the first file whose name starts
  with them (letters other than W, A, S, D and R).

### Recursive mode (include subfolders)

Press **R**, click the search-folder button in the header, or right-click a
folder and choose **Open with Subfolders**. The grid then shows every image in
the folder and all folders below it. Each thumbnail shows which subfolder it
comes from, and the sort menu gains a "Folder, Then Name" option. Very large
trees are capped at 200 000 files (adjustable in Preferences).

### Layouts, sorting and filtering

- **Ctrl+1 / Ctrl+2 / Ctrl+3** switch between grid, waterfall and list.
- **Ctrl+plus / Ctrl+minus** or Ctrl+scroll change the thumbnail size.
- The sort menu sorts by name (natural order: `img2` before `img10`), date
  modified, date created, size, type, dimensions or randomly, ascending or
  descending. In the list layout you can also click the column headers.
- **Ctrl+F** opens the filter bar. Type part of a file name, or a rating
  condition such as `rating:>=3`, `rating:5` or `rating:<2`. You can combine
  both: `beach rating:>=4`.

### The viewer

| Action | Keys / mouse |
|---|---|
| Next / previous image | → / ← or D / A, mouse wheel, mouse side buttons |
| Image above / below in the grid | ↑ / ↓ or W / S |
| Close | Space, Enter, Esc or double-click |
| Zoom | + / −, Ctrl+scroll (zooms around the pointer) |
| 100 % / fit / fill | 1 / 0 or F / Shift+F; long-press left button = 100 %, long-press right button = fit |
| Pan | drag with the left button |
| Rotate (display only, not saved) | R / Shift+R |
| Slideshow | P |
| Show the info bar permanently | I |
| Full screen | F11 |

The viewer remembers "100 %", "fit" or "fill" when you choose them explicitly.
The info bar at the bottom shows the file name, position, dimensions, file
size and zoom level, and hides itself after two seconds.

### Videos

Videos appear in the grid with a play badge and start playing muted when
opened. A control bar provides play/pause, a seek bar and volume.

| Action | Keys |
|---|---|
| Seek 5 s back / forward | ← / → (or A / D) |
| Seek 10 s | Shift+← / Shift+→ |
| Volume up / down | ↑ / ↓ (or W / S) |
| Play / pause | K |
| Mute | M |
| Previous / next file | Page Up / Page Down |

Videos can be hidden with **Main menu → Show Videos**.

### Managing files

Select files with a click, Ctrl+click, Shift+click, or by dragging a rectangle
over empty space (Ctrl+A selects all), then use the right-click menu or:

- **Delete**: move to trash. An "Undo" button appears for 10 seconds.
- **Shift+Delete**: delete permanently, after confirmation.
- **F2**: rename.
- **Ctrl+C / Ctrl+X / Ctrl+V**: copy, cut and paste, also to and from your
  file manager. Pasting shows an Undo button too.
- **Drag and drop**: drag thumbnails onto a folder in the tree or onto a tab to
  move them (hold Ctrl to copy). Files dropped in from other apps are copied
  into the current folder; a dropped folder is opened.
- **Ctrl+Shift+N**: new folder. **Alt+Enter**: properties, with dimensions,
  colour type and an EXIF summary (camera, lens, exposure, whether GPS data is
  present).

### Ratings

Press **1**–**5** in the thumbnails to rate the selected images and **0** to
clear the rating. In the viewer use **2**–**5**, **Shift+1** for one star and
**Shift+0** to clear (plain 1 and 0 are zoom keys there). Stars are shown on
the thumbnails and in the list layout.

Ratings are stored in the file's `user.baloo.rating` extended attribute, the
same place KDE uses, so they move with the file. File systems without extended
attributes (for example FAT or exFAT USB sticks) cannot store ratings; Flowlin
tells you when that happens.

### Mouse gestures

Hold the right mouse button in the thumbnails, drag at least 40 pixels and
release:

| Drag | Action |
|---|---|
| Up | Parent folder |
| Down | Back |
| Left / right | Previous / next folder that contains images |
| Up-right | Next sibling folder that contains images |
| Down-right | Close tab |

A short right-click without dragging opens the context menu as usual.

## Keyboard shortcuts

**Folders**

| Keys | Action |
|---|---|
| W A S D | Move like the arrow keys (tree and thumbnails) |
| Tab | Switch between folder tree and thumbnails |
| Space (in the tree) | Select the folder's first image |
| Backspace, Alt+↑ | Parent folder |
| Alt+← / Alt+→ | Back / forward |
| R, Ctrl+Shift+R | Include subfolders |
| Ctrl+L | Edit location |
| Ctrl+D | Toggle favourite |
| F5, Ctrl+R | Refresh |
| Ctrl+H | Show hidden files |

**Thumbnails**

| Keys | Action |
|---|---|
| Space, Enter | Open viewer |
| Ctrl+1 / 2 / 3 | Grid / waterfall / list |
| Ctrl+plus / Ctrl+minus | Larger / smaller thumbnails |
| Ctrl+F | Filter |
| Ctrl+A | Select all |
| 1–5, 0 | Rate, clear rating |

**Files**

| Keys | Action |
|---|---|
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Shift+C | Copy path |
| F2 | Rename |
| Delete / Shift+Delete | Move to trash / delete permanently |
| Ctrl+Shift+N | New folder |
| Alt+Enter | Properties |

**Window**

| Keys | Action |
|---|---|
| Ctrl+T / Ctrl+W | New tab / close tab |
| F9 | Toggle sidebar |
| F11 | Full screen |
| Ctrl+, | Preferences |
| Ctrl+? | Keyboard shortcuts |
| Ctrl+Q | Quit |

The viewer and video keys are listed in [The viewer](#the-viewer) and
[Videos](#videos).

## Preferences

Open them from the main menu or with **Ctrl+,**:

- **Style**: follow the system, or always light or dark.
- **Show file names** and **Show folder under name in recursive mode**.
- **Single-key shortcuts**: turn off to make every letter go to name search
  (type-ahead) instead of W/A/S/D/R and ratings.
- **Show hidden files**.
- **Recursive mode stays on one file system**: don't descend into other
  mounted drives.
- **Recursive file limit** (default 200 000).
- **Slideshow interval** (default 3 seconds).
- **Thumbnail memory** (default 256 MB): how much memory thumbnails may use.

## Privacy

Flowlin is built to leave no trace of what you look at:

- It makes **no network requests** of any kind: no update checks, no
  telemetry.
- Thumbnails are kept **in memory only**. Nothing is written to
  `~/.cache/thumbnails` or anywhere else, and they are gone when you quit.
- It does not remember the last folder, a browsing history or recently
  opened files, and it does not add files to the desktop's "recent files"
  list.
- The only things saved are your preferences (layout, sort order, thumbnail
  size, window size, the settings above) and the folders you explicitly add
  to favourites.

The trade-off is that thumbnails are generated again each time you start
Flowlin. This is fast for common formats, because JPEG thumbnails are decoded
at reduced size directly from the file.

## Supported formats

| Formats | Decoded by |
|---|---|
| JPEG, PNG, APNG, GIF, WebP, BMP, TIFF, ICO, PNM/PPM/PGM/PBM, TGA, QOI | Flowlin itself (always available) |
| SVG, HEIF/HEIC, AVIF, JPEG XL, camera RAW (embedded preview) | your system's image loaders, if installed |
| MP4, M4V, MKV, WebM, MOV, AVI | GStreamer, with your system's codecs |

Files without an extension are recognised by their content. Files whose
extension doesn't match their content (a PNG named `.jpg`) are shown
correctly. Damaged files show a placeholder instead of crashing the app.

## Troubleshooting

**Some formats show a placeholder icon.**
Run `flowlin` from a terminal; the first log line lists which optional loaders
were found (for example `heic=yes jxl=no`). Install the missing loaders (see
[Optional: more image formats](#optional-more-image-formats)).

**Videos don't play or show no thumbnail.**
Install the GStreamer plugins for the codec in question (often
`gstreamer1-libav` / `gstreamer1.0-libav`).

**"Could not move to trash".**
Some locations, such as `/tmp` and other system-internal mounts, have no
trash. Use Shift+Delete to delete permanently.

**More detail in the log.**
Start Flowlin with `RUST_LOG=flowlin=debug flowlin`.
`FLOWLIN_FRAME_LOG=1` additionally reports slow frames, which helps when
reporting scrolling stutter.

## Development

```sh
cargo test                                    # unit tests + GUI smoke test
cargo run --example gen_fixtures -- /tmp/fx   # test folder tree with every format
cargo run --release --bin flowlin-bench -- /path/to/folder [--recursive]
RUST_LOG=flowlin=debug cargo run -- /path
```

The GUI smoke test runs the real app under `xvfb-run`. It is skipped when
`xvfb-run` isn't installed.

Before sending changes, please run `cargo fmt` and
`cargo clippy --all-targets -- -D warnings`, as CI does.

Translations use gettext. `build-aux/update-pot.sh` regenerates
`po/flowlin.pot` from the sources; add translations as `po/<language>.po` and
list the language in `po/LINGUAS`.

Project layout:

| Path | Contents |
|---|---|
| `src/app.rs`, `src/window.rs`, `src/tab.rs` | application, window, one browser tab |
| `src/sidebar/` | folder tree |
| `src/browser/` | grid, list and waterfall views, thumbnail cells |
| `src/viewer/` | full-size viewer, zoom/pan canvas, prefetching |
| `src/model/` | the image list: items, sorting, filtering |
| `src/fs/` | scanning, recursive walking, file operations, ratings |
| `src/thumbs/`, `src/decode/` | in-memory thumbnail pipeline, image/video decoding |
| `data/` | desktop file, AppStream metadata, settings schema, icons, CSS |
| `build-aux/` | install script, Flatpak manifest, translation script |

## License

Flowlin is free software, licensed under the
[GNU General Public License v3.0 or later](COPYING).
