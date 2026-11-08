use super::*;
use crate::model::TICKS_PER_QUARTER;

const SHOWCASE: &[u8] = include_bytes!("../../../testdata/gp7/showcase.gpif");
const LEGACY: &[u8] = include_bytes!("../../../testdata/gp7/legacy.gpif");

fn showcase() -> Score {
    parse_gpif(SHOWCASE).expect("showcase parses")
}

fn legacy() -> Score {
    parse_gpif(LEGACY).expect("legacy parses")
}

fn beats(score: &Score, track: usize, bar: usize) -> &[Beat] {
    &score.tracks[track].bars[bar].voices[0].beats
}

/// Wrap master-bar-level GPIF snippets into a minimal one-track document.
fn doc(extra_root: &str, notes: &str) -> String {
    format!(
        r#"<GPIF>
  <MasterTrack><Tracks>0</Tracks></MasterTrack>
  <Tracks><Track id="0"><Name>T</Name><Staves><Staff><Properties>
    <Property name="Tuning"><Pitches>40 45 50 55 59 64</Pitches></Property>
  </Properties></Staff></Staves></Track></Tracks>
  <MasterBars><MasterBar><Time>4/4</Time><Bars>0</Bars></MasterBar></MasterBars>
  <Bars><Bar id="0"><Voices>0</Voices></Bar></Bars>
  <Voices><Voice id="0"><Beats>0</Beats></Voice></Voices>
  <Beats><Beat id="0"><Rhythm ref="0"/><Notes>0</Notes></Beat></Beats>
  <Notes><Note id="0">{notes}</Note></Notes>
  <Rhythms><Rhythm id="0"><NoteValue>Whole</NoteValue></Rhythm></Rhythms>
  {extra_root}
</GPIF>"#
    )
}

fn single_note(note_xml: &str) -> Note {
    let score = parse_gpif(doc("", note_xml).as_bytes()).unwrap();
    score.tracks[0].bars[0].voices[0].beats[0].notes[0].clone()
}

fn props(p: &str) -> String {
    format!(
        r#"<Properties><Property name="String"><String>0</String></Property>
        <Property name="Fret"><Fret>3</Fret></Property>{p}</Properties>"#
    )
}

// --- Snapshots ---------------------------------------------------------------------------

#[test]
fn showcase_snapshot() {
    insta::assert_debug_snapshot!(showcase());
}

#[test]
fn legacy_snapshot() {
    insta::assert_debug_snapshot!(legacy());
}

// --- Score-level mappings ----------------------------------------------------------------

#[test]
fn metadata() {
    let s = showcase();
    assert_eq!(s.title, "Tabster Showcase");
    assert_eq!(s.subtitle, "A self-authored test tab");
    assert_eq!(s.artist, "The Fixtures");
    assert_eq!(s.album, "Testdata & Friends");
    assert_eq!(s.words, "Tabster Authors");
    assert_eq!(s.music, "Tabster Authors");
    assert_eq!(s.tab_author, "Agent A");
    // WordsAndMusic fills both when they're empty.
    let l = legacy();
    assert_eq!(
        (l.words.as_str(), l.music.as_str()),
        ("Tabster Authors", "Tabster Authors")
    );
}

#[test]
fn tracks_and_tunings() {
    let s = showcase();
    assert_eq!(s.tracks.len(), 2);
    let gtr = &s.tracks[0];
    assert_eq!(
        (gtr.name.as_str(), gtr.short_name.as_str()),
        ("Lead Guitar", "L.Gtr")
    );
    assert_eq!(gtr.tuning, [64, 59, 55, 50, 45, 40], "string 1 = highest");
    assert_eq!(gtr.capo, 2);
    assert_eq!(gtr.program, 29, "first Sound's program");
    assert!(!gtr.is_percussion);
    let bass = &s.tracks[1];
    assert_eq!(bass.tuning, [43, 38, 33, 26], "drop D bass");
    assert_eq!((bass.capo, bass.program), (0, 33));
    for t in &s.tracks {
        assert_eq!(t.bars.len(), s.master_bars.len());
    }
}

