### Filesystem & listing

- [ ] Browse `/` — root dir lists without crash
- [ ] Browse `~` — home contents
- [ ] Browse `/usr/share/applications` — `.desktop` files show correct (FreeDesktop) icons
- [ ] Hidden files toggle works (`Ctrl+H` or equivalent)
- [ ] File sizes, dates, types match `ls -la` output
- [ ] Sorted by name / size / date / type — each column click works
- [ ] Folder-first / mixed / folder-last toggle works
- [ ] Symlinks listed as symlinks (not followed)

### Navigation

- [ ] Back / forward / up arrows work
- [ ] Address bar: type a path, hit Enter, navigates
- [ ] Breadcrumb segments are clickable
- [ ] Double-click a folder navigates into it
- [ ] Double-click a file opens with `xdg-open` (verifies `desktop_apps.rs`)
- [ ] `Ctrl+K` / `Ctrl+Shift+P` — command palette opens
- [ ] Goto bar (fuzzy file search) — type "down" finds Downloads
- [ ] Multi-pane / split view still works

### File operations (internal)

- [x] Copy a file → paste in another Atlas pane
- [ ] Cut → paste → file moves
- [ ] Rename (`F2`) — commits correctly, no `.tmp` left behind
- [ ] New folder (`Ctrl+Shift+N`)
- [ ] Move to Recycle Bin (`Delete`) → check `~/.local/share/Trash/files`
- [ ] Restore from Recycle Bin → file back at original path
- [ ] Delete permanently (`Shift+Delete`) → file gone from disk
- [ ] Batch select (`Ctrl+click`, `Shift+click`) — rubber-band selection works
- [ ] Copy 100 files — progress bar shows, all files land

### Clipboard (known broken — record current state)

- [x] Copy in Atlas → paste in Atlas
- [ ] Copy in Atlas → paste in Thunar → currently fails ("nothing to paste" — this is the bug we're about to fix)
- [ ] Copy in Thunar → paste in Atlas → does it work? (This one might work; records whether import-side is broken too)
- [ ] `wl-paste --list-types` after copying in Atlas → shows `text/plain` only, no `text/uri-list` (recorded above)

### Drag and drop

- [x] Drag from one Atlas pane to another
- [ ] Drag from Atlas to Thunar → fails (expected — GPUI-ce limitation)
- [ ] Drag from Thunar to Atlas → test, record result
- [ ] Drag from Atlas to browser file input → fails (expected)

### Preview & thumbnails

- [ ] Image thumbnails render (`jpg`, `png`, `webp`, `svg`)
- [ ] PDF preview (if PDF preview is wired — may need `pdftoppm`)
- [ ] Video thumbnails (if `ffmpeg` present)
- [ ] Text file preview
- [ ] Markdown preview
- [ ] Audio metadata preview
- [ ] Archive (`.zip`, `.tar.gz`) browse-in-place

### Context menu (test each item)

- [ ] Right-click a file → menu appears
- [ ] Open — works
- [ ] Open with — submenu populated from `desktop_apps` (Chromium, Firefox, etc.)
- [ ] Convert files — opens converter (may fail without `ffmpeg`/`imagemagick`/`pandoc`)
- [ ] Rename
- [ ] Copy / Cut / Paste
- [ ] Move to Recycle Bin / Delete permanently
- [ ] Properties — opens properties panel
- [ ] Right-click a folder — folder context menu
- [ ] Right-click a drive — "This PC Properties" item (on Linux this should be skipped or shown differently)
- [ ] Right-click empty space — background context menu

### Sidebar

- [ ] Places (Home, Desktop, Documents, Downloads, Pictures, Music, Videos) — navigate correctly
- [ ] Recycle Bin — navigate to Trash view
- [ ] Bookmarks — add one via drag or context menu, navigate to it
- [ ] Tags — create a tag, assign it to a file, open tag results
- [ ] Drives — shows all mounts, click navigates
- [x] Drive icons — hard-disk icon (FreeDesktop) (verified in screenshot)
- [ ] Drive icon on removable USB (if you have one) — should be `drive-removable-media`
- [ ] Sidebar collapse — `Ctrl+B` or button works
- [ ] Section headers — collapse/expand each
- [ ] Right-click section header — menu (hide, move, reset)

### Search

- [ ] `Ctrl+K` / Goto bar → type a filename you know exists → finds it
- [ ] Content search (if wired into goto) → search for a string in a `.txt` file → finds the file
- [ ] Large directory search performance — navigate to `~/Downloads` with 1000+ files → search stays snappy

### Convert (if you have the tools installed)

> _Note: If a converter says "missing engine," note the tool it wants._

- [ ] PNG → JPG (requires nothing — native)
- [ ] WAV → MP3 (requires `ffmpeg`)
- [ ] DOCX → PDF (requires `libreoffice` or `pandoc`)
- [ ] SVG → PNG (native)
- [ ] `.md` → `.html` (native)

### Disk analyzer / cleanup (Space Lens)

- [ ] Open Space Lens on `~` or `/tmp` → scan runs, tree builds
- [ ] Sizes match `du -sh` on the same folder (roughly)
- [ ] Cancel mid-scan → cancels cleanly
- [ ] Protected folders (`.ssh`, `.git`, `.gnupg`) — not shown

### Settings

- [ ] `Ctrl+,` opens settings
- [ ] Theme switch (Light / Dark / any theme) → applies
- [ ] Font size / family change → applies
- [ ] Language switch (if i18n wired) → applies
- [ ] Icon style setting — on Linux "Auto" should now route to FreeDesktop

### Vault (skip for now — unwired, decision pending)

- [ ] Vault is present in the code but not reachable from UI. Skip unless you decide to wire it.

### Window chrome

- [ ] Title bar renders correctly under Niri
- [ ] Resize window — layout reflows
- [ ] Move window between workspaces — state survives
- [ ] HiDPI: change Niri scale to 1.5x → text scales, icons don't blur
