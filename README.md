# shroom

```
shroom-tui   shroom-gpui        frontends: paint View, map input to Action
     \       /
    shroom-client              runtime: state + hooks + effect dispatch
          |
    shroom-core                state machine, Action/Effect/Event/View. no I/O, no pixels
          |
    shroom-backend             engine: executes effects (files today, daemon later)
          |
    shroom-ext                 wasm plugins on every action and every frame
```

| crate | role |
|---|---|
| shroom-core | domain, App reducer, view model. Pure. |
| shroom-client | Client: owns App, runs hooks, forwards effects to a Backend. |
| shroom-backend | the Backend: file store in ~/.local/share/shroom. |
| shroom-ext | extism host; Hooks impl that chains wasm plugins. |
| shroom-session | append-only JSONL log; `Hooks` impl that records every action and event. |
| shroom-tui | ratatui binary. |
| shroom-gpui | gpui binary (zeron fork: zeronsh/zui). |

A frontend only does two things: client.view() -> paint, input -> client.dispatch(action).
New feature = one Action in core, its Effect handler in backend, one arm per frontend paint.

## Extensions

Drop *.wasm into $XDG_DATA_HOME/shroom/extensions (or ~/.local/share/shroom/extensions).
Both hooks are optional, JSON in / JSON out, chained in load order:

- shroom_action(Action) -> Action: inspect or rewrite any action. Empty reply keeps it, null drops it.
- shroom_view(View) -> View: decorate every frame: rows, status, title.

## Install (Nix)

```sh
nix run github:skyne98/shroom            # terminal app (shroom)
nix run github:skyne98/shroom#desktop    # desktop app (shroom-gpui)
nix profile install github:skyne98/shroom
```

NixOS flake: add the input `shroom.url = "github:skyne98/shroom"` (with
`shroom.inputs.nixpkgs.follows = "nixpkgs"`) and put
`inputs.shroom.packages.${system}.default` in your packages, or use
`overlays.default`. The desktop app gets a launcher entry. API keys come from
the environment (`DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, ...).

## Run

```sh
cargo run -p shroom-tui
cargo run -p shroom-gpui      # NixOS: scripts/dev.sh cargo run -p shroom-gpui
```

`scripts/dev.sh` is NixOS-only: it puts fontconfig/freetype/xkbcommon on
`PKG_CONFIG_PATH` and `LD_LIBRARY_PATH`, which gpui needs to build and launch.
The terminal client needs no system libs.
