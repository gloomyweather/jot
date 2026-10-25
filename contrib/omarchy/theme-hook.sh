#!/usr/bin/env bash
# Omarchy theme-set hook: recolor the jot icon to the theme's foreground and
# accent, so it stays visible on dark and light themes alike.
# install.sh puts it in ~/.config/omarchy/hooks/theme-set.d/jot.

ICON="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/jot.svg"
COLORS="$HOME/.local/state/omarchy/current/theme/colors.toml"

[[ -f "$ICON" && -f "$COLORS" ]] || exit 0

color() { sed -nE "s/^$1[[:space:]]*=[[:space:]]*\"(#[0-9a-fA-F]{6})\".*/\1/p" "$COLORS" | head -n1; }
fg="$(color foreground)"
accent="$(color accent)"

[[ -n "$fg" ]] && sed -i -E "s/stroke=\"#[0-9a-fA-F]{3,8}\"/stroke=\"$fg\"/" "$ICON"
[[ -n "$accent" ]] && sed -i -E "s/fill=\"#[0-9a-fA-F]{3,8}\"/fill=\"$accent\"/" "$ICON"
# Bump the mtime of the theme dir so icon caches notice the change.
touch "$(dirname "$ICON")/../.."
gtk-update-icon-cache -q -t -f "$(dirname "$ICON")/../.." >/dev/null 2>&1 || true
