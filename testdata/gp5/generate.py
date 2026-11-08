"""Generate the self-authored .gp5 fixtures in this directory with PyGuitarPro.

    python3 -m venv /tmp/pygp && /tmp/pygp/bin/pip install PyGuitarPro==0.11
    /tmp/pygp/bin/python testdata/gp5/generate.py

Every file is written, read back with PyGuitarPro and summarised to stdout as a sanity check.
The Rust snapshot tests (src/formats/gp5.rs) assert the importer sees the same content.

Note: PyGuitarPro and alphaTab disagree on the byte order when a measure header has both a
time-signature change and an alternate ending; the fixtures avoid that combination.
"""

import os

import guitarpro as gp

HERE = os.path.dirname(os.path.abspath(__file__))
D = gp.Duration


def header(song, numerator=4, denominator=4, **kwargs):
    h = gp.MeasureHeader(
        timeSignature=gp.TimeSignature(numerator, gp.Duration(denominator)), **kwargs
    )
    song.addMeasureHeader(h)
    for track in song.tracks:
        track.measures.append(gp.Measure(track, h))
    return h


def beat(voice, value=D.quarter, notes=(), dotted=False, tuplet=None, status=None, **kwargs):
    """notes: list of (string, fret) or (string, fret, dict of note kwargs)."""
    duration = gp.Duration(value, dotted)
    if tuplet:
        duration.tuplet = gp.Tuplet(*tuplet)
    b = gp.Beat(voice, duration=duration, **kwargs)
    if status is None:
        status = gp.BeatStatus.normal if notes else gp.BeatStatus.rest
    b.status = status
    for spec in notes:
        string, fret = spec[0], spec[1]
        extra = dict(spec[2]) if len(spec) > 2 else {}
        effect = extra.pop("effect", gp.NoteEffect())
        n = gp.Note(b, value=fret, string=string, type=extra.pop("type", gp.NoteType.normal),
                    effect=effect, **extra)
        b.notes.append(n)
    voice.beats.append(b)
    return b


def bend(points):
    return gp.BendEffect(type=gp.BendType.bend, value=max(v for _, v in points),
                         points=[gp.BendPoint(p, v) for p, v in points])


def new_song(**info):
    song = gp.Song(**info)
    song.measureHeaders = []
    song.tracks = []
    return song


def make_track(song, number, name, strings, channel, instrument, capo=0, percussion=False):
    track = gp.Track(song, number=number, name=name, offset=capo,
                     isPercussionTrack=percussion, measures=[],
                     strings=[gp.GuitarString(i + 1, v) for i, v in enumerate(strings)])
    track.channel = gp.MidiChannel(channel=channel, effectChannel=channel, instrument=instrument)
    song.tracks.append(track)
    return track


