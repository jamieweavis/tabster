# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Tabster is a local macOS guitar tab player: it imports Guitar Pro tabs (`.gp`, `.gpx`, `.gp5`) into a
library, renders them, and plays them in sync with a YouTube video (per-tab offset or per-bar sync
points). A Rust core does all the work; a thin SwiftUI shell draws the UI. Requires macOS 26+, Rust
stable, Xcode's toolchain (the IDE is never opened) and `just`.

## Commands

```sh
just run              # build core + bindings + shell, bundle build/Tabster.app, (re)launch it
just test             # cargo test --workspace
just test-swift       # swift test for the shell's pure-logic TabsterKit target (VideoClock …)
just lint             # cargo fmt --check + clippy with -D warnings
just check            # everything CI runs: lint, test, app build
just logs [info]      # stream the app's os_log output (subsystem com.jamieweavis.tabster)
CONFIG=release just app # optimised universal (arm64 + x86_64) build; `just dmg` packages it
```

- Single test: `cargo test <name_substring>` (e.g. `cargo test timeline::`).
- Snapshot tests use `insta`: review with `cargo insta review` (or `INSTA_UPDATE=always`).
- `TABSTER_DATA_DIR` / `TABSTER_CONFIG` override the data dir / config path (handy for testing).
- `TABSTER_SIGN_IDENTITY="-"` signs ad hoc (default "Apple Development").
- DEBUG harnesses (launch through LaunchServices — `open -n -W --env K=V … build/Tabster.app`; running the
  binary directly can leave the app without a window):
  - `TABSTER_IMPORT=a:b`, `TABSTER_SNAPSHOT=<dir>`, `TABSTER_SELECT=<row>`: library window (`DebugHooks.swift`)
  - `TABSTER_VIEW=<tab file>` (+ `_TRACK`, `_PLAY`, `_SEEK`, `_LOOP`, `_APPEARANCE`, `_VIEW_SNAPSHOT=<png>` …):
    viewer only (`Viewer/ViewerHarness.swift`)
  - `TABSTER_YT_HARNESS=<video id>` (+ `TABSTER_YT_PAGE=localhost`): YouTube player + clock log (`Video/`)
- `cargo run --example render_svg -- <tab file> <track> <out.svg>` renders a track's layout to SVG.
- `cargo run --example yt_search -- "<artist>" "<title>" <m:ss> [response.json]` searches YouTube (or
  ranks a saved `youtubei/v1/search` response) and prints how the matcher scores each result.
- `UNIVERSAL=1` (default for release) builds the core for both `aarch64-` and `x86_64-apple-darwin`,
  `lipo`s them into `build/universal/<config>/`, and passes `--arch arm64 --arch x86_64` to SwiftPM
  (which then uses Xcode's build system and warns that x86_64 is deprecated: harmless, minos stays 26.0).
  `cargo test --target x86_64-apple-darwin` runs the suite under Rosetta.
- `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET=26.0` so bundled C code (SQLite) matches the app.

## Architecture

**Rust core, thin Swift shell.** Anything that decides or does something belongs in `src/`. The shell
(`shell/Sources/Tabster/`) draws UI and forwards intent. Layout is computed in Rust as a display list
(`layout::DisplayList`) the shell paints verbatim; Swift makes no layout decisions about the tab.

**Modules** (`src/`):
- `model` — format-agnostic `Score`. Ticks: 960 per quarter. Strings numbered from 1 = highest.
  Every `Track::bars` is index-aligned with `Score::master_bars`. Tempo in quarter-notes/minute.
- `formats` — importers (`gp7` zip → `gpif` XML; `gpx` BCFZ container → `gpif`; `gp5` binary).
- `timeline` — repeats/endings/jumps unrolled into *played bars*; ticks ↔ score seconds.
- `sync` — `SyncSpec` (offset+stretch, or one video time per played bar) → `SyncMap`
  (piecewise-linear score seconds ↔ video seconds).
- `layout` — track → `DisplayList`; `cursor()` and `hit_test()` against it.
- `library` — SQLite catalogue, content-addressed file store, parse cache, tags, setlists, videos,
  video searches.
- `video_search` — finding a tab's YouTube video: `client` (keyless `youtubei/v1/search`, HTML
  fallback), `parse` (walks the JSON for `videoRenderer`s), `matching` (scores title, artist, channel,
  version and length against the tab; `High` confidence is attached without asking), `finder` (one
  background thread, throttled, rate-limit backoff, pending searches survive restarts).
- `lib.rs` — the FFI surface: `Core` (library) and `TabSession` (one open tab).

**Time axes:** played ticks → (tempo map) → score seconds → (`SyncMap`) → video seconds. While a
video plays, the video clock is the source of truth; the shell asks `TabSession::position_at_video`
every frame.

**Build pipeline** (see `justfile`): `cargo build -p tabster` produces `libtabster_core.a` →
`tools/uniffi-bindgen` generates Swift bindings → header/modulemap to
`shell/Sources/TabsterCoreFFI/include/`, `tabster_core.swift` to `shell/Sources/Tabster/Generated/`
(both gitignored) → SwiftPM builds the shell linking the `.a` → bundle assembled from
`Resources/Info.plist` + `Resources/Bundle/` and codesigned. Frameworks Rust crates need must be added
to `linkerSettings` in `shell/Package.swift`.

Tests never reach YouTube: build `Core::with_searcher` with a fake `Searcher` (see
`tests/video_search.rs`), not `Core::with_paths`, whenever tabs get imported.

**Shell side:** `AppModel` owns the `Core`. Observer bridges receive Rust callbacks on arbitrary
threads and must hop to `DispatchQueue.main.async` before `MainActor.assumeIsolated`. Blocking calls
(import) run in `Task.detached`.

## Conventions

- Use `parking_lot` locks. Every `unsafe` block needs a `// SAFETY:` comment.
- Importers never panic on bad input: return `ParseError`. Unknown GPIF nodes are skipped, not fatal.
- Hot-path calls (`position_at_*`, `layout::cursor`) must not allocate beyond their return value.
- Test fixtures live in `testdata/` and must be self-authored or redistributable — never commit tabs
  downloaded from tab sites.
- Conventional Commits.
