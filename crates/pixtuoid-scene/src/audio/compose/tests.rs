//! The seed-sweep property suite: frozen takes pin ONE realization
//! (checksums); these pin the RULES — any seed must be musically well-formed.

use super::*;

const SWEEP: u64 = 64;

#[test]
fn template_chords_and_roots_are_diatonic_or_deliberately_chromatic() {
    for (name, progs) in [
        ("day", &DAY_PROGRESSIONS[..]),
        ("night", &NIGHT_PROGRESSIONS[..]),
    ] {
        for (i, p) in progs.iter().enumerate() {
            let color_tones = p
                .chords
                .iter()
                .flatten()
                .filter(|&&n| !p.scale_pcs.contains(&(n % 12)))
                .count();
            if p.chromatic {
                // at most one chord's worth of color, so it stays in the key
                assert!(
                    (1..=4).contains(&color_tones),
                    "{name}[{i}]: chromatic template carries {color_tones} color tones"
                );
            } else {
                assert_eq!(
                    color_tones, 0,
                    "{name}[{i}]: diatonic template leaked a color tone"
                );
            }
            for &r in &p.roots_pc {
                assert!(
                    p.scale_pcs.contains(&r),
                    "{name}[{i}]: root pc {r} outside its scale"
                );
            }
        }
    }
}

#[test]
fn every_day_template_chord_carries_a_third_and_seventh() {
    // the shell voicings are only well-defined over true 7th chords
    for progs in [&DAY_PROGRESSIONS[..]] {
        for (i, p) in progs.iter().enumerate() {
            for (bar, chord) in p.chords.iter().enumerate() {
                let root = p.roots_pc[bar];
                let has = |offs: [u8; 2]| {
                    chord
                        .iter()
                        .any(|&n| offs.iter().any(|&o| n % 12 == (root + o) % 12))
                };
                assert!(has([3, 4]), "template {i} bar {bar}: no third");
                assert!(has([10, 11]), "template {i} bar {bar}: no seventh");
            }
        }
    }
}

#[test]
fn lead_voice_pools_are_mood_curated() {
    let mut day_saw = (false, false, false);
    let mut night_saw = (false, false);
    for seed in 0..SWEEP {
        match compose(Mood::Day, seed).lead_voice {
            LeadVoice::EpVel => day_saw.0 = true,
            LeadVoice::Pluck => day_saw.1 = true,
            LeadVoice::Kalimba => day_saw.2 = true,
            LeadVoice::Vibraphone => panic!("seed {seed}: vibraphone is night-pool only"),
        }
        match compose(Mood::Night, seed).lead_voice {
            LeadVoice::EpVel => night_saw.0 = true,
            LeadVoice::Vibraphone => night_saw.1 = true,
            v => panic!("seed {seed}: night drew the day-pool {v:?}"),
        }
    }
    assert!(
        day_saw.0 && day_saw.1 && day_saw.2,
        "all three day lead voices must appear: {day_saw:?}"
    );
    assert!(
        night_saw.0 && night_saw.1,
        "both night lead voices must appear: {night_saw:?}"
    );
}

#[test]
fn lead_voice_distribution_tracks_the_draw_weights() {
    // day p(Pluck)=0.35 / p(Kalimba)=0.25, night p(Vibraphone)=0.35; each band
    // is ±3.5σ over 400 seeds, so a biased or misplaced draw fails but an
    // unlucky sample does not
    let (mut plucks, mut kalimbas, mut vibes) = (0usize, 0usize, 0usize);
    for s in 0..400 {
        match compose(Mood::Day, s).lead_voice {
            LeadVoice::Pluck => plucks += 1,
            LeadVoice::Kalimba => kalimbas += 1,
            _ => {}
        }
        if compose(Mood::Night, s).lead_voice == LeadVoice::Vibraphone {
            vibes += 1;
        }
    }
    assert!(
        (107..=173).contains(&plucks),
        "pluck drew {plucks}/400 vs p=0.35"
    );
    assert!(
        (70..=130).contains(&kalimbas),
        "kalimba drew {kalimbas}/400 vs p=0.25"
    );
    assert!(
        (107..=173).contains(&vibes),
        "vibe drew {vibes}/400 vs p=0.35"
    );
}

#[test]
fn compose_is_deterministic() {
    for mood in [Mood::Day, Mood::Night] {
        for seed in 0..8 {
            assert_eq!(compose(mood, seed), compose(mood, seed));
        }
    }
}

#[test]
fn every_seed_is_well_formed_day() {
    for seed in 0..SWEEP {
        let s = compose(Mood::Day, seed);
        assert_well_formed(&s, seed);
        assert!(
            (DAY_BPM.0..=DAY_BPM.1).contains(&s.bpm),
            "seed {seed}: bpm {}",
            s.bpm
        );
        assert!(
            s.drums.iter().any(|&(_, k, _)| k == DrumKind::Snare),
            "seed {seed}: a day take carries a backbeat"
        );
        assert!(s.kick_times.is_empty());
        for &(_, note, _) in &s.sparkle {
            assert!(
                (DAY_LEAD_LO..=DAY_LEAD_HI).contains(&note),
                "seed {seed}: lead note {note} out of the day register"
            );
        }
    }
}

