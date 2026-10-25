#!/usr/bin/env bash
# Remove everything install.sh added. Your notes stay where they are.

set -euo pipefail

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
HYPR_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/hypr"
BEGIN="-- >>> jot >>>"
END="-- <<< jot <<<"

# Ask jot where the notes are before its binary is gone.
NOTES="$(jot dir 2>/dev/null || echo ~/Notes)"

rm -f "$HOME/.local/bin/jot" \
  "$DATA_DIR/icons/hicolor/scalable/apps/jot.svg" \
  "$DATA_DIR/applications/jot.desktop" \
  "$DATA_DIR/applications/jot-new.desktop" \
  "${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/hooks/theme-set.d/jot"

for f in "$HYPR_DIR/hyprland.lua" "$HYPR_DIR/bindings.lua"; do
  [[ -f "$f" ]] || continue
  tmp="$(mktemp)"
  awk -v b="$BEGIN" -v e="$END" '$0 == b { skip = 1; next } $0 == e { skip = 0; next } !skip { print }' "$f" >"$tmp"
  cat "$tmp" >"$f"
  rm -f "$tmp"
done

update-desktop-database -q "$DATA_DIR/applications" >/dev/null 2>&1 || true
command -v hyprctl >/dev/null && hyprctl reload >/dev/null 2>&1 || true
echo ":: jot removed (notes kept in $NOTES)"
