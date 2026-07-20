//! The seed-sweep property suite — the composer's quality contract.
//! Frozen takes pin ONE realization (checksums); a generator pins the
//! RULES: any seed must be musically well-formed. The suite sweeps a
//! seed range so a constraint regression fails fast, not on the one
//! unlucky hour a user hits.

use super::*;

const SWEEP: u64 = 64;

#[test]
fn template_chords_and_roots_are_diatonic() {
    for (name, progs) in [
        ("day", &DAY_PROGRESSIONS[..]),
        ("night", &NIGHT_PROGRESSIONS[..]),
    ] {
        for (i, p) in progs.iter().enumerate() {
            for chord in &p.chords {
                for &n in chord {
                    assert!(
                        p.scale_pcs.contains(&(n % 12)),
                        "{name}[{i}]: chord tone {n} outside its scale"
                    );
                }
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
        assert!(s.bass_roots.is_none() && s.kick_times.is_empty());
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
        // the sleepy register: kick + closed hat only, never a backbeat
        assert!(
            s.drums
                .iter()
                .all(|&(_, k, _)| matches!(k, DrumKind::Kick | DrumKind::Hat)),
            "seed {seed}: night grew a snare/open hat"
        );
        // the texture duck rides EXACTLY the kick timestamps
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
        // the sub floor: in the ratified window, diatonic, root-true
        let roots = s.bass_roots.expect("night carries the sub floor");
        for &b in &roots {
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

/// The shared well-formedness core: in-loop, in-key, chord-tone comping,
/// density bounds, a non-empty lead.
fn assert_well_formed(s: &GeneratedScore, seed: u64) {
    let loop_s = s.loop_secs();
    let bar_s = s.bar_s();
    for &(at, _, vel) in s.sparkle.iter().chain(s.keys.iter()) {
        assert!(
            at >= 0.0 && at < loop_s,
            "seed {seed}: event at {at} outside loop {loop_s}"
        );
        assert!(vel > 0.0 && vel <= 1.0, "seed {seed}: velocity {vel}");
    }
    for &(at, _, gain) in &s.drums {
        assert!(
            at >= 0.0 && at < loop_s,
            "seed {seed}: drum at {at} outside loop"
        );
        assert!(gain > 0.0 && gain <= 1.5, "seed {seed}: drum gain {gain}");
    }
    // comping draws from the chord pool: strict chord tones ±octaves
    for &(at, note, _) in &s.keys {
        let chord = s.chord_at_bar((at / bar_s) as usize);
        assert!(
            chord
                .iter()
                .any(|&c| note == c || note == c + 12 || note == c + 24),
            "seed {seed}: keys note {note} at {at}s not a tone of {chord:?}"
        );
    }
    // the lead lives in the take's key
    for &(at, note, _) in &s.sparkle {
        assert!(
            s.scale_pcs.contains(&(note % 12)),
            "seed {seed}: lead note {note} at {at}s outside the key"
        );
    }
    // density: a lead phrase, not a solo — and never silence
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
    // every chord tone diatonic post-transpose (grammar survived the shift)
    for chord in &s.chords {
        for &n in chord {
            assert!(
                s.scale_pcs.contains(&(n % 12)),
                "seed {seed}: chord tone {n} off-scale"
            );
        }
    }
}
