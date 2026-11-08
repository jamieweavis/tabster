# Tabster: Implementation Plan

Tabster is a local macOS guitar tab player. It imports and organises Guitar Pro tabs, renders them, and plays them in sync with a YouTube video. A Rust core does the work and a thin SwiftUI shell draws the UI, using the same architecture as `~/Developer/jaycast`.

---

## 0. Research findings that shape the plan

### Tab files
- On popular tab sites, downloading tabs is often a **paid feature**. Paid plans offer Guitar Pro, MIDI, MP3 and WAV.
- Tabs from popular tab sites, and from the common open-source exporters, are **GP7/8 `.gp`** files: a zip containing `Content/score.gpif`, which is XML.
- Collections often hold older formats too: **GP3/4/5** (binary) and **GP6 `.gpx`** (a BCFZ-compressed container around the same GPIF XML).
- Plan:
  - **v1** imports `.gp`, `.gpx` and `.gp5`.
  - **Phase 6** adds `.gp3`, `.gp4` and MusicXML.

### YouTube sync
Popular online tab players do **not** use a single offset.

- Its (undocumented) `api/video-points/{songId}/{revisionId}/list` endpoint returns, for each video, a `points: [f64]` array: **one video timestamp per played bar, with repeats unrolled**.
- GP8 and alphaTab use the same idea: `SyncPoint {barIndex, barOccurrence, barPosition, millisecondOffset}`.

So that model becomes ours:
- **Default:** a single `offset`, optionally with a `rate` multiplier. From these and the score's tempo map we derive a point for every bar. This is easy for the user to set: tap "sync here" on bar 1.
- **Advanced:** per-bar sync points, as popular online tab players use. Video time is interpolated piecewise-linearly between points, so recordings with drifting tempo stay locked. The user can nudge single bar markers.
- **Import:** points from a `.gp` file's GP8 `SyncPoint` automations, or a pasted JSON points array.

### Prior art
- **alphaTab** (TypeScript) is the reference implementation for parsing and layout. Read its `GpifParser`, `Gp3To5Importer` and `GpxFileSystem`.
- **PyGuitarPro docs** are the best prose spec for the GP3–5 binary format.
- **Rust `guitarpro` crate (scorelib):** GP3–5 is usable, GP6/7 is experimental.
  - Decision: write our own GPIF parser with `zip` and `quick-xml`.
  - Evaluate the crate for GP5 in Phase 1. Vendor or fork it if good enough, otherwise port from PyGuitarPro and alphaTab.

### YouTube embedding
Use `WKWebView` with the IFrame Player API.
- **Gotcha: error 153.** WKWebView sends no Referer. Load the player with `loadHTMLString(html, baseURL: https://<stable origin>)` and pass a matching `origin=` param plus `referrerpolicy="strict-origin-when-cross-origin"`.
- **Fallback for 153:** serve the player page from a tiny local HTTP server, run by the Rust core with `tiny_http`. It binds to loopback, but the page must be loaded as `http://localhost:<port>/player.html`.
- **Spike results** (`spikes/youtube/`; 23 real launches; raw data in `spikes/youtube/results/`):
  - **Primary works:** `loadHTMLString(html, baseURL: https://tabster.local/)`. The base URLs `https://www.youtube-nocookie.com/` and `http://localhost/` also work.
  - **`localhost` fallback works.**
  - **No base URL, or a `file://` page:** error 153.
  - **An IP-literal origin** (`127.0.0.1`), either as base URL or as server host: error **150**.
  - **Safari user agent:** makes no difference.
  - **The user's test video `cJRFMfztgBg`:** embeds in both working modes (duration 216 s).
  - **Embedding disabled, or a bogus id:** error 150 (`ready` fires first, with duration 0).
  - **Before calling `tabster.*`:** wait for the page's `apiReady` message.
- **Errors 101 and 150 (embedding disabled):** show "open in browser" and keep the tab playing on its own clock.

### Smooth cursor
- Measured in the spike:
  - `getCurrentTime()` is already interpolated and is smooth to about ±1.6 ms.
  - JS→Swift messages take 0.1–0.6 ms (p50).
  - Seeks take about 0.5 s to resume.
  - **Hazard:** for 0.5–0.8 s after a rate change, the reported time runs ahead and then jumps back by 120–300 ms. The `rate` event, not `setRate`'s return value, is the reliable signal.
- The page pushes `{t, perf}` samples every 100 ms.
- Between polls, extrapolate on a display link: `t = sample + (now − sampleWallClock) × rate`.
- Re-anchor on seek, pause and rate change. For 1 s after a seek, play or rate change, correct on every sample. Freeze while buffering. In replay, steady-state error stayed under 2 ms.
- Ignore drift smaller than 40 ms so the cursor doesn't jitter.

