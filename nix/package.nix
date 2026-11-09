# shroom: the terminal app (`shroom`, also `shroom-tui`) and the desktop app
# (`shroom-gpui`, with a launcher entry). `nix build` / `nix run` from the
# flake, or `callPackage ./nix/package.nix { }` from a NixOS config.
{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  makeDesktopItem,
  copyDesktopItems,
  fontconfig,
  freetype,
  libxkbcommon,
  libxcb,
  libx11,
  libxcursor,
  libxrandr,
  libxi,
  wayland,
  vulkan-loader,
  libglvnd,
  openssl,
  git,
  bash,
}:

let
  # gpui links these and dlopens the GPU and windowing ones at run time.
  runtimeLibs = [
    fontconfig
    freetype
    libxkbcommon
    libxcb
    libx11
    libxcursor
    libxrandr
    libxi
    wayland
    vulkan-loader
    libglvnd
  ];
in
rustPlatform.buildRustPackage {
  pname = "shroom";
  version = "0.1.0";

  # Only what cargo needs: never the (huge) target/ of a local checkout.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      # The window embeds the mark (sidebar, empty session).
      ./icons
    ];
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    # One per git repository in Cargo.lock.
    outputHashes = {
      "collections-0.1.0" = "sha256-k7PI2NGIagNke4jKt3LNd5y40XMnwSHZYA+wiAozrOk=";
      "xim-ctext-0.3.0" = "sha256-pRT4Sz1JU9ros47/7pmIW9kosWOGMOItcnNd+VrvnpE=";
      "zed-font-kit-0.14.1-zed" = "sha256-KXygi0olNQi5yM8eaJVykNDtbPMDjT+cWPBF8UrtXR4=";
      "zed-scap-0.0.8-zed" = "sha256-BihiQHlal/eRsktyf0GI3aSWsUCW7WcICMsC2Xvb7kw=";
      "gpui-base-0.5.2" = "sha256-0AY0tuhP8Jc6JXDitmXPReOvzWVnim2KY1rll9Qs2xA=";
      # One repo: zatex, zeron-ui, zeron-markdown, … (the desktop's text).
      "zatex-0.2.130" = "sha256-IBL2J3pvPKsH5IT3SGf1Ks4JeGhjMNnXJM/yxBPNWYk=";
    };
  };

  cargoBuildFlags = [
    "-p"
    "shroom-tui"
    "-p"
    "shroom-gpui"
  ];
  # The test suite runs in the dev shell (scripts/dev.sh cargo test); the
  # install check below proves both binaries start.
  doCheck = false;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    copyDesktopItems
  ];
  # The cargo setup hook exports OPENSSL_NO_VENDOR=1, so crates that want
  # openssl (zeron's, extism's) link the system one instead of vendoring.
  buildInputs = runtimeLibs ++ [ openssl ];

  desktopItems = [
    (makeDesktopItem {
      name = "shroom";
      desktopName = "Shroom";
      genericName = "Coding agent";
      comment = "Chat with a coding agent that works in your project";
      exec = "shroom-gpui";
      icon = "shroom";
      categories = [
        "Development"
        "Utility"
      ];
      startupWMClass = "shroom";
    })
  ];

  postInstall = ''
    ln -s shroom-tui $out/bin/shroom
    for size in 32 48 64 128 256; do
      install -Dm644 ${./icons}/shroom-$size.png \
        "$out/share/icons/hicolor/$size"x"$size/apps/shroom.png"
    done
  '';

  # Rewind and worktrees run git, the sh tool runs bash: present even when
  # launched from a bare desktop session. The GPU driver comes from
  # /run/opengl-driver on NixOS.
  postFixup = ''
    wrapProgram $out/bin/shroom-tui \
      --suffix PATH : ${
        lib.makeBinPath [
          git
          bash
        ]
      }
    wrapProgram $out/bin/shroom-gpui \
      --suffix PATH : ${
        lib.makeBinPath [
          git
          bash
        ]
      } \
      --prefix LD_LIBRARY_PATH : "/run/opengl-driver/lib:${lib.makeLibraryPath runtimeLibs}"
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    $out/bin/shroom --version | grep -Fx "shroom 0.1.0"
    $out/bin/shroom-gpui --version | grep -Fx "shroom 0.1.0"
  '';

  meta = {
    description = "Coding agent harness with a terminal and a desktop app";
    homepage = "https://github.com/skyne98/shroom";
    mainProgram = "shroom";
    platforms = lib.platforms.linux;
  };
}