#[test]
fn legacy_tracks() {
    let s = legacy();
    // Track 9 isn't in MasterTrack/Tracks, so it's dropped.
    let names: Vec<_> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Rhythm", "Drums", "Piano"]);
    assert_eq!(
        s.tracks[0].tuning,
        [64, 59, 55, 50, 45, 38],
        "misspelt 'Tunning'"
    );
    assert_eq!(s.tracks[0].program, 25);
    assert!(s.tracks[1].is_percussion);
    assert_eq!(s.tracks[1].tuning, [0; 6]);
    assert_eq!(
        s.tracks[2].tuning,
        [0],
        "untuned track gets one zero string"
    );
    for t in &s.tracks {
        assert_eq!(t.bars.len(), 3, "{}", t.name);
    }
}

#[test]
fn time_and_key_signatures() {
    let s = showcase();
    let sigs: Vec<_> = s
        .master_bars
        .iter()
        .map(|m| (m.time_signature.numerator, m.time_signature.denominator))
        .collect();
    assert_eq!(
        sigs,
        [
            (4, 4),
            (4, 4),
            (4, 4),
            (4, 4),
            (6, 8),
            (6, 8),
            (4, 4),
            (4, 4)
        ]
    );
    assert_eq!(s.master_bars[6].key_signature, 1);
    assert_eq!(legacy().master_bars[0].key_signature, -2);
}

#[test]
fn repeats_and_alternate_endings() {
    let s = showcase();
    let mb = &s.master_bars;
    assert!(mb[0].repeat_start && mb[0].repeat_end == 0);
    assert!(!mb[1].repeat_start && mb[1].repeat_end == 2);
    assert_eq!(mb[1].alternate_endings, 0b01);
    assert_eq!(mb[2].alternate_endings, 0b10);
    assert_eq!(mb[3].alternate_endings, 0);
    let l = legacy();
    assert!(l.master_bars[1].repeat_start);
    assert_eq!(l.master_bars[1].repeat_end, 3);
}

#[test]
fn sections_directions_and_bar_flags() {
    let s = showcase();
    let mb = &s.master_bars;
    assert_eq!(
        mb[0].section,
        Some(Section {
            letter: "A".into(),
            text: "Intro".into()
        })
    );
    assert_eq!(mb[3].section.as_ref().unwrap().text, "Verse");
    assert_eq!(mb[3].directions, [Direction::Segno]);
    assert_eq!(mb[4].directions, [Direction::DaCoda]);
    assert_eq!(mb[5].directions, [Direction::DalSegnoAlCoda]);
    assert_eq!(mb[6].directions, [Direction::Coda]);
    assert_eq!(mb[6].triplet_feel, TripletFeel::Triplet8th);
    assert!(mb[2].double_bar);
    assert!(!mb[7].double_bar, "trailing double bar is cleared");
    assert!(!mb[0].anacrusis);

    let l = legacy();
    assert_eq!(
        l.master_bars[1].directions,
        [Direction::Fine, Direction::DaCapoAlFine]
    );
    assert!(l.master_bars[0].anacrusis);
    assert!(!l.master_bars[1].anacrusis);
}

#[test]
fn jump_spellings() {
    assert_eq!(jump_direction("DaSegno"), Some(Direction::DalSegno));
    assert_eq!(jump_direction("DalSegno"), Some(Direction::DalSegno));
    assert_eq!(
        jump_direction("DaSegnoSegnoAlDoubleCoda"),
        Some(Direction::DalSegnoSegnoAlDoubleCoda)
    );
    assert_eq!(
        jump_direction("DaDoubleCoda"),
        Some(Direction::DaDoubleCoda)
    );
    assert_eq!(jump_direction("Nope"), None);
    assert_eq!(target_direction("SegnoSegno"), Some(Direction::SegnoSegno));
}