### Synth (optional)
- `rustysynth` (pure Rust, SF2) with a bundled small GM SoundFont.
- Audio output through `cpal`, or through an AVAudioEngine render callback.
- Useful for practising without a video, for count-in and for isolating a track.

---

## 1. Architecture

Copy the jaycast conventions exactly so the two projects feel the same.

```
tabster/
├── Cargo.toml              # root crate `tabster` (lib name tabster_core, crate-type lib+staticlib), workspace root
├── .cargo/config.toml      # MACOSX_DEPLOYMENT_TARGET = "26.0"
├── justfile                # core / bindings / build / app / dmg / run / test / lint / check
├── CLAUDE.md               # conventions (copied/adapted from jaycast)
├── tools/uniffi-bindgen/   # uniffi::uniffi_bindgen_main()
├── src/
│   ├── lib.rs              # uniffi::setup_scaffolding!(); exports `Core` object
│   ├── model/              # format-agnostic score model (Score, Track, MasterBar, Bar, Voice, Beat, Note, tempo map)
│   ├── formats/
│   │   ├── gpif.rs         # GPIF XML -> model (shared by .gp and .gpx)
│   │   ├── gp7.rs          # zip container
│   │   ├── gpx.rs          # BCFZ/BCFS container decompressor
│   │   └── gp5.rs          # GP3/4/5 binary (Phase 6: gp3/gp4)
│   ├── timeline/           # repeat/jump unrolling, tick<->seconds tempo map, played-bar index
│   ├── sync/               # SyncMap: offset/rate + per-bar points, tab-time <-> video-time
│   ├── layout/             # tab layout engine -> display list (glyph runs, lines, positions)
│   ├── library/            # SQLite catalogue, import pipeline, file watching, search
│   ├── playback/           # transport clock, loop/speed, (optional) rustysynth
│   ├── player_server.rs    # localhost HTTP server for YouTube player page (fallback for err 153)
│   └── config.rs           # TOML at ~/.config/tabster.toml, #[serde(default)], debounced atomic save
├── shell/                  # SwiftPM package, macOS 26, swift-tools 6.2
│   ├── Package.swift
│   └── Sources/
│       ├── Tabster/       # SwiftUI app, panes, Generated/tabster_core.swift
│       └── TabsterCoreFFI/  # shim.c + include/ (header + module.modulemap)
├── Resources/              # Info.plist, entitlements, SoundFont, youtube-player.html
└── testdata/               # small, redistributable .gp/.gpx/.gp5 fixtures (self-authored)
```

### The FFI boundary
- **UniFFI 0.32 in proc-macro mode.** No `.udl` file. A static lib is linked straight into SwiftPM, with the same justfile pipeline as jaycast.
- **A single `#[derive(uniffi::Object)] Core`.** It is constructed with the data directory, which is `~/Library/Application Support/Tabster`.
- **Rust does the heavy work:**
  - parsing and import
  - library queries
  - layout into a **display list**: flat arrays of positioned primitives
  - the timeline and sync maths
  - the playback clock
- **Swift only draws.** It renders the display list with SwiftUI `Canvas` or Core Graphics, hosts the WKWebView and forwards intent.
- **Hot-path calls must be cheap and allocation-light**, because they run every frame:
  - `cursor_at(video_time) -> CursorPos {bar, beat, x, y, page}`
  - `video_time_for(bar, beat)`
- **Callbacks:**
  - `LibraryObserver` (import progress, library changed) uses `#[uniffi::export(with_foreign)]`.
  - Bridge classes hop to the main actor, as in jaycast.

### Data model, in SQLite (`rusqlite` bundled, WAL, migrations via `user_version`)

```
tabs(id, path, file_hash, format, title, artist, album, tuning, track_count, bar_count,
     duration_s, imported_at, last_opened_at, favourite, rating, notes)
tracks(tab_id, idx, name, instrument, strings, tuning, capo)
tags(id, name) / tab_tags(tab_id, tag_id)
setlists(id, name) / setlist_items(setlist_id, tab_id, position)
videos(id, tab_id, youtube_id, label, feature, is_default)      -- many videos per tab
sync(video_id, mode, offset_s, rate, points_json)               -- mode: 'offset' | 'points'
practice(tab_id, loop_start_bar, loop_end_bar, speed, last_position_bar, preferred_track)
tabs_fts(title, artist, album)                                  -- FTS5 search
```

