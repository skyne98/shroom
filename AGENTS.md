# AGENTS.md

Rules for anyone (human or model) changing shroom.

## Motion: every state change animates

Every change the user can see is followed by a fast, clear, physics-based
animation, identical in the terminal and the window. No visible value jumps.

- **One system:** `crates/ui/src/motion.rs`. Springs are solved in closed form,
  so they behave the same at any frame rate. Both clients render what
  `Chat::frame` returns. Animate there, never in a client.
- **State stays instant.** Logic, tests and the agent never wait for an
  animation. The view derives each shown value from its target once per frame:
  `motions.tick(now)` first, then `follow(key, target, spring)` for values that
  glide, `start(key, from, to, spring)` for entrances, and `nudge(key, by)` when
  content jumps but must stay put on screen.
- **Interruptible:** a new target mid-motion retargets the spring and keeps its
  velocity. Never restart from zero, never queue animations.
- **Fast and clear:** use the presets. `GLIDE` for movement (scroll, sizes),
  `FADE` for things appearing, `POP` for popups (small overshoot), `COUNT` for
  numbers. Each reaches 90 % in under 120 ms and rests in under about 300 ms
  (`COUNT` under 600 ms). Add a preset only with a test that holds it to the
  same bounds.
- **Grid and pixels:** the terminal rounds positions to cells and fades colour
  toward the background. The window also gets sub-row offsets
  (`Frame::shift`/`above`) and the popup's `open` value.
- **Idle is free:** when nothing moves, `Chat::animating()` is false and clients
  drop to the slow tick (`frame_interval`).
- **Accessibility:** `SHROOM_REDUCED_MOTION=1` or `NO_MOTION=1` snaps everything.
  The end state must always be readable without motion.
- **Tested:** a new animated state gets a test in `chat.rs` `motion_tests`,
  driven through `Chat::frame_at` on a fake clock. It must check that the first
  frame starts at the old state, that the motion is continuous or monotonic,
  that it settles within its bound, and that it ends exactly at the new state.

Curves: zeron's catalog lives in `motion.rs` too (`Ease::FADE_IN`, `MENU_IN`,
`DIALOG_IN`, `RESIZE`, `HOVER_FADE`, ...) through `ease`/`enter`; springs stay
for scroll and sizes, which must keep their velocity when retargeted.

Animated today: transcript scroll (glide, keeps its place when content arrives
while scrolled up), new rows (fade in), session switch, relayout, rewind and the welcome giving
way to the chat (fade in), the window's sidebar (glides open and shut, ctrl+b),
popup (zeron's dialog-in), effort panel (menu-in, its thumb glides), hover plates (fade), picker highlight (fades onto the item),
notices (fade in), Material component states (`motion::Controls`: state layers,
ripples, shape morphs, elevation, focus ring, on the spec's springs and
curves), the composer's text field (`Controls::field`: label floats, indicator
thickens and colours, placeholder fades), ctx meter (counts), prompt box height (grows or shrinks a row
at a time), scrollbar thumb (follows the shown scroll), image previews (fade
in with their rows; real pictures follow the same fade and smooth scroll).

## Window: only to-spec Material 3 Expressive components

Everything in the gpui window is built from Material 3 Expressive components
that match the spec, not from divs styled to look like one.

- **Canonical values only.** Sizes, paddings, shapes, colours, type, elevation,
  state-layer opacities and motion come from the AndroidX Compose Material 3
  tokens (`androidx.compose.material3.tokens.*`) and the component's
  `*Defaults`, with material-web where Compose has no answer (focus ring,
  shadow formula). Name the token next to each value. Never eyeball, round or
  "tune" a value. If the spec has no answer, say so in a comment and choose the
  nearest spec value.
- **One module per component** (`crates/ui-gpui/src/m3_*.rs`), with tests that
  pin its token tables. Roles come from `m3::Scheme`, type from `m3::font`,
  shapes, spacing and elevation from `m3::{shape, space, level}`, and icons
  are Material Symbols (`m3::icon`).
- **States are the component's.** Hover, focus, press, ripple, shape morph,
  elevation and the focus ring go through `m3_state` (`Ui`, `interact`,
  `overlay`), and `motion::Controls` animates them. No `.hover()` or
  `.active()` styling, and no ad-hoc tints.
- **Accessible like the spec:** a 48 px minimum target, a role and a label,
  toggles with `aria_toggled`, and keyboard focus (F6 / Tab on components,
  Enter / Space activate, Escape back to the composer).
- **No look-alikes.** If the GUI needs something the spec lacks, compose it
  from spec components or ask first. When you replace a look-alike, delete it.
  Do not keep restyled leftovers.
- **Build on the kit in `m3_state`, not from scratch.** A new component is its
  token tables plus:
  - `Act` holding its `aria`, `on_click`, `keyshortcuts` and `enabled`,
    with `act_setters!()` in its `impl`;
  - `Control::new(key, act, ui, cx)` for its focus handle and focus state;
    `control.look(ui, spec)` for its animated look; `control.root()` or
    `.target(zoom)` (the 48 dp target); and `control.wire(el, ui, label,
    semantics)` for keys, role and label;
  - `Overlay::new(..)` (`.outline`, `.zoom`) for its state layer, ripple and
    focus ring;
  - its colour struct mixed with `mix_tones!(Colors { .. } keep { .. })`;
  - text set with `.type_style(m3::font::ROLE, zoom)` and `.one_line()`;
  - editable text drawn with `m3_text_field::input_row`.
- **Check it in the gallery.** `scripts/dev.sh ./target/debug/shroom-gpui
  --gallery` (`m3_gallery.rs`) shows every component at rest, focused
  (pinned with `Ui::with_pinned_focus`), selected, toggled and disabled, with
  hover and press from the pointer. Add each new component and state there,
  and screenshot it before you report.

## Other rules

- Keep `shroom-core` pure: no I/O, no pixels.
- A feature is one Action in core, its Effect handler in backend, and one arm
  per frontend.
- Run `cargo test --workspace` (gpui: `scripts/dev.sh cargo test`) and
  `cargo fmt --all` before committing.