#[test]
fn tempo_normalised_to_quarters() {
    let s = showcase();
    assert_eq!(s.initial_tempo, 120.0);
    let changes: Vec<_> = s
        .tempo_changes
        .iter()
        .map(|t| (t.bar, t.position, t.bpm))
        .collect();
    // "60 3" is 60 dotted quarters = 90 quarters; the bar-99 automation is out of range.
    assert_eq!(changes, [(0, 0.0, 120.0), (2, 0.5, 140.0), (4, 0.0, 90.0)]);

    let l = legacy();
    // A bare value is quarter notes; "180 1" is 180 eighths = 90 quarters. Sorted by position.
    assert_eq!(l.initial_tempo, 100.0);
    let changes: Vec<_> = l
        .tempo_changes
        .iter()
        .map(|t| (t.bar, t.position, t.bpm))
        .collect();
    assert_eq!(changes, [(0, 0.0, 100.0), (1, 0.25, 90.0)]);
}

#[test]
fn tempo_units() {
    let xml = |v: &str| {
        doc("", "").replace(
            "<MasterTrack><Tracks>0</Tracks></MasterTrack>",
            &format!(
                "<MasterTrack><Tracks>0</Tracks><Automations><Automation><Type>Tempo</Type>\
                 <Bar>0</Bar><Position>0</Position><Value>{v}</Value></Automation>\
                 </Automations></MasterTrack>"
            ),
        )
    };
    let tempo = |v: &str| parse_gpif(xml(v).as_bytes()).unwrap().initial_tempo;
    assert_eq!(tempo("100 1"), 50.0);
    assert_eq!(tempo("100 2"), 100.0);
    assert_eq!(tempo("100 3"), 150.0);
    assert_eq!(tempo("100 4"), 200.0);
    assert_eq!(tempo("100 5"), 300.0);
    assert_eq!(tempo("100 9"), 100.0);
    assert_eq!(tempo("-5 2"), DEFAULT_TEMPO, "nonsense tempo ignored");
    assert_eq!(tempo("abc"), DEFAULT_TEMPO);
}

#[test]
fn no_tempo_defaults_to_120() {
    let s = parse_gpif(doc("", "").as_bytes()).unwrap();
    assert_eq!(s.initial_tempo, 120.0);
    assert!(s.tempo_changes.is_empty());
}

#[test]
fn sync_points() {
    let s = showcase();
    // FrameOffset / 44100 s, minus the backing track's 4410-frame (100 ms) padding; sorted.
    assert_eq!(
        s.sync_points,
        [
            FileSyncPoint {
                bar: 0,
                bar_occurrence: 0,
                bar_position: 0.0,
                millisecond_offset: 900.0,
            },
            FileSyncPoint {
                bar: 0,
                bar_occurrence: 1,
                bar_position: 0.0,
                millisecond_offset: 8000.0,
            },
        ]
    );
    assert!(legacy().sync_points.is_empty());
}

// --- Rhythm ------------------------------------------------------------------------------

#[test]
fn rhythms_tuplets_and_dots() {
    let s = showcase();
    let b = beats(&s, 0, 1);
    assert_eq!(
        b[0].duration,
        Duration {
            value: 8,
            dots: 0,
            tuplet: Some((3, 2))
        }
    );
    assert_eq!(
        b[..3].iter().map(|b| b.duration.ticks()).sum::<u32>(),
        TICKS_PER_QUARTER
    );
    assert_eq!(
        b[3].duration,
        Duration {
            value: 4,
            dots: 1,
            tuplet: None
        }
    );
    // Every guitar bar adds up to its time signature (grace beats take no time).
    for (i, bar) in s.tracks[0].bars.iter().enumerate() {
        let ticks: u32 = bar.voices[0]
            .beats
            .iter()
            .filter(|b| !b.effects.grace)
            .map(|b| b.duration.ticks())
            .sum();
        assert_eq!(
            ticks,
            s.master_bars[i].time_signature.bar_ticks(),
            "bar {i}"
        );
    }
    let l = legacy();
    let d = l.tracks[0].bars[1].voices[1].beats[0].duration;
    assert_eq!((d.value, d.dots, d.tuplet), (8, 2, Some((5, 4))));
    assert_eq!(
        beats(&l, 2, 1)[0].duration.value,
        1,
        "double whole clamps to whole"
    );
}