#[test]
fn every_seed_is_well_formed_night() {
    for seed in 0..SWEEP {
        let s = compose(Mood::Night, seed);
        assert_well_formed(&s, seed);
        assert!(
            (NIGHT_BPM.0..=NIGHT_BPM.1).contains(&s.bpm),
            "seed {seed}: bpm {}",
            s.bpm
        );
        assert!(
            s.drums
                .iter()
                .all(|&(_, k, _)| matches!(k, DrumKind::Kick | DrumKind::Hat)),
            "seed {seed}: night grew a snare/open hat"
        );
        let mut kicks: Vec<f32> = s
            .drums
            .iter()
            .filter(|&&(_, k, _)| k == DrumKind::Kick)
            .map(|&(at, _, _)| at)
            .collect();
        kicks.sort_by(f32::total_cmp);
        let mut kt = s.kick_times.clone();
        kt.sort_by(f32::total_cmp);
        assert_eq!(kicks, kt, "seed {seed}: kick_times desynced from drums");
        for &b in &s.bass_roots {
            assert!(
                (26..=38).contains(&b),
                "seed {seed}: bass {b} out of window"
            );
            assert!(
                s.scale_pcs.contains(&(b % 12)),
                "seed {seed}: bass pc off-scale"
            );
        }
        for &(_, note, _) in &s.sparkle {
            assert!(
                (NIGHT_LEAD_LO..=NIGHT_LEAD_HI).contains(&note),
                "seed {seed}: lead note {note} out of the night register"
            );
        }
    }
}

fn assert_well_formed(s: &GeneratedScore, seed: u64) {
    let loop_s = s.loop_secs();
    let bar_s = s.bar_s();
    for &(at, _, vel) in s.sparkle.iter().chain(s.keys.iter()).chain(s.bass.iter()) {
        assert!(
            at >= 0.0 && at < loop_s,
            "seed {seed}: event at {at} outside loop {loop_s}"
        );
        assert!(vel > 0.0 && vel <= 1.0, "seed {seed}: velocity {vel}");
    }
    assert!(
        s.bass.len() >= GEN_LOOP_BARS,
        "seed {seed}: the bass must anchor every bar's ONE"
    );
    for &(at, note, _) in &s.bass {
        // 25 admits the leading-tone pickup one step under the root window
        assert!(
            (25..=38).contains(&note),
            "seed {seed}: bass note {note} at {at}s out of the sub window"
        );
    }
    for &(at, _, gain) in &s.drums {
        assert!(
            at >= 0.0 && at < loop_s,
            "seed {seed}: drum at {at} outside loop"
        );
        assert!(gain > 0.0 && gain <= 1.5, "seed {seed}: drum gain {gain}");
    }
    for &(at, note, _) in &s.keys {
        let bar = ((at / bar_s) as usize).min(GEN_LOOP_BARS - 1);
        let chord = s.bar_chords[bar];
        let ninth_pc = (s.bar_roots[bar] + 2) % 12;
        assert!(
            chord
                .iter()
                .any(|&c| note == c || note == c + 12 || note == c + 24)
                || note % 12 == ninth_pc,
            "seed {seed}: keys note {note} at {at}s not a tone/ninth of {chord:?}"
        );
    }
    for &(at, note, _) in &s.sparkle {
        let bar = ((at / bar_s) as usize).min(GEN_LOOP_BARS - 1);
        let in_turnaround = s.bar_chords[bar].iter().any(|&c| note % 12 == c % 12);
        assert!(
            s.scale_pcs.contains(&(note % 12)) || in_turnaround,
            "seed {seed}: lead note {note} at {at}s outside key AND bar chord"
        );
    }
    assert!(
        s.sparkle.len() >= 2,
        "seed {seed}: the lead lost its identity"
    );
    let max_per_bar = match s.mood {
        Mood::Day => 3,
        Mood::Night => 1,
    };
    for bar in 0..GEN_LOOP_BARS {
        let n = s
            .sparkle
            .iter()
            .filter(|&&(at, _, _)| {
                let b = (at / bar_s) as usize;
                b == bar
            })
            .count();
        // the humanization lag can push a bar-boundary event into the
        // next bar's count, so allow one over
        assert!(
            n <= max_per_bar + 1,
            "seed {seed}: bar {bar} lead density {n} > {max_per_bar}"
        );
    }
    let color_tones = s
        .chords
        .iter()
        .flatten()
        .filter(|&&n| !s.scale_pcs.contains(&(n % 12)))
        .count();
    assert!(
        color_tones <= 4,
        "seed {seed}: {color_tones} color tones — more than one chromatic chord"
    );
    // bar 7 is the turnaround, so only bars 0-6 must match the template
    for bar in 0..7 {
        assert_eq!(
            s.bar_chords[bar],
            s.chords[bar % 4],
            "seed {seed}: bar {bar} drifted from the template"
        );
    }
}

