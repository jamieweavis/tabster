# Tabster

A local, private guitar tab player for macOS: import your Guitar Pro tabs, read them in a clean
interactive view, and play along in sync with the song's YouTube video — or hear the tab itself
through a built-in synth.

- **Library** — import `.gp` (Guitar Pro 7/8), `.gpx` (GP6) and `.gp5` files or whole folders;
  search, favourites, tags, setlists, watched folders that import new tabs automatically.
- **Viewer** — fast tab rendering with rhythm, techniques, sections, multi-bar rests and drums;
  smooth playback cursor, click to seek, loops, speed control, track switching, zoom.
- **YouTube sync** — attach any embeddable YouTube video to a tab. Sync it with one tap on bar 1 plus
  fine nudges, a stretch for recordings that drift uniformly, or per-bar sync points (tap along,
  drag markers, import Guitar Pro 8 sync points or pasted per-bar points).
- **Tab audio** — hear the tab (GeneralUser GS SoundFont) with a metronome when there's no video.

Everything stays on your Mac: tabs are stored in `~/Library/Application Support/Tabster`, settings in
`~/.config/tabster.toml`.

## Build

Requires macOS 26+, Rust stable, Xcode 26's command-line tools and [`just`](https://github.com/casey/just).

```sh
just run      # build the Rust core, Swift bindings and app, then launch build/Tabster.app
just check    # lint + Rust tests + Swift tests + app build (what CI runs)
CONFIG=release just dmg   # optimised, packaged build/Tabster-<version>-macos-arm64.dmg
```

The first `just app` downloads the GeneralUser GS SoundFont (~31 MB, SHA-256 verified; licence in
`Resources/Bundle/SOUNDFONT-LICENSE.txt`). The app is signed with "Apple Development" by default;
`TABSTER_SIGN_IDENTITY="-"` signs ad hoc.

## Architecture

A Rust core (`src/`) does everything — parsing, timeline, sync maths, layout, library, synth — and is
exposed to a thin SwiftUI shell (`shell/`) through UniFFI. See [CLAUDE.md](CLAUDE.md) for the details
and [PLAN.md](PLAN.md) for the roadmap.

## A note on tabs

Tabster doesn't download anything for you: bring your own tab files. Respect the licences of the tabs
and videos you use.