#[test]
fn rests() {
    let s = showcase();
    assert!(beats(&s, 0, 0)[4].is_rest());
    assert!(beats(&s, 0, 5)[0].is_rest());
    assert_eq!(beats(&s, 0, 5)[0].duration.ticks(), 2880);
}

// --- Notes -------------------------------------------------------------------------------

#[test]
fn strings_are_flipped() {
    let s = showcase();
    let n = &beats(&s, 0, 0)[0].notes[0];
    // GPIF string 1 (A string, second lowest) → model string 5.
    assert_eq!((n.string, n.fret), (5, 3));
    assert_eq!(
        s.tracks[0].pitch(n.string, n.fret),
        Some(50),
        "matches the file's Midi"
    );
    let low_e = &beats(&s, 0, 3)[0].notes[0];
    assert_eq!(low_e.string, 6);
    let bass = &beats(&s, 1, 0)[0].notes[0];
    assert_eq!(
        (bass.string, s.tracks[1].pitch(bass.string, bass.fret)),
        (4, Some(26))
    );
}

#[test]
fn note_effects() {
    let s = showcase();
    let g = |bar, beat: usize, note: usize| beats(&s, 0, bar)[beat].notes[note].clone();
    assert!(g(0, 0, 0).effects.palm_mute);
    assert!(g(0, 1, 0).effects.dead);
    assert!(g(0, 2, 0).effects.hammer_pull);
    assert!(!g(0, 3, 0).effects.hammer_pull);
    assert_eq!(
        g(1, 3, 0).effects.bend,
        Some(Bend {
            points: vec![(0.0, 0.0), (0.6, 2.0)]
        })
    );
    assert_eq!(g(1, 4, 0).effects.slide, Some(Slide::Legato));
    assert_eq!(
        g(1, 5, 0).effects.trill,
        Some(9),
        "trill pitch 70 on B string with capo 2"
    );
    let tie_from = g(2, 0, 0);
    let tie_to = g(2, 1, 0);
    assert!(!tie_from.tie_destination && tie_to.tie_destination);
    assert_eq!(tie_from.effects.harmonic, Some(Harmonic::Natural));
    let held = g(3, 0, 0);
    assert!(held.effects.vibrato && held.effects.let_ring);
    assert!(g(4, 1, 0).effects.accent);
    assert!(g(4, 2, 0).effects.ghost);
    let e = g(4, 3, 0).effects;
    assert!(e.staccato && e.heavy_accent && !e.accent);
    assert!(g(6, 2, 0).effects.tapped);
    assert_eq!(g(7, 0, 0).effects.slide, Some(Slide::OutDown));
    assert_eq!(
        beats(&s, 1, 0)[1].notes[0].effects.slide,
        Some(Slide::InFromBelow)
    );
}

#[test]
fn velocities_from_dynamics() {
    let s = showcase();
    assert_eq!(beats(&s, 0, 0)[0].notes[0].velocity, 79, "MF");
    assert_eq!(beats(&s, 0, 2)[0].notes[0].velocity, 111, "FF");
    assert_eq!(beats(&s, 0, 3)[0].notes[0].velocity, 31, "PP");
    assert_eq!(beats(&s, 1, 0)[0].notes[0].velocity, 95, "no dynamic = F");
    let l = legacy();
    assert_eq!(
        l.tracks[0].bars[1].voices[1].beats[0].notes[0].velocity,
        127
    );
    assert_eq!(beats(&l, 1, 1)[0].notes[0].velocity, 15);
}

