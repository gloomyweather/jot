#!/usr/bin/env bash
# Install jot for the current user and hook it into Omarchy:
# binary, icon, launcher entries, floating window rules and two hotkeys.
# Safe to run again: everything is replaced, nothing is duplicated.

set -euo pipefail

usage() {
  cat <<'EOF'
usage: contrib/omarchy/install.sh [--yes] [--no-build] [--no-hypr]

  --yes       change the Hyprland config without asking
  --no-build  install target/release/jot as is, without running cargo
  --no-hypr   do not touch the Hyprland config
EOF
}

YES=0
BUILD=1
HYPR=1
for arg in "$@"; do
  case "$arg" in
    -y | --yes) YES=1 ;;
    --no-build) BUILD=0 ;;
    --no-hypr) HYPR=0 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN_DIR="$HOME/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
HYPR_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/hypr"
OMARCHY_HOOKS="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/hooks"
BEGIN="-- >>> jot >>>"
END="-- <<< jot <<<"

say() { printf '\033[1m::\033[0m %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# Replace the block between the markers in $1 with $2 (or append it).
# An empty $2 just removes the block.
put_block() {
  local file="$1" body="$2" tmp
  tmp="$(mktemp)"
  if [[ -f "$file" ]]; then
    awk -v b="$BEGIN" -v e="$END" '
      $0 == b { skip = 1; next }
      $0 == e { skip = 0; next }
      !skip { print }
    ' "$file" >"$tmp"
    # Drop trailing blank lines left behind by a previous block.
    sed -i -e ':a' -e '/^\n*$/{$d;N;ba' -e '}' "$tmp"
  fi
  if [[ -n "$body" ]]; then
    printf '\n%s\n%s\n%s\n' "$BEGIN" "$body" "$END" >>"$tmp"
  fi
  # Write in place so symlinks and permissions survive.
  cat "$tmp" >"$file"
  rm -f "$tmp"
}

confirm() {
  ((YES)) && return 0
  [[ -t 0 ]] || return 1
  if have gum; then
    gum confirm "$1"
  else
    local reply
    read -r -p "$1 [y/N] " reply
    [[ "$reply" == [yY]* ]]
  fi
}

# 1. Binary.
if ((BUILD)); then
  say "building release binary"
  cargo build --release --manifest-path "$REPO/Cargo.toml"
fi
install -Dm755 "$REPO/target/release/jot" "$BIN_DIR/jot"
say "installed $BIN_DIR/jot"

# 2. Icon, recolored to the current theme on every theme switch.
install -Dm644 "$REPO/assets/jot.svg" "$DATA_DIR/icons/hicolor/scalable/apps/jot.svg"
if [[ -d "$OMARCHY_HOOKS" ]]; then
  install -Dm755 "$REPO/contrib/omarchy/theme-hook.sh" "$OMARCHY_HOOKS/theme-set.d/jot"
  bash "$OMARCHY_HOOKS/theme-set.d/jot"
  say "installed theme hook, the icon follows the Omarchy theme"
fi
gtk-update-icon-cache -q -t -f "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true

# 3. Launcher entries (Super+Alt+Space, search "notes").
mkdir -p "$DATA_DIR/applications"
cat >"$DATA_DIR/applications/jot.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Notes
GenericName=Quick notes
Comment=Quick markdown notes in the terminal
Exec=omarchy-launch-or-focus-tui jot
Icon=jot
Terminal=false
Categories=Utility;TextEditor;
Keywords=notes;jot;memo;markdown;заметки;
StartupWMClass=org.omarchy.jot
EOF
cat >"$DATA_DIR/applications/jot-new.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Quick Note
GenericName=New note
Comment=Write a note right away
Exec=omarchy-launch-or-focus-tui --app-id=org.omarchy.jotnew jot new
Icon=jot
Terminal=false
Categories=Utility;TextEditor;
Keywords=note;new;capture;заметка;
StartupWMClass=org.omarchy.jotnew
EOF
update-desktop-database -q "$DATA_DIR/applications" >/dev/null 2>&1 || true
say "installed launcher entries"

# 4. Hyprland: floating windows and hotkeys.
((HYPR)) || exit 0
if [[ ! -f "$HYPR_DIR/hyprland.lua" || ! -f "$HYPR_DIR/bindings.lua" ]]; then
  say "no Omarchy Lua Hyprland config in $HYPR_DIR, skipping"
  exit 0
fi
if ! confirm "add floating window rules, Super+Alt+N and Super+Ctrl+Alt+N to $HYPR_DIR?"; then
  say "skipped the Hyprland setup (run with --yes to apply it)"
  exit 0
fi

for f in hyprland.lua bindings.lua; do
  [[ -e "$HYPR_DIR/$f.bak.jot" ]] || cp "$HYPR_DIR/$f" "$HYPR_DIR/$f.bak.jot"
done

put_block "$HYPR_DIR/hyprland.lua" \
  'o.window("^org\\.omarchy\\.jot$", { float = true, center = true, size = { 1100, 700 } })
o.window("^org\\.omarchy\\.jotnew$", { float = true, center = true, size = { 760, 440 } })'
put_block "$HYPR_DIR/bindings.lua" \
  'o.bind("SUPER + ALT + N", "Notes", { tui = "jot", focus = true })
o.bind("SUPER + CTRL + ALT + N", "Quick note", "omarchy-launch-or-focus-tui --app-id=org.omarchy.jotnew jot new")'
say "updated hyprland.lua and bindings.lua (backups: *.bak.jot)"

if have hyprctl; then
  hyprctl reload >/dev/null 2>&1 || true
fi
say "done. Super+Alt+N opens your notes, Super+Ctrl+Alt+N writes a new one"
