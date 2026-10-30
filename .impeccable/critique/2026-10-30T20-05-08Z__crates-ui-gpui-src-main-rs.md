---
target: shroom desktop app UI
total_score: 23
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 3
target_identity: "file:/home/fox/Project/shroom/crates/ui-gpui/src/main.rs"
target_fingerprint: "sha256:28bfd302a9539fe3bb22c4a591a6c02d115ff5c634ff7493c261599c0de062c2"
target_path: /home/fox/Project/shroom/crates/ui-gpui/src/main.rs
timestamp: 2026-10-30T20-05-08Z
slug: crates-ui-gpui-src-main-rs
---
# Critique: shroom desktop app (crates/ui-gpui, rendering crates/ui)

## Design Health Score

| # | Heuristic | Score | Key Issue |
|---|-----------|-------|-----------|
| 1 | Visibility of System Status | 3 | Live calls, timers, subagent dock are clear; the project folder the agent works in is never shown |
| 2 | Match System / Real World | 2 | ctx 0%, ↓5 tok, 0% cached, ✻max, ▣, "1/12", "thought · 681 words" |
| 3 | User Control and Freedom | 3 | esc stop, steering, rewind, delete confirm; a bad key means quit and restart |
| 4 | Consistency and Standards | 2 | No window title; no scrim under dialogs; a turn can show two summary rows |
| 5 | Error Prevention | 2 | Nothing shows or limits what the agent may run or change before it does |
| 6 | Recognition Rather Than Recall | 2 | Unlabeled model columns and glyphs; rewind hidden behind esc esc |
| 7 | Flexibility and Efficiency | 3 | Keyboard-first, fuzzy palette, steering, zoom; no side-by-side sessions |
| 8 | Aesthetic and Minimalist Design | 2 | Monospace prose at 110 columns, wall-of-text answers, 75% empty on ultrawide |
| 9 | Error Recovery | 3 | Errors name the fix; shown twice; jargon (host:port, env var, restart) |
| 10 | Help and Documentation | 1 | No first-run guidance, no help view; hints and palette only |
| **Total** | | **23/40** | **Acceptable** |

## Design Specificity Verdict

LLM assessment: category-interchangeable. The desktop app is the terminal UI in a window: one monospace grid, a "> type a message" prompt, a dim hint bar. Next to Claude Code, Codex or opencode it would not be told apart. Its specific touches: violet subagent dock, one-line summaries for finished work, spring motion. The product has no presence in its own window: no name, no mark, no window title. The new green mushroom logo appears nowhere inside.

Deterministic scan: not applicable. `impeccable detect` ran on crates/ui-gpui/src and crates/ui/src and returned [] because it reads web markup only; that is not a clean result. In its place, the theme was measured (WCAG ratios) from crates/ui/src/lib.rs NIGHT:
- text #e6e6ea on bg: 14.99. dim #9b9da4: 6.89. accent 9.03, code 9.96, agent 7.17, error 6.86.
- faint #5c6070 on bg: 2.99 (fails AA). On surface: 2.33. dim on the highlighted row (#3b4766): 3.40.
- faint is used as text in layout.rs:533 (summary meta "3 calls · 2.8s · thought"), popup.rs:121 (shortcut column) and layout.rs:955 (code line numbers).

## Overall Impression

Calm, fast and keyboard-complete, and the live view of the agent at work is the best part. The weak points: the moment before any work (empty first screen), the long answer that comes back (dense monospace wall), and the desktop window itself, which brings over only the terminal's limits. Biggest opportunity: let the desktop app read like a desktop app (proportional prose, real empty state, use of the side space) while keeping the terminal-grade tool rows.

## What's Working

- Work in progress is legible: the program header, per-call ✓/○ glyphs, right-aligned timers and the violet dock (07-live-subagents) show what runs and for how long. Finished work then folds to one line, so the chat ends calm.
- Keyboard discoverability: the hint bar lists only keys that work right now; ctrl+p lists every command with its shortcut and filters as you type.
- Restraint with meaning in colour: green answer/ok, amber running/code, violet agents, red errors; selected rows get a band and an accent bar.

## Priority Issues