#[test]
fn single_note_properties() {
    let n = single_note(&props(
        r#"<Property name="HarmonicType"><HType>Pinch</HType></Property>"#,
    ));
    assert_eq!((n.string, n.fret), (6, 3));
    assert_eq!(n.effects.harmonic, Some(Harmonic::Pinch));
    for (flags, slide) in [
        (1, Slide::Shift),
        (2, Slide::Legato),
        (8, Slide::OutUp),
        (32, Slide::InFromAbove),
        (64, Slide::OutDown),
        (18, Slide::Legato),
    ] {
        let n = single_note(&props(&format!(
            r#"<Property name="Slide"><Flags>{flags}</Flags></Property>"#
        )));
        assert_eq!(n.effects.slide, Some(slide), "flags {flags}");
    }
    // Bend with a middle point and release.
    let n = single_note(&props(
        r#"<Property name="Bended"><Enable/></Property>
        <Property name="BendMiddleValue"><Float>50</Float></Property>
        <Property name="BendDestinationValue"><Float>0</Float></Property>"#,
    ));
    assert_eq!(
        n.effects.bend.unwrap().points,
        [(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)]
    );
    // Bend values without the Bended flag are ignored.
    let n = single_note(&props(
        r#"<Property name="BendDestinationValue"><Float>100</Float></Property>"#,
    ));
    assert_eq!(n.effects.bend, None);
}

#[test]
fn notes_without_string_use_pitch() {
    let l = legacy();
    let gtr = &l.tracks[0];
    // Midi 67 with no string → highest playable string (E4 = 64), fret 3.
    let n = &beats(&l, 0, 0)[1].notes[0];
    assert_eq!((n.string, n.fret), (1, 3));
    // String 9 doesn't exist on a 6-string: falls back to its concert pitch F#3 (54).
    let n = &l.tracks[0].bars[1].voices[1].beats[0].notes[0];
    assert_eq!(gtr.pitch(n.string, n.fret), Some(54));
    assert_eq!((n.string, n.fret), (4, 4));
}

#[test]
fn percussion_and_untuned_tracks() {
    let l = legacy();
    let drums = &l.tracks[1];
    let pitches = |bar: usize| -> Vec<Vec<u8>> {
        drums.bars[bar].voices[0]
            .beats
            .iter()
            .map(|b| {
                b.notes
                    .iter()
                    .map(|n| drums.pitch(n.string, n.fret).unwrap())
                    .collect()
            })
            .collect()
    };
    // Kick + open hi-hat; side stick; ride bell.
    assert_eq!(pitches(0), [vec![35, 46], vec![35, 46]]);
    assert_eq!(pitches(1), [vec![37], vec![59]]);
    let strings: Vec<_> = beats(&l, 1, 0)[0].notes.iter().map(|n| n.string).collect();
    assert_eq!(strings, [1, 2], "drum notes get distinct strings");

    let piano = &l.tracks[2];
    let n = &beats(&l, 2, 0)[0].notes[0];
    assert_eq!(piano.pitch(n.string, n.fret), Some(60));
    let chord: Vec<_> = beats(&l, 2, 1)[0]
        .notes
        .iter()
        .map(|n| piano.pitch(n.string, n.fret).unwrap())
        .collect();
    assert_eq!(chord, [64, 70], "Octave/Tone and ConcertPitch fallbacks");
}

