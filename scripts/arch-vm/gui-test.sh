#!/usr/bin/env bash
# GUI scenarios for an installed Atlas under a headless Wayland compositor (sway).
# Runs inside the VM: scripts/arch-vm/vm.sh ssh 'bash gui-test.sh [out-dir]'
# Every scenario drives Atlas with real key/mouse input and checks the result on
# disk (or in the compositor/clipboard); screenshots land in the output directory.
# Needs: sway grim wtype wl-clipboard jq imagemagick zip (pacman -S --needed ...).
set -uo pipefail
out=${1:-$HOME/gui-test}
work=$HOME/atlas-gui-work
rm -rf "$out" "$work" "$HOME/.config/atlas-gui-test" "$HOME/.local/share/Trash"
mkdir -p "$out" "$work"
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
export ATLAS_CONFIG_DIR=$HOME/.config/atlas-gui-test
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
# Software rendering inside the VM, as in scripts/linux-smoke.sh.
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe GPUI_BACKEND=${GPUI_BACKEND:-gl}
pass=0 fail=0
log() { printf '%s\n' "$*" | tee -a "$out/results.txt"; }
ok() { pass=$((pass + 1)); log "PASS  $1"; }
bad() { fail=$((fail + 1)); log "FAIL  $1${2:+ ($2)}"; }
shot() { grim "$out/$1.png" 2>/dev/null; }
# wtype uploads a fresh keymap on every call; the pause lets the client apply it
# before the first key, which is otherwise dropped.
key() { wtype -s 250 "$@"; sleep 0.3; }
# Poll a condition for up to $2 seconds (default 10).
until_true() {
  local cond=$1 limit=${2:-10} i
  for ((i = 0; i < limit * 4; i++)); do eval "$cond" && return 0; sleep 0.25; done
  return 1
}
# Ctrl+L opens the location finder prefilled with the current folder.
goto() { key -M ctrl l -m ctrl; key -M ctrl a -m ctrl; key "$1"; sleep 1; key -k Return; sleep 1; }
check() { local name=$1 cond=$2 limit=${3:-10}; if until_true "$cond" "$limit"; then ok "$name"; else bad "$name"; fi; shot "$(tr ' /' '__' <<<"$name")"; }

# Fixtures
mkdir -p "$work/start" "$work/rename" "$work/src" "$work/dst" "$work/trash" "$work/hidden" \
  "$work/deep/a/b/c/Needle Target Folder" "$work/clip-in" "$work/clip-src" "$work/content"