- [P1] Empty first screen, no context. A new session (01-first-run) is a blank window with only "> type a message" at the bottom. It never says which folder the agent will work in, which model or key is active, or what to ask. The window has no title (niri reports title null), so overview and alt-tab show nothing.
  - Why: the user hands an agent a shell in an unnamed folder; first-timers stall; the high-stakes moment gets no reassurance.
  - Fix: an empty state with the shroom mark, "Working in ~/Project/shroom (git, main)", model + key status, 3 starter prompts, 4 keys; set the window title to "shroom — <session> — <folder>".
  - Command: $impeccable onboard
- [P1] Secondary text below WCAG AA. faint (#5c6070) is used for text at 2.99:1: summary-row meta, the palette's shortcut column, code line numbers (2.33:1 on surface). dim on the highlighted row is 3.40:1.
  - Why: the numbers users scan for (calls, time, shortcuts) are the hardest to read; fails low-vision users.
  - Fix: keep faint for rules and borders only; a text tier at ≥ #7a7e8c (4.61:1 on bg) or #8c8f9c (4.52:1 on surface); dim on highlighted rows becomes text.
  - Command: $impeccable audit
- [P1] Answers are walls of monospace text. Prose renders in JetBrains Mono at up to 110 columns, list items have no spacing, bold lead-ins are the only hierarchy, and many inline code chips (file names) highlight half the lines (02-transcript).
  - Why: answers are the product's output; long measure plus monospace slows reading and hides structure.
  - Fix: on desktop, proportional font for prose at about 70ch, mono for code, tool rows and tables; half-line gap between list items; real heading sizes; quieter inline code (no background, colour only).
  - Command: $impeccable typeset
- [P2] The desktop window only inherits terminal limits. On 3440 px the column uses about a quarter of the width (03-transcript-wide). Dialogs open over a fully bright transcript with no scrim (04, 05, 06). Clickable rows and hints have no visible affordance. The collapsed scrollbar reads as a vertical rule next to the last turn.
  - Why: content and dialog compete; space that could show sessions or agents sits empty; mouse users can't see what is clickable.
  - Fix: dim the transcript behind dialogs (scrim around 60%); above about 160 columns show a sessions/agents side panel; hover state on clickable rows; keep the collapsed scrollbar at the window edge, away from the text.
  - Command: $impeccable layout
- [P2] Jargon and unexplained glyphs. "ctx 0%", "↓5 tok · 0% cached", "✻max", ▣, ⟲, "commands 1/12". The model list has 7 unlabeled columns over 502 models (1M, $0.15/$0.60). Auth errors name host:port and an env var and say "restart".
  - Why: recall instead of recognition; first-timers can't decode the status line or pick a model.
  - Fix: a header row in the model list (context · price in/out per 1M · thinks · sees images); spell out "context 4% used"; show cache % only on hover or in details; let a key be entered in the app instead of restarting.
  - Command: $impeccable clarify

## Persona Red Flags

- Alex (power user): no files-changed or diff summary per turn ("3 calls" says nothing about edits). Rewind sits on esc esc, the same key that stops a run and clears the box, so one extra press changes meaning. No way to watch two sessions at once.
- Sam (accessibility): shroom adds no accessible names, so the painted grid has nothing for a screen reader. Secondary text at 2.99:1. Reduced motion only through SHROOM_REDUCED_MOTION, not the OS setting. Zoom (ctrl+= / ctrl+-) works: a plus.
- Jordan (first-timer): the blank first screen says nothing about what the agent can do or where. The status line is code (ctx, tok, cached, ✻high). The key error sends Jordan to edit an environment variable and restart.

## Minor Observations

- The block cursor sits on the placeholder's first letter ("▮ype a message").
- One turn shows two summary rows ("▸ Build contact sheet…" then "▸ thought · 681 words") because an image row splits the run.
- Errors appear twice: red in the transcript and amber in the status line, which also hides the key hints.
- The live state shows four timers at once (header, spinner, dock title, each call).
- The status line's right side repeats the first prompt as the session name ("Survey this workspace: map the…").

## Questions to Consider

- What if the desktop app kept monospace for the agent's work but set answers like a document?
- What would the ultrawide sides hold that a terminal can't: sessions, running agents, files changed this turn?
- Where does the mushroom live in the product, beyond the launcher icon?
