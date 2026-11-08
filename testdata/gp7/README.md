# GPIF fixtures

Hand-written `score.gpif` documents, self-authored for Tabster's tests (no third-party content).
Tests zip them into `.gp` containers on the fly (`Content/score.gpif`).

- `showcase.gpif` — GP7/8 layout. Metadata; lead guitar (standard tuning, capo 2) and drop-D bass;
  4/4 and 6/8; tempo changes (including a dotted-quarter unit); `|: :|` ×2 with 1st/2nd endings;
  sections; Segno / To Coda / D.S. al Coda / Coda; triplets, dotted notes, rests, ties, a grace
  note; hammer-on, slides, bend, palm mute, dead note, harmonics, vibrato, let ring, accents,
  ghost note, trill, tapping, brush, slap/pop, fade in, tremolo picking, whammy; two GP8
  `SyncPoint`s with backing-track padding; a missing (`-1`) bar.
- `legacy.gpif` — GP6 layout: track-level properties (misspelt `Tunning`), `GeneralMidi`,
  element/variation drums, a two-staff piano without tuning, an anacrusis, voice placeholders,
  and deliberately dangling references.