#[test]
fn gp7_drum_articulations() {
    let xml = r#"<GPIF>
  <MasterTrack><Tracks>0</Tracks></MasterTrack>
  <Tracks><Track id="0"><Name>Drums</Name>
    <InstrumentSet><Type>drumKit</Type><Elements>
      <Element><Name>Snare</Name><Articulations>
        <Articulation><OutputMidiNumber>38</OutputMidiNumber></Articulation>
        <Articulation><OutputMidiNumber>37</OutputMidiNumber></Articulation>
      </Articulations></Element>
      <Element><Name>Kick</Name><Articulations>
        <Articulation><OutputMidiNumber>36</OutputMidiNumber></Articulation>
      </Articulations></Element>
    </Elements></InstrumentSet>
    <Staves><Staff><Properties><Property name="Tuning"><Pitches>0 0 0 0 0 0</Pitches></Property></Properties></Staff></Staves>
  </Track></Tracks>
  <MasterBars><MasterBar><Time>4/4</Time><Bars>0</Bars></MasterBar></MasterBars>
  <Bars><Bar id="0"><Voices>0 -1 -1 -1</Voices></Bar></Bars>
  <Voices><Voice id="0"><Beats>0</Beats></Voice></Voices>
  <Beats><Beat id="0"><Rhythm ref="0"/><Notes>0 1</Notes></Beat></Beats>
  <Notes>
    <Note id="0"><InstrumentArticulation>2</InstrumentArticulation>
      <Properties><Property name="String"><String>5</String></Property></Properties></Note>
    <Note id="1"><InstrumentArticulation>1</InstrumentArticulation></Note>
  </Notes>
  <Rhythms><Rhythm id="0"><NoteValue>Whole</NoteValue></Rhythm></Rhythms>
</GPIF>"#;
    let s = parse_gpif(xml.as_bytes()).unwrap();
    let t = &s.tracks[0];
    assert!(t.is_percussion);
    let notes: Vec<_> = t.bars[0].voices[0].beats[0]
        .notes
        .iter()
        .map(|n| (n.string, n.fret))
        .collect();
    assert_eq!(notes, [(1, 36), (2, 37)]);
}

// --- Beats -------------------------------------------------------------------------------

#[test]
fn beat_effects() {
    let s = showcase();
    assert_eq!(beats(&s, 0, 0)[0].text.as_deref(), Some("Intro riff"));
    assert_eq!(beats(&s, 0, 4)[0].chord.as_deref(), Some("Em"));
    assert_eq!(beats(&s, 0, 4)[0].notes.len(), 2);
    let coda = beats(&s, 0, 6);
    assert!(coda[0].effects.grace);
    assert_eq!(coda[1].effects.stroke, Some(Stroke::Up));
    assert_eq!(coda[1].notes.len(), 3);
    assert!(coda[2].effects.tapping);
    assert!(!coda[1].effects.tapping);
    let last = &beats(&s, 0, 7)[0];
    assert!(last.effects.fade_in);
    assert_eq!(last.effects.tremolo_picking, Some(16));
    assert_eq!(
        last.effects.whammy,
        Some(Bend {
            points: vec![(0.0, 0.0), (0.5, -1.0), (1.0, 0.0)]
        })
    );
    let bass = beats(&s, 1, 6);
    assert!(bass[0].effects.slap && !bass[0].effects.pop);
    assert!(bass[1].effects.pop && !bass[1].effects.slap);

    let l = legacy();
    let b = beats(&l, 0, 0);
    assert_eq!(b[0].chord.as_deref(), Some("D5"));
    assert_eq!(b[0].effects.stroke, Some(Stroke::Down));
    assert_eq!(
        b[1].effects.whammy,
        Some(Bend {
            points: vec![(0.0, 0.0), (1.0, -2.0)]
        })
    );
}

// --- Structure and robustness ------------------------------------------------------------

