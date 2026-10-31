#!/usr/bin/env bash
# NixOS only: runs a command in shell.nix, which has gpui's native libraries
# (fontconfig, xkbcommon, wayland, vulkan, …) and zeron-ui's (webkitgtk,
# openssl, alsa, dbus, libclang).
#
#   scripts/dev.sh cargo run -p shroom-gpui
#   scripts/dev.sh cargo test
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec nix-shell "$here/shell.nix" --run "$(printf '%q ' "$@")"