def basics():
    """GP 5.10: two tracks, repeats, endings, markers, directions, most effects."""
    song = new_song(title="Tabster Basics", subtitle="GP5 fixture", artist="Tabster",
                    album="Fixtures", words="Nobody", music="Tabster tests",
                    copyright="CC0", tab="generate.py", instructions="Play it")
    song.tempo = 120
    song.key = gp.KeySignature.CMajor
    gtr = make_track(song, 1, "Guitar", [64, 59, 55, 50, 45, 40], 0, 29, capo=2)
    bass = make_track(song, 2, "Bass", [43, 38, 33, 28], 1, 33)

    # Bar 1: marker, repeat open. Hammer-on, legato slide, tie into bar 2.
    header(song, 4, 4, isRepeatOpen=True, marker=gp.Marker("Intro", gp.Color(255, 0, 0)))
    v = gtr.measures[0].voices[0]
    beat(v, D.quarter, [(3, 5, {"effect": gp.NoteEffect(hammer=True)})], text="hammer")
    beat(v, D.quarter, [(3, 7, {"effect": gp.NoteEffect(slides=[gp.SlideType.legatoSlideTo])})])
    beat(v, D.quarter, [(3, 9)])
    beat(v, D.quarter, [(1, 0), (2, 1), (3, 0)])  # chord shape C (top three strings)
    v = bass.measures[0].voices[0]
    beat(v, D.whole, [(4, 3)])

    # Bar 2: triplets, dotted, rest, palm mute, dead note, tie.
    h2 = header(song, 4, 4)
    v = gtr.measures[1].voices[0]
    for fret in (0, 2, 3):
        beat(v, D.eighth, [(6, fret, {"effect": gp.NoteEffect(palmMute=True)})], tuplet=(3, 2))
    beat(v, D.quarter, dotted=True, notes=[(5, 0, {"type": gp.NoteType.dead})])
    beat(v, D.eighth)  # rest
    beat(v, D.quarter, [(5, 3, {"velocity": gp.Velocities.piano})])
    v = bass.measures[1].voices[0]
    beat(v, D.half, [(4, 3)])
    beat(v, D.half, [(4, 3, {"type": gp.NoteType.tie})])

    # Bar 3: first ending, repeat close (played twice).
    header(song, 4, 4, repeatAlternative=1, repeatClose=1)
    v = gtr.measures[2].voices[0]
    beat(v, D.half, [(2, 5, {"effect": gp.NoteEffect(
        bend=bend([(0, 0), (6, 4), (12, 4)]))})])
    beat(v, D.half, [(2, 8, {"effect": gp.NoteEffect(vibrato=True, letRing=True)})])
    v = bass.measures[2].voices[0]
    beat(v, D.whole, [(3, 0)])

    # Bar 4: second ending, double bar.
    header(song, 4, 4, repeatAlternative=2, hasDoubleBar=True)
    v = gtr.measures[3].voices[0]
    beat(v, D.quarter, [(1, 12, {"effect": gp.NoteEffect(harmonic=gp.NaturalHarmonic())})])
    beat(v, D.quarter, [(1, 5, {"effect": gp.NoteEffect(harmonic=gp.PinchHarmonic())})])
    beat(v, D.quarter, [(1, 7, {"effect": gp.NoteEffect(
        grace=gp.GraceEffect(fret=5, transition=gp.GraceEffectTransition.hammer))})])
    beat(v, D.quarter, [(1, 7, {"effect": gp.NoteEffect(
        trill=gp.TrillEffect(fret=9, duration=gp.Duration(D.sixteenth)))})])
    v = bass.measures[3].voices[0]
    beat(v, D.whole, [(4, 1)])

    # Bar 5: 6/8, key of G, tempo change to 90 on beat 2, segno, two voices.
    header(song, 6, 8, keySignature=gp.KeySignature.GMajor,
           direction=gp.DirectionSign("Segno"))
    v = gtr.measures[4].voices[0]
    beat(v, D.quarter, dotted=True, notes=[(2, 3, {"effect": gp.NoteEffect(
        accentuatedNote=True, ghostNote=True)})])
    b = beat(v, D.quarter, dotted=True, notes=[(2, 5, {"effect": gp.NoteEffect(
        heavyAccentuatedNote=True, staccato=True)})])
    b.effect.mixTableChange = gp.MixTableChange(tempo=gp.MixTableItem(90), tempoName="Slower")
    v2 = gtr.measures[4].voices[1]
    beat(v2, D.eighth, [(6, 3)])
    beat(v2, D.eighth, [(6, 5)])
    beat(v2, D.eighth, [(6, 7)])
    beat(v2, D.quarter, dotted=True)  # rest in voice 2
    v = bass.measures[4].voices[0]
    beat(v, D.quarter, dotted=True, notes=[(4, 3)],
         effect=gp.BeatEffect(slapEffect=gp.SlapEffect.slapping))
    beat(v, D.quarter, dotted=True, notes=[(3, 5)],
         effect=gp.BeatEffect(slapEffect=gp.SlapEffect.popping))

    # Bar 6: 6/8, triplet feel, D.S. al Fine, whammy, strokes, fade in, tapping, tremolo.
    header(song, 6, 8, keySignature=gp.KeySignature.GMajor,
           tripletFeel=gp.TripletFeel.eighth,
           fromDirection=gp.DirectionSign("Da Segno al Fine"))
    v = gtr.measures[5].voices[0]
    beat(v, D.eighth, [(1, 3), (2, 3), (3, 0)],
         effect=gp.BeatEffect(stroke=gp.BeatStroke(gp.BeatStrokeDirection.down, D.sixteenth)))
    beat(v, D.eighth, [(1, 3), (2, 3), (3, 0)],
         effect=gp.BeatEffect(stroke=gp.BeatStroke(gp.BeatStrokeDirection.up, D.sixteenth)))
    beat(v, D.eighth, [(1, 12)], effect=gp.BeatEffect(slapEffect=gp.SlapEffect.tapping))
    beat(v, D.eighth, [(1, 10)], effect=gp.BeatEffect(fadeIn=True))
    beat(v, D.quarter, [(2, 7, {"effect": gp.NoteEffect(
        tremoloPicking=gp.TremoloPickingEffect(gp.Duration(D.thirtySecond)))})],
         effect=gp.BeatEffect(tremoloBar=gp.BendEffect(
             type=gp.BendType.dip, value=-4,
             points=[gp.BendPoint(0, 0), gp.BendPoint(6, -4), gp.BendPoint(12, 0)])))
    v = bass.measures[5].voices[0]
    beat(v, D.half, dotted=True, notes=[(4, 3, {"effect": gp.NoteEffect(
        slides=[gp.SlideType.intoFromBelow])})])

    # Bar 7: 6/8 again, Fine, slide out, an explicit "empty" beat in voice 2 (ignored).
    header(song, 6, 8, keySignature=gp.KeySignature.GMajor,
           direction=gp.DirectionSign("Fine"))
    v = gtr.measures[6].voices[0]
    beat(v, D.half, dotted=True, notes=[(1, 3, {"effect": gp.NoteEffect(
        slides=[gp.SlideType.outDownwards])})])
    beat(gtr.measures[6].voices[1], D.quarter, status=gp.BeatStatus.empty)
    v = bass.measures[6].voices[0]
    beat(v, D.half, dotted=True, notes=[(4, 3)])

    _ = h2
    return song


