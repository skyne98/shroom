# The dev shell for scripts/dev.sh: gpui's native libraries, plus what
# zeron-ui (the desktop's markdown and input) builds against.
{ pkgs ? import <nixpkgs> { } }:
let
  libs = with pkgs; [
    fontconfig
    freetype
    libxkbcommon
    xorg.libxcb
    xorg.libX11
    xorg.libXcursor
    xorg.libXrandr
    xorg.libXi
    wayland
    vulkan-loader
    libglvnd
    openssl
    alsa-lib
    dbus
    zlib
    glib
    gtk3
    webkitgtk_4_1
    libsoup_3
    json-glib
  ];
in
pkgs.mkShell {
  packages = with pkgs; [ pkg-config meson ninja cmake clang ];
  buildInputs = libs;
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  # rustc forwards LIBRARY_PATH to the linker; the binary finds them at run.
  LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
}