- **Files:** imported files are copied into `<data dir>/tabs/<hash>.<ext>`, so they are content-addressed and deduplicated. Users may instead choose "link in place".
- **Watched folders:** optionally watch a folder, such as `~/Downloads`, for new `.gp` files using the `notify` crate.
- **Parse cache:** parsed scores are cached as `bincode` blobs, keyed by `file_hash`, so tabs open instantly.

---

## 2. Phased delivery and agent parallelisation

Each phase lists **workstreams that can run as parallel agents**. They share only the interfaces frozen at the start of the phase. Work at the shared boundaries goes in the order: interfaces first (one agent), then fan out, then integrate (one agent).

Every agent works in its own **git worktree** (`isolation: "worktree"`) on its own module directory, and must pass `just lint && just test` before handing back. Integration is done by the lead session.

### Phase 0: Scaffold and contracts (single agent, sequential, about half a day)
This is the critical path. Everything else depends on it.

1. **Scaffold:**
   - `git init`
   - Cargo workspace, `tools/uniffi-bindgen`
   - SwiftPM shell
   - justfile, `.cargo/config.toml`, CLAUDE.md, `.gitignore`
   - CI workflow
   - These are copied and adapted from jaycast.
2. **Hello world:** `Core::new(data_dir)` and `core.version()` are shown in a SwiftUI window, and `just run` works.
3. **Freeze the contracts**, which are Rust types and traits with doc comments and `todo!()` bodies:
   - `model::Score` and its children, the tempo map and the repeat/jump markers
   - `formats::Importer` trait: `fn parse(bytes: &[u8]) -> Result<Score>`
   - `timeline::Timeline` API: unrolled played bars, `tick_to_seconds` and its inverse
   - `sync::SyncMap` API
   - `layout::DisplayList` schema: primitive enum, page/system/bar/beat hit-boxes
   - the UniFFI `Core` surface: method signatures returning stub data
4. **Self-authored fixtures:** a few tiny `.gp`, `.gpx` and `.gp5` files covering:
   - repeats and alternate endings
   - tempo changes
   - triplets
   - multiple tracks
   - bends, slides, hammer-ons and palm mutes

   Make them in TuxGuitar or MuseScore, or with alphaTab's exporter.

### Phase 1: Core engines (5 parallel agents)

