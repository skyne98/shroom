# GUI reference: AntiWebUI (Material 3 Expressive)

> What to borrow from [AntiWebUI](https://github.com/bea4dev/AntiWebUI) when
> shaping `shroom-gpui`, and what to leave alone. Study it as inspiration,
> keep **gpui** as the engine.

Status: **reference** (not a dependency). Recorded 2026-11-13.

---

## What it is

A native Rust GPU UI toolkit for building shells (bars, docks, panels). It is
the renderer behind `shoji-bar-4` (the desktop shell in the NixOS config),
and its M3 Expressive variant crate is `anti-webui-md3e`.

- **Philosophy:** "Anti"-WebUI = a GPU-native replacement for a web UI, not a
  browser/webview. It draws every pixel itself.
- **Renderer:** `wgpu 30` (Vulkan), `naga` for WGSL, `resvg`/`image` for
  raster, `kurbo` + `bytemuck` for geometry/GPU buffers.
- **Text:** `parley 0.12` (+ `complex-scripts`) + `swash` + `icu_properties`.
- **Window/display:** Wayland (`wayland-client`, `wayland-protocols`, `-wlr`,
  `wayland-backend`, `wayland-scanner`) + `xkbcommon` input +
  `raw-window-handle`; platform crates `anti-webui-wayland`,
  `anti-webui-wayland-shell`, `anti-webui-x11`.

---

## Why it's a good reference

### Design language (borrow freely)
Full **Material 3 Expressive** system, decoupled from the framework:
- `shapes.rs` (~35 KB) — the expressive corner/shape token set.
- `motion.rs` + springs — M3 Expressive motion (the spatial / focus spring
  split is exactly what our `crates/ui/src/motion.rs` presets aim at).
- `typography.rs`, `density.rs`, `color.rs` — type/compactness/tone scales.
- `dynamic/` — a complete **HCT / CAM16** dynamic-color engine (`hct`,
  `palette`, `quantize`, `score`, `solver`, `contrast`, `temperature`,
  `scheme`). This is the good part to lift for a theme from a source colour.

### Component vocabulary (borrow the naming, not the code)
`anti-webui-md3e/src/components` has 34 M3 components — good naming and
composition ideas for our view layer:
`app_bar, badge, button, button_group, card, carousel, checkbox, chip,
date_picker, fab, fab_menu, icon, icon_button, image, list, loading, menu,
modal, navigation, progress, search, shape, slider, snackbar, sortable,
split_button, surface, switch, tabs, text_field, time_picker, toolbar, tooltip`.
Base primitives live in a lower `anti-webui-widgets` crate; icons in
`anti-webui-symbols-outlined`; plus `markdown`/`code`/`math`/`pdf`/`video`
extension crates.

---

## What to leave alone

- **Do not depend on it.** It is `0.1.0`, unpublished, single author
  (`bea4dev`), pinned to a git `rev` in the NixOS flake. Documented leaks
  (the NixOS service caps it at `MemoryMax = 2G` after an upstream
  `8.6 GB / 15 h` leak).
- **One rendering stack.** `shroom-gpui` is gpui-based; importing an
  AntiWebUI renderer means maintaining two. Recreate any M3 Expressive ideas
  in gpui terms.
- **Extensiveness ≠ reliability.** Its breadth is surface area, not proof.

---

## Rule of thumb

AntiWebUI = **inspiration** for design language, colour, motion, and how to
structure components. gpui = **the engine.** If a specific M3 Expressive
shape/motion/colour behaviour genuinely improves `shroom-gpui`, port the
idea — not the crate.