printf 'Atlas test\n' > "$work/start/readme.txt"
printf 'rename me\n' > "$work/rename/before.txt"
for i in $(seq -w 1 100); do head -c $((10#$i * 97)) /dev/urandom > "$work/src/file-$i.bin"; done
printf 'trash me\n' > "$work/trash/to-trash.txt"
printf 'secret\n' > "$work/hidden/.dotfile"; printf 'visible\n' > "$work/hidden/visible.txt"
printf 'copied through the clipboard\n' > "$work/clip-src/from-wl-copy.txt"
printf 'needle-content-31337\n' > "$work/content/match.txt"
magick -size 640x400 gradient:'#3060c0-#f0a040' "$work/start/picture.png" 2>/dev/null
(cd "$work/start" && printf 'inside zip\n' > zipped.txt && zip -q archive.zip zipped.txt && rm zipped.txt)

# Compositor
mkdir -p "$out/sway"
cat > "$out/sway/config" <<'EOF'
output HEADLESS-1 resolution 1366x768 position 0 0
default_border none
focus_follows_mouse no
EOF
sway -c "$out/sway/config" > "$out/sway.log" 2>&1 &
sway_pid=$!
until_true '[[ -n $(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v lock | head -n1) ]]' 15 || { bad "sway starts"; exit 1; }
export WAYLAND_DISPLAY=$(basename "$(ls "$XDG_RUNTIME_DIR"/wayland-* | grep -v lock | head -n1)")
export SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.* | head -n1)
unset DISPLAY

start_atlas() {
  atlas "$@" >> "$out/atlas.log" 2>&1 &
  atlas_pid=$!
  until_true "swaymsg -t get_tree | jq -e '.. | objects | select(.app_id? == \"atlas\")' >/dev/null" 30
}
# Close the window like a user does (xdg_toplevel close), so Atlas saves its session.
stop_atlas() {
  swaymsg -q '[app_id="atlas"] kill' 2>/dev/null
  until_true '! kill -0 "$atlas_pid" 2>/dev/null' 10 || kill "$atlas_pid" 2>/dev/null
  wait "$atlas_pid" 2>/dev/null
}
trap 'kill "$atlas_pid" "$sway_pid" 2>/dev/null' EXIT

# 1. Native Wayland window
if start_atlas "$work/start"; then
  shell=$(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "atlas") | .shell' | head -n1)
  [[ $shell == xdg_shell ]] && ok "native Wayland window (xdg_shell)" || bad "native Wayland window" "shell=$shell"
else
  bad "Atlas window appears"; cat "$out/atlas.log"; exit 1
fi
sleep 3; shot 01-first-frame
key -k Escape
check "first-run assistant dismissed" "grep -q '\"getting_started_seen\": *true' '$ATLAS_CONFIG_DIR/settings.json' 2>/dev/null"
sleep 2; shot 02-browser

# 2. Create a folder (Ctrl+Shift+N, type the name, Enter)
goto "$work/start"
key -M ctrl -M shift n -m shift -m ctrl; sleep 0.5
key "Created by Atlas"; key -k Return
check "create folder" "[[ -d '$work/start/Created by Atlas' ]]"

# 3. Rename (F2)
goto "$work/rename"; key -k Home; key -k F2; key -M ctrl a -m ctrl; key "after.txt"; key -k Return
check "rename file" "[[ -f '$work/rename/after.txt' && ! -e '$work/rename/before.txt' ]]"

# 4. Copy 100 files between folders
goto "$work/src"; key -M ctrl a -m ctrl; key -M ctrl c -m ctrl
goto "$work/dst"; key -M ctrl v -m ctrl
check "copy 100 files" "[[ \$(ls '$work/dst' | wc -l) -eq 100 ]] && cmp -s '$work/src/file-100.bin' '$work/dst/file-100.bin'" 30

# 5. Clipboard to other apps: Atlas copy is visible as text/uri-list
goto "$work/rename"; key -k Home; key -M ctrl c -m ctrl
check "clipboard exports text/uri-list" "wl-paste -t text/uri-list 2>/dev/null | grep -q 'file://$work/rename/after.txt'"
check "clipboard offers text to editors" "wl-paste -t text/plain 2>/dev/null | grep -q 'file://$work/rename/after.txt'"
check "clipboard marks a copy for GNOME" "wl-paste -t x-special/gnome-copied-files 2>/dev/null | head -n1 | grep -qx copy"

# 6. Clipboard from other apps: wl-copy a file, paste in Atlas
printf 'file://%s\r\n' "$work/clip-src/from-wl-copy.txt" | wl-copy -t text/uri-list
goto "$work/clip-in"; key -M ctrl v -m ctrl
check "paste file copied by another app" "[[ -f '$work/clip-in/from-wl-copy.txt' ]]"

# 7. Trash (Delete) and undo (Ctrl+Z)
goto "$work/trash"; key -k Home; key -k Delete
check "send to trash" "[[ ! -e '$work/trash/to-trash.txt' && -f \$HOME/.local/share/Trash/files/to-trash.txt ]]"
key -M ctrl z -m ctrl
check "undo restores from trash" "[[ -f '$work/trash/to-trash.txt' ]]"

# 8. Hidden files toggle (Ctrl+.)
goto "$work/hidden"; sleep 1; shot 08a-hidden-off
key -M ctrl . -m ctrl; sleep 1.5; shot 08b-hidden-on
if ! cmp -s "$out/08a-hidden-off.png" "$out/08b-hidden-on.png"; then ok "hidden files toggle changes the listing"; else bad "hidden files toggle"; fi
key -M ctrl . -m ctrl

# 9. ZIP opens as a folder and Quick Look reads a file inside it (copying out of
# archives is not implemented upstream yet). Judged from the screenshot.
goto "$work/start"; key -k Home; key -k Down; key -k Return; sleep 1.5
key -k Home; key -k space; sleep 1.5; shot 09-zip; key -k space
key -M alt -k Up -m alt; sleep 1  # leave the archive: Ctrl+L edits the path text there
log "LOOK  09-zip.png: archive.zip open, Quick Look shows \"inside zip\""

# 10. Ctrl+K jumps to a deep folder (probe: a new folder lands there)
goto "$work/start"; key -M ctrl k -m ctrl; sleep 0.5; key "Needle Target Folder"; sleep 3; shot 10-goto
key -k Return; sleep 1.5
key -M ctrl -M shift n -m shift -m ctrl; sleep 0.5; key "probe"; key -k Return
check "Ctrl+K navigates to a folder found by name" "[[ -d '$work/deep/a/b/c/Needle Target Folder/probe' ]]"

# 11. Search in subfolders by name (Ctrl+Shift+F); content search is covered by tests/linux_integration.rs. Judged from the screenshot.
goto "$work"; key -M ctrl -M shift f -m shift -m ctrl; sleep 0.5; key "match"; sleep 4; shot 11-subfolder-search
log "LOOK  11-subfolder-search.png: content/match.txt listed"
key -k Escape

# 12. Quick Look (Space) on image and text
goto "$work/start"; key -k Home; key -k End; sleep 0.3; key -k space; sleep 2; shot 12-quicklook; key -k space

# 13. Command palette, settings
key -M ctrl -M shift p -m shift -m ctrl; sleep 1; shot 13-palette; key -k Escape
key -M ctrl , -m ctrl; sleep 1.5; shot 14-settings; key -k Escape
kill -0 "$atlas_pid" && ok "palette and settings open without crash" || bad "palette and settings open without crash"

# 14. Idle cost (no constant redraws) and memory
sleep 5
ticks() { awk '{print $14 + $15}' "/proc/$atlas_pid/stat"; }
t0=$(ticks); sleep 30; t1=$(ticks)
cpu=$(awk -v d=$((t1 - t0)) -v hz="$(getconf CLK_TCK)" 'BEGIN { printf "%.2f", d / hz / 30 * 100 }')
rss=$(awk '/VmRSS/ {printf "%d", $2/1024}' "/proc/$atlas_pid/status")
log "MEASURE idle CPU over 30 s: ${cpu}%  RSS: ${rss} MiB"
awk -v c="$cpu" 'BEGIN { exit !(c < 1.0) }' && ok "idle CPU < 1%" || bad "idle CPU < 1%" "$cpu%"
((rss < 350)) && ok "idle RSS < 350 MiB" || bad "idle RSS < 350 MiB" "$rss MiB"

# 15. Restart restores the session (probe: a new folder lands in the last folder)
goto "$work/hidden"; sleep 1; stop_atlas
start_atlas && sleep 3
key -M ctrl -M shift n -m shift -m ctrl; sleep 0.5; key "after-restart"; key -k Return
check "restart restores the last folder" "[[ -d '$work/hidden/after-restart' ]]"

# 16. Large folder: 100 000 files appear within 2 s of navigating
mkdir -p "$work/big"; (cd "$work/big" && seq -w 1 100000 | sed 's/^/f/' | xargs touch)
goto "$work/start"; sleep 1
key -M ctrl l -m ctrl; key -M ctrl a -m ctrl; key "$work/big"; start=$(date +%s.%N); key -k Return
key -k End; key -k F2; key -M ctrl a -m ctrl; key "last-renamed"; key -k Return
if until_true "[[ -e '$work/big/last-renamed' ]]" 20; then
  took=$(awk -v s="$start" -v e="$(date +%s.%N)" 'BEGIN { printf "%.2f", e - s }')
  log "MEASURE 100k folder: open + select last + rename took ${took}s (includes ~2.8 s of scripted key delays)"
  ok "100 000-file folder browsable (last entry renamed)"
else bad "100 000-file folder browsable"; fi
shot 16-big-folder

stop_atlas
log "TOTAL pass=$pass fail=$fail"
((fail == 0))