| Agent | Scope | Depends on | Done when |
|---|---|---|---|
| **A: GPIF parser** | `formats/gpif.rs` and `gp7.rs`: zip, then quick-xml, then the model. Covers tracks, tunings, masterbars, time and key signatures, repeats, alternate endings, directions (D.S./D.C./Coda), tempo automations, GP8 sync points, rhythm and tuplets, and core note effects | contract | All `.gp` fixtures parse; golden snapshot tests (`insta`) of the model |
| **B: GPX and GP5** | `formats/gpx.rs` (BCFZ bit-stream decompressor, then the GPIF parser, using A's stub until A lands) and `formats/gp5.rs`. First evaluate the `guitarpro` crate for GP5 (half a day max), then fork or port | contract | Fixtures parse; GP5 and GP7 versions of the same song produce equivalent models |
| **C: Timeline and sync** | `timeline/`: repeat and jump unrolling into played-bar indices, the tempo map, tick-to-seconds conversion. `sync/`: offset/rate mode, points mode with piecewise-linear interpolation and binary search, extrapolation past the ends, generating points from offset and tempo, per-bar `points[]` import | model types only | Property tests (`proptest`) that the mapping is monotonic and invertible; `criterion` bench showing `cursor_at` under 1 µs |
| **D: Layout engine** | `layout/`: tab staff layout. Covers bar widths proportional to rhythmic content, line breaking into systems and pages for a given viewport width, fret numbers, rhythm stems and beams under the tab staff, technique glyphs, section and marker labels, and hit-boxes for click-to-seek. The output is a `DisplayList` | model types only | Snapshot tests of display lists; bench: relayout of a 300-bar tab under 5 ms |
| **E: Library and storage** | `library/` and `config.rs`: SQLite schema and migrations, the import pipeline (hash, copy, parse via the `Importer` trait, extract metadata, insert, fill the parse cache), FTS search, tags, setlists, video and sync CRUD, folder watching | contract | Tests import fixtures, run search, delete, re-import and dedupe, and test migrations |

- **What can run alongside what:**
  - A, C, D and E are fully parallel.
  - B starts immediately on the GP5 path and the GPX decompressor, then plugs into A's GPIF parser once it lands.
- **Integration (lead):**
  - Wire `Core` methods to the real modules.
  - Add a smoke test that runs import, open, layout and `cursor_at` from end to end.

### Phase 2: macOS shell, library and viewer (3 parallel agents, plus a Rust follow-up)

Before fanning out, freeze the UniFFI surface from Phase 1. All Swift agents work only in `shell/`.

| Agent | Scope |
|---|---|
| **F: Library UI** | `NavigationSplitView` with sidebar (All, Recent, Favourites, Tags, Setlists, Watched folders) and a table (title, artist, tuning, tracks, has-video badge). Drag-and-drop and `⌘O` import with a progress sheet. Search field backed by FTS. Context menu: tag, favourite, reveal in Finder, delete |
| **G: Tab renderer** | `TabCanvasView`: renders the `DisplayList` with `Canvas` or `CALayer`. Virtualises by page or system, so only visible systems are drawn. Track picker. Zoom. Relayout on resize, debounced and calling into Rust. Click to place the cursor. Shift-drag to set a loop range. Cursor and highlighted-beat overlay layer, kept separate from the static tab so it redraws cheaply |
| **H: App plumbing** | `AppModel` with `@Observable` per-area models, observer bridges, menu commands and keyboard shortcuts (space play/pause, ←/→ by bar, `[` `]` loop, `-`/`+` speed), settings window, window restoration, last-position resume |

- **Rust follow-up (agent I, parallel with F, G and H):** batch and async import for large folders.
  - Use `rayon` to parse files in parallel and stream progress through `LibraryObserver`.
  - Target: import 1,000 tabs in under 10 seconds.

### Phase 3: YouTube sync (2 parallel agents, then integration)

| Agent | Scope |
|---|---|
| **J: YouTube player component (Swift and web)** | `Resources/youtube-player.html` uses the IFrame API, with a JS↔Swift bridge through `WKScriptMessageHandler`. Commands: `load(id)`, `play`, `pause`, `seek(t)`, `setRate(r)`. Events: `state`, `time`, `error`, `rates`. Handle error 153 with `baseURL` and origin, falling back to the Rust `player_server` at `http://localhost:<port>` (an IP-literal origin gets error 150). Reference implementation: `spikes/youtube/Sources/YouTubeSpike/PlayerController.swift`. Handle 101 and 150 with a fallback UI. Resizable split or picture-in-picture layout |
| **K: Transport clock (Rust and Swift)** | `playback/`: one `Transport` is the source of truth, driven either by video time samples or by an internal clock when there is no video. It does display-link extrapolation with drift correction, applies the loop range to video seeks, and speed changes call `setPlaybackRate` (snapped to the rates YouTube supports). It exposes `tick(now) -> CursorPos` for the renderer |

**Integration and sync-editing UX (lead, or one agent after J and K land):**
- **Attach a video:** paste a URL or ID. Store several videos per tab, with labels (original, live, cover, lesson).
- **Quick sync:** play the video, press **"Tap bar 1"**, and the offset is set. Fine-tune with ±10 ms and ±100 ms nudges. Add a rate slider for "slightly fast or slow" recordings.
- **Sync editor (points mode):**
  - Show a bar ruler under the video.
  - "Tap along" mode: press a key on each downbeat while the video plays, to record points.
  - Drag individual markers.
  - Points between keyframes are regenerated from the tempo map.
  - Points saved with the file can be persisted back into a `.gp` file as GP8 sync points (Phase 6).
- **Import points:** from GP8 `SyncPoint` automations inside the file (handled by agent A), or by pasting a JSON points array.

### Phase 4: Practice features (3 parallel agents)

| Agent | Scope |
|---|---|
| **L: Synth playback** | `rustysynth` with a bundled GM SoundFont. MIDI events come from the timeline: tuning plus fret gives pitch; also velocity, dead and muted notes, simple bends. Output through `cpal`. Per-track mute/solo, metronome, count-in. Used when there is no video, or as a "tab audio" track mixed with the video |
| **M: Practice tools** | Loop with auto-repeat. Speed trainer that steps the tempo up by N% after each clean loop. Saved loop sections per tab. Practice history (time spent per tab, stored in SQLite) |
| **N: Rendering fidelity** | Optional standard-notation staff above the tab, slash rhythms, chord diagrams, lyrics lane, and more technique glyphs (vibrato, harmonics, tapping, let-ring, whammy) |

### Phase 5: Polish and release (2 parallel agents)

- **Agent O: packaging**
  - `just app` and `just dmg` with ad-hoc or development signing (`TABSTER_SIGN_IDENTITY`)
  - release-plz and Conventional Commits, as in jaycast
- **Agent P: performance and robustness**
  - fuzz the parsers (`cargo-fuzz` on the gp5, gpx and gpif importers)
  - Instruments pass on scrolling and cursor animation
  - memory check with a 1,000-tab library
  - handle corrupt or partial files gracefully

### Phase 6: Nice to have (each independent and parallel)
- GP3/GP4 and MusicXML import.
- Write sync points back into `.gp` files as GP8 `SyncPoint` automations, so they are portable to Guitar Pro and alphaTab.
- An optional metadata helper that fetches `api/video-points` for a song ID. This uses an undocumented API and may break or conflict with the site's terms, so it is off by default and the user triggers it each time.
- Export a setlist as a folder or zip, and sync the library to iCloud Drive by pointing the data directory at it.
- An iPad shell that reuses the same Rust core.

---

## 3. Parallelisation summary

```
Phase 0  [scaffold+contracts] ───────────────┐ (1 agent, critical path)
Phase 1  A gpif │ B gpx/gp5 │ C timeline/sync │ D layout │ E library   (5 parallel)
            └──────── integrate (lead) ───────┘
Phase 2  F library UI │ G tab renderer │ H app plumbing │ I bulk import (4 parallel)
Phase 3  J youtube component │ K transport clock → integrate + sync UX  (2 parallel)
Phase 4  L synth │ M practice │ N notation fidelity                   (3 parallel)
Phase 5  O packaging │ P perf/fuzz                                    (2 parallel)
```

**Rules for agents:**
1. Each agent owns specific directories and must not edit files outside them. Contract changes go back to the lead.
2. Each agent runs in its own worktree, delivers green `just lint && just test`, and summarises any API changes in its report.
3. Fixtures are shared and read-only during a phase. New fixtures go into `testdata/<agent>/`.
4. The lead merges in dependency order (A before B's GPX integration; C and D before G; K before the sync UX) and runs `just check` after each merge.

---

## 4. Key technical decisions

| Decision | Choice | Why |
|---|---|---|
| FFI | UniFFI 0.32 proc-macros with a static lib, linked through SwiftPM | Proven in jaycast; no Xcode project needed |
| Rendering split | Rust computes layout into a display list; Swift draws | Layout is the expensive, testable part. Drawing stays native and smooth |
| Sync model | Offset/rate by default, per-bar points when needed | Matches popular tab players and GP8. Offset alone breaks on tempo drift |
| Time authority | The video clock while a video plays; otherwise the internal clock | Avoids the cursor and the video disagreeing |
| Storage | SQLite (WAL, FTS5) plus content-addressed file store plus bincode parse cache | Fast search, instant open, no duplicates |
| Parsers | Custom GPIF (quick-xml) and GPX; GP5 from the `guitarpro` crate if it is good enough, otherwise ported | The Rust ecosystem is immature for GP6/7 |
| Synth | rustysynth plus cpal | Pure Rust, testable, no system dependencies |

## 5. Performance targets
- Open a cached 300-bar tab and show its first page in under 50 ms.
- Relayout on window resize in under 5 ms.
- `cursor_at` in under 1 µs. The cursor redraws at 120 Hz on ProMotion without dropping frames.
- Cursor and video stay within ±30 ms of each other during steady playback.
- Import 1,000 files in under 10 s (parallel parse).
- Idle CPU close to 0%. The display link runs only while playing.

## 6. Risks
- **YouTube embedding in WKWebView (error 153):** *resolved by the spike.* The `baseURL` mode works, with `localhost` as the fallback (see §0).
- **GPIF breadth:** it has many effects. Ship the core set and log unknown nodes rather than failing.
- **GPX decompression:** the BCFZ bit-reader is fiddly. Port it carefully from alphaTab's `GpxFileSystem`.
- **Rendering quality:** good tab engraving is a deep problem. Aim for "clear and readable", not Guitar Pro-grade engraving, in v1.
- **Licensing:**
  - Tabs remain the user's own files.
  - alphaTab is MPL-2.0, so porting logic from it is fine at file level. Note this in file headers.
  - Bundle only a SoundFont whose licence allows redistribution, such as GeneralUser GS or TimGM6mb.

---

## Status (2026-11-08)

Phases 0–5 are built and merged on `main`; `just check` is green: 207 Rust tests (plus `#[ignore]`d
real-tab and benchmark tests), 35 Swift tests, and the signed app build.

Not done yet:
- From Phase 6: GP3/GP4 and MusicXML import, writing sync points back into `.gp` files, setlist
  export, and the iPad shell.
- Known gaps:
  - Synth has no pitch bends or slides.
  - Lyrics aren't shown.
  - The video keeps playing past the tab's start and end.
  - The video/tab split size isn't remembered.
  - Keyboard and mouse input and the toolbar menus and sheets haven't been tested by driving them
    for real.