#[test]
fn day_bar8_is_the_turnaround_dominant_and_night_is_not() {
    for seed in 0..SWEEP {
        let d = compose(Mood::Day, seed);
        // the hinge: a dominant 7th of the returning bar-1 root
        let dom_pc = (d.roots_pc[0] + 7) % 12;
        let want: Vec<u8> = [0u8, 4, 7, 10].iter().map(|o| (dom_pc + o) % 12).collect();
        let got: Vec<u8> = d.bar_chords[7].iter().map(|&n| n % 12).collect();
        assert_eq!(got, want, "seed {seed}: bar 8 is not V7 of the loop root");
        assert_eq!(d.bar_roots[7], dom_pc);
        let n = compose(Mood::Night, seed);
        assert_eq!(
            n.bar_chords[7], n.chords[3],
            "seed {seed}: night must keep the plain cycle (the sleepy loop)"
        );
    }
}

#[test]
fn day_keys_comp_in_rolled_shell_pairs() {
    for seed in 0..SWEEP {
        let s = compose(Mood::Day, seed);
        assert_eq!(s.keys.len() % 2, 0, "seed {seed}: unpaired shell note");
        for pair in s.keys.chunks(2) {
            let (a, b) = (pair[0], pair[1]);
            let roll = b.0 - a.0;
            assert!(
                (0.010..=0.022).contains(&roll),
                "seed {seed}: shell roll {roll}s outside the hand's range"
            );
            assert!(
                b.2 < a.2,
                "seed {seed}: the rolled upper voice rides softer"
            );
            assert_ne!(a.1 % 12, b.1 % 12, "seed {seed}: shell voices collapsed");
        }
    }
}

#[test]
fn day_lead_uses_sixteenth_anticipations_somewhere_in_the_sweep() {
    let mut seen_push = false;
    for seed in 0..SWEEP {
        let s = compose(Mood::Day, seed);
        let beat_s = s.beat_s();
        for &(at, _, _) in &s.sparkle {
            // strip the played-not-sequenced lag before reading the grid
            let beats = (at - 0.015) / beat_s;
            let frac = beats - beats.floor();
            let on_grid = [0.0f32, 0.25, 0.5, 0.75]
                .iter()
                .any(|g| (frac - g).abs() < 0.07 || (frac - g - 1.0).abs() < 0.07);
            assert!(on_grid, "seed {seed}: lead event off-grid (frac {frac})");
            let bar_beat = beats % 4.0;
            if [0.75f32, 1.75, 2.75]
                .iter()
                .any(|g| (bar_beat - g).abs() < 0.07)
            {
                seen_push = true;
            }
        }
    }
    assert!(seen_push, "no 16th anticipation surfaced across the sweep");
}

#[test]
fn the_answer_quotes_the_statements_opening() {
    for seed in 0..SWEEP {
        let s = compose(Mood::Day, seed);
        let bar_s = s.bar_s();
        let bar_of = |at: f32| ((at / bar_s) as usize).min(GEN_LOOP_BARS - 1);
        let bar0: Vec<u8> = s
            .sparkle
            .iter()
            .filter(|&&(at, _, _)| bar_of(at) == 0)
            .map(|&(_, n, _)| n)
            .collect();
        let bar4: Vec<u8> = s
            .sparkle
            .iter()
            .filter(|&&(at, _, _)| bar_of(at) == 4)
            .map(|&(_, n, _)| n)
            .collect();
        if !bar0.is_empty() && !bar4.is_empty() {
            assert_eq!(
                bar4[0], bar0[0],
                "seed {seed}: the answer must open on the statement's pitch"
            );
        }
    }
}

#[test]
fn adopted_color_templates_actually_rotate_in() {
    let mut seen_color = false;
    for seed in 0..SWEEP {
        let s = compose(Mood::Day, seed);
        if s.chords
            .iter()
            .flatten()
            .any(|&n| !s.scale_pcs.contains(&(n % 12)))
        {
            seen_color = true;
        }
    }
    assert!(seen_color, "no chromatic template drawn across the sweep");
}

#[test]
fn night_hats_articulate_above_the_v1_floor() {
    for seed in 0..SWEEP {
        let s = compose(Mood::Night, seed);
        for &(_, k, g) in s.drums.iter().filter(|&&(_, k, _)| k == DrumKind::Hat) {
            let _ = k;
            assert!(
                g >= 0.40 * 0.9 * 0.95,
                "seed {seed}: hat gain {g} below the adopted floor"
            );
        }
    }
}
