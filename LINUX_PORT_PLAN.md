# Atlas on Linux (CachyOS + Niri/Wayland) — Port Plan

## Table of Contents

1. [Guiding Principles](#1-guiding-principles)
2. [Phase 0 — Baseline Build & Verification Harness](#2-phase-0--baseline-build--verification-harness)
3. [Phase 1 — Workspace / Cargo.toml Surgery](#3-phase-1--workspace--cargotoml-surgery)
4. [Phase 2 — atlas-model (No Changes Expected)](#4-phase-2--atlas-model-no-changes-expected)
5. [Phase 3 — atlas-core (The Big One)](#5-phase-3--atlas-core-the-big-one)
    - 3.1 Filesystem & Icons
    - 3.2 Watcher
    - 3.3 Recycle Bin
    - 3.4 Mounts / udisks
    - 3.5 Cleanup / Disk Analyzer
    - 3.6 Search
    - 3.7 Vault (WinFsp → FUSE)
    - 3.8 Cloud / Extensions
6. [Phase 4 — atlas-ui (GPUI / Wayland)](#6-phase-4--atlas-ui-gpui--wayland)
7. [Phase 5 — Windows-Only Crates](#7-phase-5--windows-only-crates)
8. [Phase 6 — Niri/Wayland-Specific Integration](#8-phase-6--niriwayland-specific-integration)
9. [Verification Strategy (Per-Crate)](#9-verification-strategy-per-crate)
10. [Per-Crate "Done" Checklist](#10-per-crate-done-checklist)
11. [Testing Matrix for CachyOS + Niri](#11-testing-matrix-for-cachyos--niri)
12. [Risks & Mitigations](#12-risks--mitigations)

---

## 1. Guiding Principles

- Respect the layered architecture described in `ARCHITECTURE.md`:
    ```
    atlas-ui -> atlas-core -> atlas-model
    atlas    -> atlas-ui + atlas-core + atlas-model
    atlas-model -> no runtime crates
    ```
- **Never** let a Windows API leak into `atlas-model`.
- **Every** OS-specific implementation in `atlas-core` must live behind `#[cfg(target_os = "windows")]` or `#[cfg(target_os = "linux")]` — never inline.
- The Linux path is a **first-class target**, not a shim. Prefer FreeDesktop.org standards over hardcoded paths.
- Every change must be verifiable by `cargo check`, `cargo clippy`, and `cargo test` on **both** Windows (CI) and Linux (CachyOS).
- Feature-flag aggressively. A missing feature should be a clean `#[cfg]` gate, not a compile error and not a silent no-op.

---

## 2. Phase 0 — Baseline Build & Verification Harness

**Goal:** Prove the project builds on Linux _as-is_ (mostly fails) and set up the feedback loop.

### 0.1 Install CachyOS build dependencies

```bash
sudo pacman -S --needed base-devel rustup clang pkgconf \
  libxcb wayland wayland-protocols libxkbcommon \
  vulkan-icd-loader vulkan-headers libvulkan \
  openssl fontconfig freetype2 alsa-lib \
  ffmpeg unzip 7zip unrar \
  gtk3 gtk4 libportal
```

`unrar`, `7z`, `ffmpeg`, `pdftoppm` (poppler), `gs` (ghostscript), `pandoc`, `libreoffice`, `rclone`, `ebook-convert` (calibre) are **optional runtime engines** — the app must function without them. They are only probed by `Tools::discover()` in `crates/atlas-core/src/convert/tools.rs`.

### 0.2 Toolchain

```bash
rustup toolchain install 1.97.1 --profile minimal -c rustfmt,clippy
rustup default 1.97.1
```

The repo pins `rust-toolchain.toml` to `1.97.1`, matching the requirement in `Cargo.toml` for `hint::cold_path` used by GPUI.

### 0.3 First baseline build attempt

```bash
cargo check --workspace 2>&1 | tee build_baseline.log
```

Capture the log — every error you see is a task in Phase 1 or 2. Categorize errors into:

- **Missing Windows crate** → Phase 1 (workspace Cargo.toml)
- **Windows-only API in code** → Phase 3/4 (feature gate)
- **Missing Linux crate** → add dependency
- **Wayland/GPUI backend** → Phase 4

### 0.4 Set up a repeatable verification script

Create `scripts/verify-linux.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

CRATE="${1:-atlas-core}"
MODE="${2:-check}"   # check | test | clippy

echo "==> Verifying $CRATE ($MODE)"

case "$MODE" in
  check)  cargo check  -p "$CRATE" --all-features ;;
  test)   RUST_MIN_STACK=16777216 cargo test -p "$CRATE" --all-features --no-fail-fast ;;
  clippy) cargo clippy -p "$CRATE" --all-features --all-targets -- -D warnings ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac

echo "==> OK: $CRATE ($MODE)"
```

`RUST_MIN_STACK=16777216` is mandated by `CONTRIBUTING.md` because the debug translation initializer needs a 16 MiB stack.

---

## 3. Phase 1 — Workspace / Cargo.toml Surgery

**Goal:** Make `cargo check` on Linux reach `atlas-model` and `atlas-core` before touching any code.

### 1.1 Root `Cargo.toml`

Add a `[target.'cfg(windows)'.dependencies]` guard for `atlas-win-space` — it already exists, good. Confirm it's under `[target.'cfg(windows)'.dependencies]`, not `[dependencies]`.

### 1.2 `crates/atlas-core/Cargo.toml`

Current problematic lines:

```toml
[target.'cfg(windows)'.dependencies]
atlas-win-space = { path = "../atlas-win-space" }
winfsp = { version = "0.13.2", default-features = false, features = ["delayload"] }
windows = { version = "0.61", features = [...] }
```

These are already correctly gated. Add a Linux section:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
# Pure-Rust mounting through fusermount3 (package fuse3); no libfuse headers to build.
fuser = { version = "0.15", default-features = false }
libc = "0.2"
```

Then guard Windows-only module paths in `src/vault.rs`:

```rust
#[cfg(windows)]
pub mod mount;
#[cfg(target_os = "linux")]
pub mod mount_linux;
#[cfg(target_os = "linux")]
pub use mount_linux as mount;
```

### 1.3 `crates/atlas-ui/Cargo.toml`

The entire `[target.'cfg(windows)'.dependencies]` block is fine — it will simply be skipped on Linux. But `atlas-win-space` is referenced in `Cargo.toml` under Windows deps, and in `src/dnd/windows/*` — those modules must be gated in `lib.rs` / `dnd/mod.rs`.

Verify the `gpui_platform` features already include `x11` and `wayland`:

```toml
gpui_platform = { ..., features = ["font-kit", "x11", "wayland"] }
```

✅ Already correct.

### 1.4 `crates/atlas-win-space` and `crates/atlas-icon-extension`

Remove from `[workspace]` members OR make them conditional. Recommended: keep them in the workspace but only build on Windows:

```toml
[workspace]
members = [
    ".",
    "crates/atlas-model",
    "crates/atlas-core",
    "crates/atlas-ui",
    "crates/atlas-publisher",
    "crates/atlas-cloud-extension",
    "crates/atlas-search-service",
    # Windows-only:
    # "crates/atlas-win-space",
    # "crates/atlas-icon-extension",
]
```

But `atlas-core` and `atlas-ui` reference them, so simply removing them breaks the Windows build. Better approach: keep them as workspace members, but ensure their Cargo.toml files have no Linux host deps and that nothing on the Linux build path pulls them in.

Then in `crates/atlas-win-space/Cargo.toml`, make sure `[target.'cfg(windows)'.dependencies]` is used, and the crate is `#[cfg(windows)]`-gated at the `lib.rs` root with a stubbed no-op API on other platforms — OR rely on the fact that the crate is only linked when `#[cfg(windows)]` blocks reference it. The latter requires that **no** non-Windows code path references it.

**Verification:**

```bash
cargo check -p atlas-model   # must succeed on Linux
```

---

## 4. Phase 2 — atlas-model (No Changes Expected)

`atlas-model` is explicitly designed with no runtime dependencies. Confirm with:

```bash
cargo tree -p atlas-model
```

You should see only `serde`, `serde_json`, `compact_str`, `chrono`, `uuid`, `dirs`, `toml`, `anyhow`. **No** `tokio`, **no** `gpui`, **no** `windows`, **no** `winfsp`.

Review these files for platform-specific assumptions:

- `location.rs` — already has `#[cfg(feature = "provider-host")]` gating ✅
- `file_entry.rs` — uses `std::path`, cross-platform ✅
- `profiles.rs` — uses `dirs::home_dir()`, cross-platform ✅
- `settings.rs` — plain serde ✅

**Action:** None required. This crate is the contract.

**Verification:**

```bash
./scripts/verify-linux.sh atlas-model check
./scripts/verify-linux.sh atlas-model test
./scripts/verify-linux.sh atlas-model clippy
```

---

## 5. Phase 3 — atlas-core (The Big One)

This is where ~90% of the porting work lives. Work module by module, top to bottom. **Do not proceed to Phase 4 until `cargo test -p atlas-core` passes on Linux.**

### 3.1 Filesystem & Icons

Files: `src/fs/list.rs`, `src/fs/icon_loader.rs` (in atlas-ui), `src/fs/preview/mod.rs`, `src/fs/preview/linux_thumb.rs`, `src/fs/desktop_apps.rs`.

**Current state:**

- `list.rs` has a `#[cfg(windows)]` native enumerator and a `#[cfg(not(windows))]` `list_dir_portable` fallback ✅
- `linux_thumb.rs` correctly implements the FreeDesktop Thumbnail spec ✅
- `desktop_apps.rs` correctly parses `.desktop` files ✅

**Tasks:**

1. **Audit `list.rs`:** Confirm `list_dir_portable` is used on Linux, and `entry_for_path` in the non-Windows branch does not `use std::os::windows::fs::MetadataExt` (it uses `#[cfg(windows)]` ✅).
2. **Icon loading:** `atlas-ui`'s `icon_loader.rs` and `thumbs.rs` are Windows-heavy. In `atlas-core`'s `preview/mod.rs`:
    - `is_cloud_placeholder` returns `false` on non-Windows ✅ (already implemented)
    - `thumbnail` path must route through `linux_thumb::freedesktop_thumbnail` first, then fall back to decoding images with `image` crate ✅ (already structured that way in `thumbs.rs`)
3. **Desktop apps:** Add unit tests for `DesktopApp::parse_str` on a real CachyOS `.desktop` fixture (e.g. `/usr/share/applications/org.gnome.Nautilus.desktop`).

**Verification:**

```bash
cargo test -p atlas-core fs:: --all-features
```

Add specific tests:

- Listing `/usr/share/applications` returns `.desktop` files
- `apps_for_file` returns a non-empty list for a `.png`
- `freedesktop_thumbnail` matches the MD5 hash spec

### 3.2 Watcher

File: `src/fs/watcher.rs`

**Current state:** Has a `#[cfg(not(windows))]` branch using `notify` ✅.

**Tasks:**

1. Confirm the `notify` backend on Linux is `inotify`. This works under Niri without extra privileges.
2. Test that recursive watching works. The current implementation uses `watch_path_recursive` in `space_lens.rs`, but `navigate` only uses `watch_path` (non-recursive). Add a Linux integration test that:
    - Creates a temp dir
    - Starts a `watch_path` handle
    - Writes a file, renames it, deletes it
    - Asserts the channel receives `Upsert`, `Rename`, `Remove`

**Verification:**

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core watcher --all-features
```

### 3.3 Recycle Bin

File: `src/fs/recycle_bin.rs`

**Current state:** Fully implements the FreeDesktop Trash spec ✅. The `parse_trashinfo`, `parse_deletion_date`, and `percent_decode` are already cross-platform.

**Tasks:**

1. Verify `accessible_bin_dirs()` returns `~/.local/share/Trash` on CachyOS (or `$XDG_DATA_HOME/Trash`).
2. Integration test:
    - Create a file
    - Send it to trash (`trash::delete`)
    - `list_recycle_bin()` returns it
    - `restore()` restores it
    - `purge()` deletes it

Note: the `trash` crate (v5) is already Linux-compatible, so no shims needed.

**Verification:**

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core recycle_bin --all-features
```

### 3.4 Mounts / udisks

Files: `src/fs/mounts.rs`, `src/fs/udisks.rs`

**Current state:**

- `mounts.rs` uses `sysinfo` for `list_drives` (cross-platform ✅) and `/proc/mounts` for Linux ✅
- `udisks.rs` calls `lsblk --json` and `udisksctl` (both available in CachyOS) ✅

**Tasks:**

1. `udisksctl` requires `udisks2` daemon — verify it's running:
    ```bash
    systemctl status udisks2
    ```
    CachyOS ships it by default. If not: `sudo pacman -S udisks2`.
2. Ensure `lsblk` is available: `pacman -S util-linux` (installed by default).
3. `sysinfo` on Linux reads `/proc` — confirm `Disks::new_with_refreshed_list()` works under Niri (it should).

**Verification:**

```bash
cargo test -p atlas-core mounts --all-features
cargo test -p atlas-core udisks --all-features  # will be no-op on non-Linux CI
```

On the target machine, add a manual test in `docs/` showing a USB device enumeration.

### 3.5 Cleanup / Disk Analyzer

Files: `src/cleanup/`

**Current state:**

- `mod.rs` publishes `ScanJob` and `verified` (`linux_verified` on Linux, `atlas_win_space::cleanup` on Windows) ✅
- `mft.rs` is NTFS-only and gated with `#[cfg(windows)]` ✅
- `linux_verified.rs` implements inode-pinned `openat(O_NOFOLLOW)` candidate inspection, verified deletion, and `statvfs` capacity checks ✅
- `walk.rs` is parallel and cross-platform ✅
- `tree.rs`, `policy.rs`, `scan.rs`, `execution.rs` are cross-platform ✅

**Tasks:**

1. In `src/cleanup/mod.rs`:
    ```rust
    #[cfg(windows)]
    mod mft;
    ```
2. In `src/cleanup/scan.rs`, replace the Windows MFT branch with a clean cfg gate:
    ```rust
    #[cfg(windows)]
    if self.snapshot.fast_scan_available && self.try_mft(&root) {
        return;
    }
    self.walk_tree(&root);
    ```
    ✅ Already structured this way.
3. Route `delete()` and `free()` in `execution.rs` through `super::verified::{delete_verified, capacity}` under `#[cfg(any(windows, target_os = "linux"))]`. ✅
4. `caps.rs` / `cleanup::ScanMode::FastFolder` must be rejected on Linux — `try_mft` will never be called because `fast_scan_available` is false on non-Windows.

**Verification:**

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core cleanup --all-features
```

Add integration tests:

- `ScanJob::start(..., ScanMode::Folder)` over a temp tree with known sizes
- Assert `snapshot.bytes` matches `du -sb`
- Assert `partial` status when unreadable subdir present
- Assert `walk.rs` parallel correctness (already tested in `walk.rs` tests ✅)

### 3.6 Search

Files: `src/search/`, `src/fs/index.rs`

**Current state:**

- `catalog.rs` — cross-platform trait ✅
- `nucleo_pool.rs` — cross-platform ✅
- `content.rs` — uses `grep-searcher`, cross-platform ✅
- `service.rs` — has `#[cfg(unix)]` Unix socket path ✅
- `volume_index.rs` — NTFS MFT-specific; only used by `atlas-search-service`
- `name.rs` — uses `ignore::WalkBuilder`, cross-platform ✅

**Tasks:**

1. **Primary backend on Linux:** `fs::index::build_index_scoped` is already called by `goto_index_probe.rs` and `fs/index.rs` — this is the intended Linux path.
2. `search/service.rs::linux_socket_path()` uses `$XDG_RUNTIME_DIR/atlas-search.sock` ✅ — works under Niri (systemd user session sets `XDG_RUNTIME_DIR`).
3. **`atlas-search-service` side:** `linux_daemon.rs` already exists and uses an in-memory `ArenaTree` — this is a complete Linux implementation. **No Windows-specific code in that crate for the Linux path.**
4. Add feature detection: if the search service is unavailable, fall back to `nucleo` in-process index. The `service.rs::is_service_available()` + `ensure_service_started()` already implement this fallback flow.

**Verification:**

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core search --all-features
RUST_MIN_STACK=16777216 cargo test -p atlas-search-service --all-features
```

Add:

- Native index test: create 10k temp files, verify `build_index_scoped` injects them
- Content search test: create files with known strings, verify hits
- Service test: spawn `atlas-search-service --console`, verify Unix socket responds (already present in `arena_index.rs` ✅)

### 3.7 Vault (WinFsp → FUSE)

Files: `src/vault/`, `src/vault/mount_linux.rs`

**This was the largest single effort and is now implemented and wired to the UI.**

**Tasks:**

1. **Gate and re-export the Linux mount:**

    ```rust
    // src/vault.rs
    #[cfg(windows)]
    pub mod mount;
    #[cfg(target_os = "linux")]
    pub mod mount_linux;
    #[cfg(target_os = "linux")]
    pub use mount_linux as mount;
    ```

2. **Implement FUSE mount in `mount_linux.rs`:**
    - Uses `fuser = { version = "0.15", default-features = false }` (pure-Rust mounting via `fusermount3`, no `libfuse` headers required)
    - Implements `fuser::Filesystem` trait backed by `Session`
    - Mirrors the WinFsp callbacks 1:1:
        - `lookup` → `Session::resolve`
        - `getattr` → `Session::entry`
        - `readdir` → `Session::children`
        - `open` / `release` → handle tracking
        - `read` / `write` → `Session::read_range` / `Session::write_at`
        - `create` → `Session::create_entry`
        - `unlink` → `Session::delete_entry`
        - `rename` → `Session::rename_entry`
    - Mount point: `$XDG_RUNTIME_DIR/atlas-private-mounts/<uuid>/` with `DefaultPermissions`, `NoDev`, `NoSuid` (no `AutoUnmount`, avoiding `user_allow_other` in `/etc/fuse.conf`), plus `remove_stale_mounts()` on startup to clean up orphaned `ENOTCONN` mountpoints
    - Reuses all existing `Control`, `Session`, and mutable-block logic

3. **Locking:** The `Session` holds an `fs2::FileExt::try_lock_exclusive` on `lock`. This is cross-platform ✅. `Session` has `Send`-safe interior state; `fuser::Filesystem` thread-safety is satisfied via `Mutex<Option<Session>>` inside `Shared`.

4. **`atlas_core::vault::live::NAMESPACE`:** `"atlas-private-mounts"` — reused on Linux under `$XDG_RUNTIME_DIR`.

**Verification:**

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core vault --all-features
RUST_MIN_STACK=16777216 cargo test -p atlas-core --test linux_integration -- --ignored private_folder
```

### 3.8 Cloud / Extensions

Files: `src/cloud/`, `src/ext/`

**Current state:** Both are process-oriented and platform-neutral. The `rclone` binary has a `.exe` suffix on Windows and none on Linux — already handled by `std::env::consts::EXE_SUFFIX` in `installation.rs`.

**Tasks:**

1. Verify `atlas-cloud-extension` builds on Linux — its only deps are `atlas-core` + `anyhow` + `serde_json` + `tokio`. ✅
2. Verify `atlas-core/src/ext/install.rs::ensure_packaged_icon_provider` is Windows-only (it references `.exe` — gate with `#[cfg(windows)]` or verify the sibling detection works on Linux).
3. `rclone` discovery: `installation.rs` uses `env::current_exe()` and looks for `atlas-cloud-extension` (with no `.exe` on Linux) ✅.

**Verification:**

```bash
cargo check -p atlas-cloud-extension
RUST_MIN_STACK=16777216 cargo test -p atlas-core ext --all-features
RUST_MIN_STACK=16777216 cargo test -p atlas-cloud-extension
```

---

## 6. Phase 4 — atlas-ui (GPUI / Wayland)

### 4.1 Confirm the GPUI Wayland backend

`gpui_platform` features already include `wayland` (see `Cargo.toml`). Confirm the actual crate has the backend enabled. If GPUI only does X11 under Wayland via `xwayland`, the app will still work but without native protocol integration. **Test first:**

```bash
WAYLAND_DISPLAY=wayland-1 cargo run --release
```

If it fails with a Wayland error, fall back to `WINIT_UNIX_BACKEND=x11` and file a support issue upstream. Do **not** patch vendored GPUI unless absolutely necessary — the fork in `vendor/gpui-ce` is intentionally pinned.

### 4.2 Gate `shell_thumb`, `shell_menu`, `dnd/windows`, `shell_file_operation`

In `crates/atlas-ui/src/lib.rs`:

```rust
#[cfg(windows)]
pub(crate) mod shell_file_operation;
#[cfg(windows)]
pub(crate) mod shell_menu;
#[cfg(windows)]
pub(crate) mod shell_thumb;
pub(crate) mod dnd;   // dnd/mod.rs already gates its windows submodule
```

Then in `dnd/mod.rs`, the `#[cfg(target_os = "windows")] pub mod windows;` is already correct.

### 4.3 Implement Linux Drag & Drop

File: `src/dnd/mod.rs` — `mod linux` provides `LinuxDndState`, `LinuxDropPayload`, `LinuxDndEvent`, and `handle_drop_uri_list` (accepting `text/uri-list`).

**Status:** Verified working on Wayland/Niri — both dragging files out of Atlas to external file managers and dropping files into Atlas work through GPUI's native `wl_data_device` integration.

**Verification:** Manual drag Atlas → Nautilus/Thunar and back. Automated unit tests cover the pure-DnD logic in `dnd/mod.rs` ✅.

### 4.4 Replace Shell Context Menu

Files: `src/shell_menu/`

Windows-only — gate as above. For Linux, build a context menu from:

- `atlas-core::fs::desktop_apps::apps_for_file(path)` — "Open With"
- Native file operations (cut, copy, paste, rename, trash)
- Custom actions (already in `palette/`)

**Tasks:**

1. Add a Linux branch to whatever builds the context menu spec.
2. `.desktop` `Exec=` templates must be parsed and launched via the existing `build_launch_command` ✅.

### 4.5 Replace Shell Icons

Files: `src/shell_thumb.rs`, `src/icon_theme.rs`, `src/app/icon_loader.rs`, `src/freedesktop_icons.rs`

Windows-only — gate `shell_thumb.rs`. Non-Windows icon pipeline (implemented ✅):

- `freedesktop_icons.rs` — parses FreeDesktop `index.theme`, walks `Directories=`, follows `Inherits=`, prefers SVG over PNG
- `app/icon_loader.rs::rasterize_multi` — parses SVG icon themes via `resvg` ✅
- `ui/thumbs.rs` — decodes image thumbnails via `image` crate ✅
- `linux_thumb::freedesktop_thumbnail` for cached FreeDesktop thumbnails

### 4.6 Window Chrome & Niri

**Niri is a scrollable tiling compositor with its own decorations and window rules.**

1. GPUI's client-side decorations render cleanly under Niri without fighting the compositor.
2. **Window rules:** Documented in `docs/linux.md`:
    ```kdl
    window-rule {
        match app-id="atlas"
        default-column-width { proportion 0.6; }
    }
    ```
    The application ID is set in `WindowOptions { app_id: Some("atlas".to_string()), .. }` ✅.

### 4.7 Clipboard

Files: `src/dnd/windows/ole_clipboard.rs` (Windows-only), `src/app/ops.rs` (`linux_clipboard`).

**Status:** Implemented ✅. `arboard` has `wayland-data-control` enabled, and `ops.rs::linux_clipboard::offer_files` uses `wl-clipboard-rs` (`Options::new().copy_multi`) on Wayland to offer `text/uri-list`, `x-special/gnome-copied-files` (`copy`/`cut`), `application/x-kde-cutselection`, and `text/plain` simultaneously, with an X11 fallback via `arboard`.

### 4.8 Everything Else

Grep the whole `crates/atlas-ui/src` tree for `windows::` (the crate) and `winfsp`. Every hit must be inside a `#[cfg(windows)]` block:

```bash
grep -rn "windows::\|winfsp\|atlas_win_space" crates/atlas-ui/src --include="*.rs" \
  | grep -v "#\[cfg(windows)\]" | grep -v "//"
```

## Phase 4.9 — Archive extraction context menu

### Motivation

Right-clicking an archive in Atlas currently offers "Open" (browse-in-place via
`fs/preview/archive.rs`) and "Convert files" (repack to another archive format
via `convert/`). Neither gives the user the everyday operation they expect
from any file manager: **extract the archive contents to disk**.

Competing file managers (Nautilus, Thunar, Dolphin, 7-Zip on Windows, WinRAR)
all expose this as two right-click entries:

- **Extract here** — extract contents next to the archive
- **Extract to…** — extract into a user-chosen folder

Add both to the Atlas context menu, gated on the selected files being archives.

### What we already have

The core reading side is complete. All four backends exist and are
`#[cfg]`-gated correctly:

- `crates/atlas-core/src/fs/archive/zip_backend.rs` — native via `zip` crate
- `crates/atlas-core/src/fs/archive/tar_backend.rs` — native via `tar` +
  `flate2`/`xz2`/`bzip2`/`zstd` for `.tar.gz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`
- `crates/atlas-core/src/fs/archive/sevenz_backend.rs` — native via `sevenz-rust2`
- `crates/atlas-core/src/fs/archive/rar_backend.rs` — via bundled `unrar` (gated
  behind the `rar` feature; system `unrar` fallback)
- `crates/atlas-core/src/fs/archive/sanitize.rs` — path traversal guard
- `crates/atlas-core/src/fs/archive/bomb_guard.rs` — decompression-bomb guard
- `crates/atlas-core/src/fs/archive/cache.rs` — cache for browse-in-place
- `crates/atlas-core/src/fs/compress.rs` — the **compress** counterpart
  (mirror API shape to follow)

These are used today by `fs/preview/archive.rs` for the "browse archive
without extracting" flow. **Nothing in the current tree calls them for
write-to-disk extraction.** `convert/native_archive` explicitly repacks
without extracting (test: `native_archive_repack_preserves_contents_without_extracting`).

### What's missing

1. A public **`extract` API** in `atlas-core` that writes the archive's
   entries to disk, respecting the existing sanitize + bomb-guard helpers.
2. An **archive-detection predicate** usable at render time on Linux (the
   context menu can't shell out). Extension-based, matching what the four
   backends accept.
3. **Two context-menu entries** in `atlas-ui` with archive-only visibility.
4. **Progress + cancellation** plumbing (mirrors `BgResult::OpComplete` used by
   copy/move today).
5. **Password prompt** for encrypted `.zip`/`.7z`/`.rar` (single modal,
   retry-on-failure).
6. **Conflict handling** (files already exist at the destination).

### Sub-phases

#### 4.9.1 — Core `extract` API (atlas-core)

New public function in `crates/atlas-core/src/fs/archive/mod.rs`:

```rust
pub struct ExtractOptions {
    /// Extract into `<dest>/<archive_stem>/` instead of `<dest>/` directly.
    pub subfolder: bool,
    /// What to do on conflict (per-entry).
    pub on_conflict: OnConflict,
    /// Optional password for encrypted archives.
    pub password: Option<String>,
    /// Cancellation handle.
    pub cancel: Arc<AtomicBool>,
}

pub enum OnConflict {
    /// Overwrite the existing file.
    Overwrite,
    /// Skip and report in the summary.
    Skip,
    /// Abort the whole extraction.
    Abort,
}

pub struct ExtractReport {
    pub extracted: usize,
    pub skipped: usize,
    pub bytes_written: u64,
    pub errors: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

pub async fn extract_archive(
    archive: &Path,
    dest_dir: &Path,
    options: ExtractOptions,
) -> Result<ExtractReport>;
```

---

## 7. Phase 5 — Windows-Only Crates

### `crates/atlas-win-space`

- Keep as a workspace member (Windows build needs it).
- Root `lib.rs` is gated with `#![cfg(windows)]` and only pulled in via `[target.'cfg(windows)'.dependencies]`.

### `crates/atlas-icon-extension`

- Cross-platform Open VSX icon-theme downloader/extractor (`crates/atlas-icon-extension/src/main.rs`), built and packaged alongside `atlas`, `atlas-search-service`, and `atlas-cloud-extension`.

### `crates/atlas-search-service`

Has a complete `linux_daemon.rs` (Unix socket + ArenaTree index). **No Windows-specific code on the Linux path** — verify with:

```bash
cargo check -p atlas-search-service --target x86_64-unknown-linux-gnu
```

---

## 8. Phase 6 — Niri/Wayland-Specific Integration

| Concern                         | Action                                                                                       |
| ------------------------------- | -------------------------------------------------------------------------------------------- |
| Window decorations              | GPUI client-side decorations work cleanly under Niri                                         |
| App ID                          | Set `app_id: Some("atlas")` — already done ✅                                                |
| Global shortcuts                | Use `org.freedesktop.portal.GlobalShortcuts` or `niri`'s `binds` — likely unnecessary for v1 |
| Native file pickers             | `rfd` uses `xdg-desktop-portal` — verify `xdg-desktop-portal-gtk` or `-gnome` is running     |
| Scale factor                    | GPUI reads `wl_output.scale` — verify UI at 1x, 1.5x, 2x                                     |
| Fractional scaling              | Niri supports it; verify GPUI handles fractional correctly                                   |
| Notification (toasts)           | Currently rendered in-window — fine, no portal needed                                        |
| Autostart                       | User's responsibility, documented in `docs/linux.md`                                         |
| File associations (`MimeType=`) | Shipped in `packaging/linux/atlas.desktop` and documented in `docs/linux.md`                 |
| Search service                  | `systemd --user` unit shipped in `packaging/linux/atlas-search.service`                      |

Example Niri config addition (documented in `docs/linux.md`):

```kdl
// ~/.config/niri/config.kdl
window-rule {
    match app-id="atlas"
    default-column-width { proportion 0.6; }
}
```

---

## 9. Verification Strategy (Per-Crate)

### General principle

Verification must be **crate-scoped** and **automated** wherever possible. Manual smoke tests are only acceptable for things that fundamentally cannot be automated (DnD gestures, compositor integration).

### Verification commands

For **`atlas-model`** (must pass, full suite):

```bash
./scripts/verify-linux.sh atlas-model check
./scripts/verify-linux.sh atlas-model test
./scripts/verify-linux.sh atlas-model clippy
```

For **`atlas-core`** (must pass, full suite — this is the gate before touching UI):

```bash
./scripts/verify-linux.sh atlas-core check
RUST_MIN_STACK=16777216 cargo test -p atlas-core --all-features --no-fail-fast -- --nocapture
./scripts/verify-linux.sh atlas-core clippy
```

### Compile-only verification

```bash
cargo check -p atlas-core --all-features
cargo check -p atlas-core --no-default-features
cargo check -p atlas-core --all-features --target x86_64-unknown-linux-gnu
```

### Integration & VM GUI tests

- `crates/atlas-core/tests/linux_integration.rs` — covers watcher, FreeDesktop trash/restore/purge, FreeDesktop thumbnail URI hashes, folder scan totals, 10k-file native index, content search, FUSE vault mount (`-- --ignored`), and 100k-file listing/search budgets (`--release -- --ignored`).
- `scripts/arch-vm/vm.sh` + `scripts/arch-vm/gui-test.sh` — boots a headless Arch Linux QEMU/KVM VM and runs 16 GUI scenarios under headless `sway` (`wtype` + `grim` + `wl-clipboard`).

Run core integration tests:

```bash
RUST_MIN_STACK=16777216 cargo test -p atlas-core --test linux_integration --no-fail-fast
```

### Feature-matrix verification

`atlas-core` has several features (`free`, `provider-host`, `cloud-backend`, `rar`, `sevenz`). Verify all meaningful combinations on Linux:

```bash
for f in "" "provider-host" "cloud-backend" "provider-host,cloud-backend" \
         "rar" "sevenz" "provider-host,cloud-backend,rar,sevenz"; do
  echo "==> features: [$f]"
  cargo check -p atlas-core --no-default-features --features "$f"
done
```

---

## 10. Per-Crate "Done" Checklist

A crate is "done" when **all** of the following are true on a CachyOS + Niri machine:

Ticked items were verified on 2026-11-17: CI (Ubuntu 24.04 and Windows), a Windows 10
machine, and a headless Arch Linux VM running the `atlas-git` package under sway and
Xvfb (`scripts/arch-vm/`). Strict clippy still fails on code that predates the port
(see CONTRIBUTING.md).

### `atlas-model`

- [x] `cargo check -p atlas-model` passes
- [x] `cargo test -p atlas-model` passes
- [ ] `cargo clippy -p atlas-model -- -D warnings` passes
- [x] `cargo tree -p atlas-model` shows no `tokio`, `gpui`, `windows`, `winfsp`, `atlas-win-space`

### `atlas-core`

- [x] `cargo check -p atlas-core --all-features` passes
- [x] `cargo check -p atlas-core --no-default-features` passes
- [x] `cargo check -p atlas-core --target x86_64-unknown-linux-gnu` passes (from a Windows host too)
- [x] `cargo test -p atlas-core --all-features --no-fail-fast` passes
- [ ] `cargo clippy -p atlas-core --all-features --all-targets -- -D warnings` passes
- [x] Feature-matrix check passes (all combos above)
- [x] No `grep -rn "atlas_win_space\|winfsp\|windows::" crates/atlas-core/src --include="*.rs"` hits that are outside a `#[cfg(windows)]` block
- [x] All Linux-only modules (`fs::desktop_apps`, `fs::udisks`, `preview::linux_thumb`, `vault::mount_linux`, `cleanup::linux_verified`) have passing unit/integration tests
- [x] The Linux binary can list `/`, `/home`, and `/usr/share/applications` without error
- [x] The Linux binary can send a file to trash and restore it

### `atlas-ui`

- [x] `cargo check -p atlas-ui --all-features` passes on Linux
- [x] `cargo test -p atlas-ui --lib` passes
- [ ] `grep -rn "windows::\|winfsp\|atlas_win_space\|shell_menu\|shell_thumb" crates/atlas-ui/src --include="*.rs"` returns only hits inside `#[cfg(windows)]` blocks
- [x] The binary launches a window under Niri
- [x] A folder can be browsed, a file opened with `xdg-open`, a file renamed and trashed via the UI
- [x] DnD into/out of Nautilus works
- [x] Clipboard copy/paste into Nautilus works

### `atlas-search-service`

- [x] `cargo check -p atlas-search-service` passes
- [x] `cargo test -p atlas-search-service` passes
- [x] Binary responds on `$XDG_RUNTIME_DIR/atlas-search.sock`

### `atlas-cloud-extension`

- [x] `cargo check -p atlas-cloud-extension` passes
- [x] `rclone` is discovered from `PATH` or from the bundled path

### `atlas-win-space`, `atlas-icon-extension`

- [x] `cargo check -p atlas-win-space --target x86_64-pc-windows-msvc` still passes (Windows-only)
- [x] `atlas-win-space` is absent from the Linux build graph (`cargo tree -p atlas-core -e normal --target x86_64-unknown-linux-gnu` must not list it)

---

## 11. Testing Matrix for CachyOS + Niri

| Scenario                                | Status | Expected result                                                |
| --------------------------------------- | ------ | -------------------------------------------------------------- |
| Launch under Niri                       | ✅     | Window appears, native Wayland                                 |
| Browse `/`                              | ✅     | Lists root dir, no crash                                       |
| Browse `/home`                          | ✅     | Lists home contents                                            |
| Open `~/.config`                        | ✅     | Hidden dirs visible toggle works                               |
| Open a `.png`                           | ✅     | Opens in default app via `xdg-open`                            |
| Search for a file via Ctrl+K            | ✅     | Fuzzy results stream in                                        |
| Search in content via goto bar          | ✅     | `grep`-based results appear                                    |
| Create a folder                         | ✅     | Appears in listing                                             |
| Copy 100 files                          | ✅     | Progress bar shows, files copied                               |
| Move files across partitions            | ✅     | Progress + completion                                          |
| Send to Trash                           | ✅     | Appears in `~/.local/share/Trash/files`                        |
| Restore from Trash                      | ✅     | File back at original path                                     |
| Empty Trash                             | ✅     | Trash cleared                                                  |
| Convert PNG → JPG                       | ✅     | Works if `ffmpeg`/`magick` present                             |
| Open a `.zip`                           | ✅     | Archive browse mode works                                      |
| Open a `.rar`                           | ✅     | Archive browse mode works if `unrar` present                   |
| Create a private folder                 | ✅     | FUSE mount appears in `$XDG_RUNTIME_DIR/atlas-private-mounts/` |
| Lock private folder                     | ✅     | FUSE unmounts, ciphertext remains                              |
| Delete a file                           | ✅     | Removed from disk (Shift+Del)                                  |
| Drag file out to Nautilus               | ✅     | URI list accepted                                              |
| Drag file in from Nautilus              | ✅     | Dropped at target folder                                       |
| Copy file, paste in Nautilus            | ✅     | Paste works                                                    |
| Copy file from Nautilus, paste in Atlas | ✅     | Paste works                                                    |
| Switch profile                          | ✅     | Layout and theme switch                                        |
| Settings → change theme                 | ✅     | Repaints correctly                                             |
| Ctrl+, → open Settings                  | ✅     | Renders without crash                                          |
| Command palette `Ctrl+Shift+P`          | ✅     | Opens                                                          |
| Restart app                             | ✅     | Session restored                                               |
| Move window between workspaces          | ✅     | Layout survives                                                |
| HiDPI at 1.5x                           | ⏳     | Text scales; icons do not blur                                 |

---

## 12. Risks & Mitigations

| Risk                                                      | Likelihood | Mitigation                                                                                                                |
| --------------------------------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------- |
| GPUI Wayland backend missing required protocol (DnD, IME) | Low        | DnD and Wayland clipboard (`wl-clipboard-rs` + `arboard` `wayland-data-control`) are both verified working.               |
| FUSE mount callback ordering differs from WinFsp          | Low        | `mount_linux.rs` uses pure-Rust `fuser` (`default-features = false`) via `fusermount3`, tested in `linux_integration.rs`. |
| `unrar` license                                           | Low        | Already `git`-gated behind the `rar` feature. Document in `NOTICE`.                                                       |
| Fractional scaling in GPUI                                | Medium     | Test on 1.5x. If broken, document and file upstream.                                                                      |
| Path length (`MAX_PATH`) assumptions                      | Low        | `std::path` handles it; test with deep directories.                                                                       |
| `udisksctl` requiring polkit authorization                | Medium     | CachyOS ships `polkit`; test with a real USB. If it fails, use `gio mount` as fallback.                                   |
| Missing `.desktop` files on minimal systems               | Low        | Fall back to opening with the system default via `open` crate.                                                            |
| `systemd --user` search-service failing                   | Low        | `service.rs::ensure_service_started()` already falls back to in-process `nucleo` ✅.                                      |
| Non-free firmware / codecs                                | Low        | `ffmpeg` in CachyOS is built with all codecs.                                                                             |

---

# Linux Port — Status & Deviations from the Plan

_Last updated: 2026-11-22, after PR #1 (FUSE vault UI wiring, Wayland clipboard, verified cleanup, Arch PKGBUILD, VM GUI harness) and external DnD verification._

## Completed phases

### Phase 0 — Baseline ✓

- `cargo check --workspace` succeeded on Linux with 0 errors at baseline.
- Toolchain: rustc 1.97.1, CachyOS, Linux 7.3.3-1-cachyos, Niri 26.04 Wayland.
- Dependencies installed: `fuse3 3.18.3`, `ffmpeg n9.0.2`, `lsblk` (util-linux), `udisks2`, plus the standard X/Wayland/Vulkan stack.

### Phase 1 — Cargo gating ✓

- `crates/atlas-win-space/src/lib.rs` gated with `#![cfg(windows)]`; `crates/atlas-icon-extension/src/main.rs` restored as a cross-platform Open VSX theme downloader/extractor (`#![windows_subsystem = "windows"]`).
- Root `Cargo.toml` fixed to `path = "crates/atlas-win-space"`.
- `fuser = { version = "0.15", default-features = false }` + `libc = "0.2"` added to `[target.'cfg(target_os = "linux")'.dependencies]` in `atlas-core/Cargo.toml` (mounts via `fusermount3` without needing `libfuse` C headers).
- Verified with `cargo tree -p atlas-core --target x86_64-unknown-linux-gnu`: no `atlas-win-space`, no `winfsp`, no `windows v0.61` in the Linux normal graph.

### Phase 2 — atlas-model ✓

No changes needed. `cargo tree -p atlas-model` confirms no runtime deps. All tests pass on Linux unchanged.

### Phase 3 — atlas-core ✓ **COMPLETE**

All 8 sub-phases verified, including `crates/atlas-core/tests/linux_integration.rs` covering watcher events, FreeDesktop trash/restore/purge, FreeDesktop thumbnail URI hashing, folder scans, 10k-file native indexing, content search, FUSE vault mounting, and 100k-file performance budgets.

Key highlights:

- **3.1–3.4 FS, watcher, recycle bin, mounts:** Cross-platform and FreeDesktop implementations verified (`list_dir_portable`, `inotify` watcher, FreeDesktop Trash, `sysinfo` + `/proc/mounts` + `lsblk`).
- **3.5 Cleanup (`linux_verified.rs` & `policy.rs`):** Implemented `openat(O_NOFOLLOW)` inode-pinned candidate verification and safe deletion, `statvfs` capacity reporting, and `/proc/self/mountinfo` mountpoint detection (with octal escape decoding). `user_temp_root()` and `categories()` target `$XDG_CACHE_HOME` (`~/.cache`) and deliberately exclude the Trash. - **3.6 Search:** Unix socket at `$XDG_RUNTIME_DIR/atlas-search.sock` + `atlas-search-service` (`linux_daemon.rs`) verified.
- **3.7 Vault / FUSE (`mount_linux.rs` & `vault.rs`):** Re-exported `mount_linux as mount`. Uses `DefaultPermissions`, `NoDev`, and `NoSuid` without `AutoUnmount` (avoiding `user_allow_other` in `/etc/fuse.conf`), and cleans up stale `ENOTCONN` mountpoints on startup via `remove_stale_mounts()`.
- **3.8 Cloud / Extensions:** Cross-platform `rclone` sidecar discovery verified.

### Phase 4 — atlas-ui (Core Wayland/Desktop Integration ✓)

- **4.3 External & Internal DnD ✓:** Both internal pane-to-pane DnD and external DnD into/out of Atlas work natively on Wayland/Niri via GPUI's `wl_data_device` integration.
- **4.4 Context menu ✓:** Works on Linux with `.desktop` "Open with" entries (`desktop_apps::apps_for_file`) and **Disk cleanup** on volume mountpoints.
- **4.5 Icons ✓:** Implemented `freedesktop_icons.rs` (`index.theme` parser with `Inherits=` resolution and SVG preference), wired into `icon_theme.rs`, `icon_loader.rs`, `file_icons.rs`, and sidebar drive icons.
- **4.6 Window chrome & Session persistence ✓:** GPUI decorations render cleanly under Niri. Added `app.save_on_quit()` in the Linux window-close handler (`crates/atlas-ui/src/lib.rs`) because GPUI releases the view before `on_app_quit` fires on Linux.
- **4.7 Clipboard ✓:** Enabled `arboard`'s `wayland-data-control` feature and added `wl-clipboard-rs` (`ops.rs::linux_clipboard::offer_files`) to publish `text/uri-list`, `x-special/gnome-copied-files`, `application/x-kde-cutselection`, and `text/plain` simultaneously on Wayland, with an X11 fallback.
- **4.8 Vault UI wiring ✓:** Enabled `vault_mount`, `poll_vault`, `close_native_vault`, `manage_native_vault`, `mount_ready`, and `render_private_session_bar` on Linux (`#[cfg(any(windows, target_os = "linux"))]`).

### Packaging & Automated VM Testing ✓

- **`packaging/linux/arch/PKGBUILD` & `packaging/linux/atlas-search.service`:** Full Arch/CachyOS `atlas-git` package building all four binaries (`atlas`, `atlas-search-service`, `atlas-cloud-extension`, `atlas-icon-extension`) and installing `.desktop`, icon, systemd user unit, license, and docs.
- **`docs/linux.md`:** User and contributor documentation for Arch/CachyOS, optional dependencies, FUSE private folders, search daemon, Niri/Hyprland rules, cleanup, clipboard, and VM testing.
- **`scripts/arch-vm/vm.sh` & `scripts/arch-vm/gui-test.sh`:** Automated QEMU/KVM Arch VM harness and 16-scenario headless `sway` GUI test suite (`wtype`, `grim`, `wl-clipboard`).

---

## Remaining Work

1. **Phase 4.9 — Archive Extraction Context Menu:** Implement `extract_archive` in `atlas-core` and wire **Extract here** / **Extract to…** context menu entries (with progress, cancellation, password prompt, and conflict handling) in `atlas-ui`.
2. **Code Hygiene & Warning Sweep:**
    - Gate Windows-only UI modules (`shell_thumb.rs`, `shell_menu/`, `dnd/windows/`) at the module level with `#[cfg(windows)]` to eliminate remaining dead-code warnings in `atlas-ui`.
    - Address pre-existing `cargo clippy -- -D warnings` lints across workspace crates.
3. **Sidebar Mount Deduplication:** Group btrfs subvolumes under their parent block device in `mounts.rs::list_drives_internal` so multi-subvolume NVMe drives do not render duplicate sidebar rows.
4. **Docs Polish (`docs/linux.md`):** Update the Known Limits note regarding external drag-and-drop now that Wayland drag-out/in is verified working.
