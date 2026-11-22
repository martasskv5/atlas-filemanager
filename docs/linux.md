# Atlas on Linux

Atlas runs natively on Wayland and X11. It uses the FreeDesktop trash, icon themes,
thumbnail cache and `.desktop` applications, FUSE for private folders and a Unix-socket
search daemon.

## Arch Linux (and CachyOS, EndeavourOS)

Build and install from source:

```sh
git clone https://github.com/martasskv5/atlas-filemanager.git
cd atlas-filemanager/packaging/linux/arch
makepkg -si
```

To package a local checkout instead of the GitHub branch:

```sh
ATLAS_GIT="git+file://$PWD/../../..#branch=$(git branch --show-current)" makepkg -si
```

Remove it with `sudo pacman -R atlas-git`.

The release build of `atlas-ui` peaks at about 10 GB of RAM in a single `rustc`
process, so build on a machine with 16 GB or add swap.

Optional packages enable more features; Atlas works without them:

| Package | Enables |
| --- | --- |
| `fuse3` | Private folders open as a mounted folder (otherwise they open in Atlas's private view) |
| `udisks2` | Mounting and ejecting removable drives |
| `xdg-desktop-portal` + a backend | Native file pickers |
| `ffmpeg`, `poppler`, `libreoffice-fresh`, `pandoc` | Previews and local conversion |
| `rclone` | Cloud drives |
| `7zip`, `unrar` | More archive formats |

## Private folders

An unlocked private folder is mounted with FUSE at
`$XDG_RUNTIME_DIR/atlas-private-mounts/<random id>`, readable only by your user (no
`allow_other`, so `/etc/fuse.conf` needs no changes). Only ciphertext is written to disk.
Locking the folder, idling for five minutes or quitting Atlas unmounts it. If Atlas is
killed, the stale mount point is removed the next time a private folder is opened; to do
it by hand: `fusermount3 -u -z "$XDG_RUNTIME_DIR"/atlas-private-mounts/*`.

## Search

`Ctrl+K` and the search bar use `atlas-search-service`, which Atlas starts on demand and
which listens on `$XDG_RUNTIME_DIR/atlas-search.sock`. To keep the index warm for the
whole session:

```sh
systemctl --user enable --now atlas-search.service
```

## Window managers

The window's app ID is `atlas`. Example rules:

```kdl
// niri: ~/.config/niri/config.kdl
window-rule {
    match app-id="atlas"
    default-column-width { proportion 0.6; }
}
```

```ini
# Hyprland: ~/.config/hypr/hyprland.conf
windowrulev2 = size 60% 80%, class:^(atlas)$
```

Use Atlas as the default file manager:
`xdg-mime default atlas.desktop inode/directory`.

## Disk cleanup

The cleanup panel analyzes any folder. Cleanable categories are regenerable caches under
`~/.cache` (browser caches, thumbnails, package-manager caches); the Trash is never
offered. Before deleting, Atlas re-checks that the file and each of its parent folders
are still the same inodes, have no other hard links and were not modified, and it never
follows symlinks.

## Clipboard

Copying files puts them on the clipboard as `text/uri-list`, which other file managers
paste. On Wayland Atlas uses the data-control protocol (sway, niri, Hyprland, KDE) and adds
plain text for editors and the GNOME and KDE cut markers, so Nautilus and Dolphin move what
Atlas cut. Where the compositor lacks data-control, Atlas falls back to the X11 clipboard
through XWayland, with `text/uri-list` only.

## Known limits

- Dragging files out of Atlas works on Wayland; on X11 only dropping into Atlas works.
- Windows-only features: the native shell context menu entries, WinFsp, NTFS fast scan
  (MFT) and Windows OCR. Linux uses `.desktop` "Open with" entries and a parallel
  directory walk instead.

## Testing in a VM

`scripts/arch-vm/vm.sh` boots a headless Arch Linux VM with QEMU/KVM. Install the
package there with `pacman -U`, then `scripts/arch-vm/gui-test.sh` drives Atlas under a
headless sway session with real keyboard input (create, rename, copy, clipboard, trash
and undo, hidden files, ZIP, Ctrl+K, restart, a 100 000-file folder, idle CPU and
memory) and saves screenshots. `scripts/linux-smoke.sh` covers X11 under Xvfb.