def legacy_500():
    """GP 5.00: guitar + drums, a tempo change in the middle of a bar."""
    song = new_song(title="Tabster v5.00", artist="Tabster")
    song.tempo = 100
    gtr = make_track(song, 1, "Clean", [64, 59, 55, 50, 45, 40], 0, 27)
    drums = make_track(song, 2, "Drums", [0, 0, 0, 0, 0, 0], 9, 0, percussion=True)

    for bar in range(2):
        header(song, 3, 4)
        v = gtr.measures[bar].voices[0]
        beat(v, D.quarter, [(6, 0)])
        b = beat(v, D.quarter, [(5, 2), (4, 2)])
        if bar == 1:
            b.effect.mixTableChange = gp.MixTableChange(tempo=gp.MixTableItem(140))
        beat(v, D.quarter, [(3, 1, {"effect": gp.NoteEffect(
            slides=[gp.SlideType.shiftSlideTo])})])
        v = drums.measures[bar].voices[0]
        beat(v, D.quarter, [(1, 42), (6, 36)])
        beat(v, D.quarter, [(1, 42), (5, 38)])
        beat(v, D.quarter, [(1, 42)])
    return song


def main():
    for name, song, version in [
        ("basics.gp5", basics(), (5, 1, 0)),
        ("v500.gp5", legacy_500(), (5, 0, 0)),
    ]:
        path = os.path.join(HERE, name)
        gp.write(song, path, version=version)
        back = gp.parse(path)
        print(f"{name}: {back.version!r} {back.title!r} tempo={back.tempo} "
              f"bars={len(back.measureHeaders)} tracks={[t.name for t in back.tracks]}")
        for h in back.measureHeaders:
            print(f"  bar {h.number}: {h.timeSignature.numerator}/"
                  f"{h.timeSignature.denominator.value} open={h.isRepeatOpen} "
                  f"close={h.repeatClose} alt={h.repeatAlternative} "
                  f"marker={h.marker.title if h.marker else None} "
                  f"dir={h.direction and h.direction.name} "
                  f"from={h.fromDirection and h.fromDirection.name}")
        for t in back.tracks:
            for m in t.measures:
                for vi, voice in enumerate(m.voices):
                    for b in voice.beats:
                        notes = [(n.string, n.value, n.type.name) for n in b.notes]
                        print(f"    {t.name} bar {m.number} v{vi} {b.status.name} "
                              f"1/{b.duration.value}{'.' if b.duration.isDotted else ''} {notes}")


if __name__ == "__main__":
    main()