#[test]
fn shared_and_missing_references() {
    let s = showcase();
    // Bass bars 1–3 share voice 9; master bar 7 lists "-1" for the bass.
    assert_eq!(s.tracks[1].bars[1], s.tracks[1].bars[3]);
    assert_eq!(s.tracks[1].bars[7], Bar::default());

    let l = legacy();
    // Bar 404 doesn't exist and master bar 2 lists no piano bar: both become empty bars.
    assert_eq!(l.tracks[0].bars[2], Bar::default());
    assert_eq!(l.tracks[2].bars[2], Bar::default());
    // Voice "-1 4": the missing first voice is kept as an empty placeholder.
    let bar = &l.tracks[0].bars[1];
    assert_eq!(bar.voices.len(), 2);
    assert!(bar.voices[0].beats.is_empty());
    // Beat 404 is missing and beat 5's rhythm is missing: both skipped.
    assert_eq!(bar.voices[1].beats.len(), 1);
    // Note 404 is missing: skipped.
    assert_eq!(beats(&l, 0, 0)[1].notes.len(), 1);
    // Two-staff piano: its second staff's bar ("3") is skipped, not given to another track.
    assert_eq!(l.tracks[2].bars[0].voices[0].beats.len(), 1);
    assert_eq!(beats(&l, 2, 0)[0].duration.value, 2);
}

#[test]
fn malformed_xml_is_an_error() {
    for bad in [
        &b""[..],
        b"not xml at all",
        b"<GPIF><Score>",
        b"<GPIF></Score>",
        b"<GPIF><a b=></a></GPIF>",
        b"\xff\xfe<\x00",
    ] {
        assert!(
            matches!(parse_gpif(bad), Err(ParseError::Invalid(_))),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
    assert!(matches!(
        parse_gpif(b"<Score/>"),
        Err(ParseError::Invalid(_))
    ));
}

#[test]
fn minimal_documents_parse() {
    let s = parse_gpif(b"\xEF\xBB\xBF<GPIF/>").unwrap();
    assert!(s.tracks.is_empty() && s.master_bars.is_empty());
    assert_eq!(s.initial_tempo, 120.0);
    // Tracks without a MasterTrack list come in document order.
    let s = parse_gpif(
        br#"<GPIF><Tracks><Track id="5"><Name>B</Name></Track><Track id="2"><Name>A</Name></Track></Tracks>
        <MasterBars><MasterBar><Bars>x y</Bars></MasterBar></MasterBars></GPIF>"#,
    )
    .unwrap();
    let names: Vec<_> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["B", "A"]);
    assert!(s.tracks.iter().all(|t| t.bars == [Bar::default()]));
}

#[test]
fn bad_numbers_fall_back() {
    let xml = doc("", &props(""))
        .replace("<Time>4/4</Time>", "<Time>banana</Time><Key><AccidentalCount>99</AccidentalCount></Key>")
        .replace(
            "<NoteValue>Whole</NoteValue>",
            "<NoteValue>Whole</NoteValue><AugmentationDot count=\"9\"/><PrimaryTuplet num=\"0\" den=\"2\"/>",
        );
    let s = parse_gpif(xml.as_bytes()).unwrap();
    assert_eq!(s.master_bars[0].time_signature, TimeSignature::default());
    assert_eq!(s.master_bars[0].key_signature, 7);
    let d = s.tracks[0].bars[0].voices[0].beats[0].duration;
    assert_eq!((d.value, d.dots, d.tuplet), (1, 2, None));
    // A fret below zero or a string out of range without a pitch drops the note.
    let s = parse_gpif(
        doc(
            "",
            r#"<Properties><Property name="String"><String>7</String></Property></Properties>"#,
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(s.tracks[0].bars[0].voices[0].beats[0].notes.is_empty());
}

#[test]
fn truncated_documents_never_panic() {
    for cut in (0..SHOWCASE.len()).step_by(97) {
        let _ = parse_gpif(&SHOWCASE[..cut]);
    }
    for cut in (0..LEGACY.len()).step_by(89) {
        let _ = parse_gpif(&LEGACY[..cut]);
    }
}

proptest::proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..512)) {
        let _ = parse_gpif(&bytes);
    }

    #[test]
    fn mutated_fixture_never_panics(pos in 0usize..SHOWCASE.len(), byte in proptest::num::u8::ANY) {
        let mut xml = SHOWCASE.to_vec();
        xml[pos] = byte;
        let _ = parse_gpif(&xml);
    }
}
