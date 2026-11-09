# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/jamieweavis/tabster/releases/tag/v0.1.0) - 2026-11-09

### Added

- *(viewer)* open new tabs on a preferred instrument
- *(shell)* floating player bar with sound, video and loop controls
- *(playback)* metronome sound, volume and pitch; tab kits and volume
- *(core)* glide the cursor into the next bar
- *(layout)* label each tab line with its open string
- *(core)* name tracks by their instrument
- *(shell)* drop the zoom buttons and remember the zoom
- *(shell)* always hide the YouTube video
- *(shell)* drop the library sidebar for a customisable tab table
- keep a hidden video playing as the audio source
- play the metronome over YouTube audio
- *(practice)* saved loops, speed trainer, count-in and practice history
- *(layout)* multi-bar rests, tie arcs, drum notation and cleaner rhythm
- *(viewer)* hear the tab through the synth when there's no video; metronome toggle
- *(shell)* YouTube as the playback clock, video pane and sync editor
- *(playback)* hear the tab with a SoundFont synth
- playedBars FFI for the viewer; wire viewer debug harness
- *(shell)* YouTube player component and smooth video clock
- *(shell)* tab viewer with display-list canvas, transport, loop and practice state
- rename setlists, refresh Recently Opened on open, persist settings via the core
- *(shell)* library window, import, sidebar, table and settings
- *(core)* parallel bulk import, config FFI, watched folders, player server
- *(spike)* YouTube IFrame player in WKWebView + player page
- *(formats)* GP7/8 .gp and GPIF importer
- *(layout)* tab layout engine, cursor and hit-testing
- *(formats)* GPX container and GP5 importers
- *(timeline,sync)* unroll repeats/jumps, tempo map and sync maps
- *(library)* SQLite catalogue, content-addressed store, parse cache and folder watcher

### Fixed

- *(shell)* keep the window wide enough for the library list and tab pane
- *(shell)* keep saved column widths and window size across launches
- *(shell)* synth clock implements time(at:)
- *(library)* exact float round-trip for stored sync points; add render_svg example
- *(model)* grace beats don't add to a voice's length

### Other

- refresh the README and drop PLAN.md
- release with release-plz and attach the macOS dmg
- *(shell)* reuse the YouTube player between tabs
- *(core)* don't reload the library on every tab open
- reorder imports and lockfile entries
- *(plan)* record build status
- Merge agent P: fuzzing and robustness
- fuzz the importers and harden against hostile tab files
- tolerate either order for same-content parallel imports
- build workflow; docs: README
- Merge agent K: YouTube clock and sync UX
- bundle the SoundFont with the app
- wire YouTube debug harness; document harnesses and Swift tests
- *(plan)* record YouTube embedding spike results
- *(shell)* TabViewer contract stub for Phase 2
- end-to-end import, layout, cursor and sync pipeline
- *(layout)* staff lines use secondary ink
- Merge agent D: tab layout engine
- *(library)* compute tab duration from the played timeline
- ignore local real-world tab files
- scaffold Rust core, Swift shell and Phase 1 contracts
