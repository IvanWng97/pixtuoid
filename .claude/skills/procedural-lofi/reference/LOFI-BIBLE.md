# pixtuoid Lofi Production Bible (synthesis of 5 research reports)

Report tags: [H]=harmony, [R]=rhythm, [SD]=sound-design, [M]=mixing, [G]=generative. Ratified rule applied throughout: **where literature and our measured Lofi Girl fingerprints disagree, the measurement wins.**

---

## 1. RULES — note-table generator (harmony / melody / form)

Each rule → the pixtuoid parameter it maps to.

- **R1. Never emit a plain triad; every chord carries a 7th** (the genre marker). → chord-interval table rows always include interval 10 or 11. [H, orphiq.com/resources/lofi-chord-progressions]
- **R2. Quality palette**: major degrees → maj7/maj9 {0,4,7,11,(14)}; minor → m7/m9/m11 {0,3,7,10,(14),(17)}; V → 7/9/7#5/7b9#5; color → dim7 {0,3,6,9}, m7b5 {0,3,6,10}. → interval-set table. [H]
- **R3. Progression templates, weighted**: 40% four-chord turnarounds (`Imaj7–vi7–ii7–V7`, `ii7–V7–Imaj7`, `IVmaj7–iii7–vi7–V7`), 35% two-chord loops (`Imaj9–IVmaj13`, `i9–bVIImaj7`, `i9–bii m7`, `i11–viidim7`), 25% chromatic color (`Imaj7–bVII7–IVmaj7–iv7`, planing). → progression table + weights. [H; anchor: Nujabes "Aruarian Dance" = ii7–iii7–V7–Imaj7, hooktheory.com/theorytab/view/nujabes/aruarian-dance]
- **R4. One chord/bar, 4-bar loop; 8-bar cycle = loop×2 with bar-8 tension substitute (V7alt or dim7) resolving to bar 1.** → bar counter + substitution pass. [H, richardpryn.com/13-lofi-chord-progressions/]
- **R5. Keys {C, F, G major; D, A, C minor}, ~50:50 major:minor** — roots land D1–G2 comfortably for the sub-heavy fingerprint; minor loops allow dorian ♮6 in melody at low weight. → key table. [H]
- **R6. Extension roll**: 7th always; 9th 60%; 11th 30% (minor chords only); 13th 15% (IV/dominant). → per-chord extension probability. [H §7]
- **R7. Register map (MIDI)**: bass root **36–47, alone** (no interval <P5 below 48 — low-interval limit); shell 3rd+7th **48–60**, spread ≥P4; extensions **60–72**; melody **72–84** (one octave). Drop the 5th when >4 voices. → note-table register clamps. [H, robin-hoffmann.com/dfsb/low-interval-limits/, pianogroove.com shell voicings]
- **R8. Nearest-voicing voice-leading**: minimize total semitone motion between adjacent chords; invert so bass moves stepwise (±2 semitones) where possible. → voicing selector cost function. [H]
- **R9. Melody pool = pentatonic** (+ maj7 color tone on major; dorian 6 on minor); chord tones (3rd/7th preferred over root) on strong beats, extensions on weak. → melody note table, chord-relative. [H]
- **R10. Melody form**: 2-bar phrase = A + A′ (exactly one note or rhythm cell mutated); 4-bar period = call (ends off-tonic) + response (ends on chord tone); density 2–4 onsets/bar. → phrase generator. [H]
- **R11. Harmony is static for the whole track; the arrangement IS the busy-ness stem mixer** — sections 20–30s, drums drop every 3rd–4th section, ABABB macro form. → stem-gate scheduler (already exists). [H, richardpryn.com/lofi-music-structure/]
- **R12. Whole chords land 10–30 ms behind the grid** (composes with swing offsets). → per-stem lag offset. [H]
- **R13. Melody rhythm duration prior: 65% 8ths / 25% 16ths / 5% quarters / 5% halves.** → duration table. [G, mtsandra.github.io/blog/2023/lofi-generator/]

## 2. GROOVE numbers

- **Swing formula**: delay every even 16th by `(s − 0.5) × 8th-duration`. At 75 BPM (8th = 400 ms): 54%→+16 ms, 58%→+32 ms, 62%→+48 ms. [R, Roger Linn via brettworks.com]
- **Differential swing** (the Dilla move): hats **56–62%**, kick/snare **52–54%**. Single-knob fallback: 54–58%. [R, musicproductionwiki.com; SD gearspace Dilla thread agrees at 53–57%]
- **Lag bias** (element friction, not randomness): hats stay quantized; kick/snare dragged **+10–30 ms** late; alternative cheap version = whole-track delays, clap +20 ms / hats +35 ms. [R, grokipedia.com/page/Dilla_Time; splice.com/blog/lo-fi-beat-origin-sound/]
- **Jitter**: ±5–15 ms uniform per hit; snare biased late (never early). [R, lockah.net]
- **Velocity (0–127)**: kick 115–127, jitter ±0–5%, inter-bar wobble 15–20 units; snare backbeat 100–115; hats accents 110–127 / off-8ths 70–90 / fill-16ths 40–60, jitter ±10–15%; ghosts 25–50% (≈32–64; [R] says 38–64, [M] says 20–40 — use 32–64 for snare ghosts, 20–40 for the quietest kick ghosts). [R beatkitchen.io; M]
- **Patterns (16-step)**: boom-bap kick steps 1,7,11 + ghost 16th before snare; snare 5,13; hats straight 8ths. Sleepy/half-time: kick step 1, snare step 9 (rimshot/brush), quarter-note hats vel ≤70. [R, blog.native-instruments.com/what-is-boom-bap/; transmissionsamples.com]
- **Onset budget: 1.2–2/s total (MEASURED — prefer)**. Full boom-bap computes to 2.5–3/s → hats mixed low and few ghosts, or half-time. [R derivation vs our fingerprint]
- **BPM: 68–90 (MEASURED — prefer)**, center 74–80 (literature sweet spot sits inside our band). [R midimighty.com; M]
- **Sidechain, two regimes**: transparent duck = attack 0–2 ms, release 50–100 ms, 4:1, **2–4 dB GR**; audible sway (sleepy states) = same attack, tempo-synced release ≈1/4 note (200–400 ms audible recovery), **3–6 dB GR**, never >6–8. [R, edmprod.com/sidechain-compression/; sonarworks.com]

## 3. SOUND targets per voice

**Pad** — extensions/upper-shell register (MIDI 60–72+). Slow attack / long release AR; dual-voice detune **±5–10 cents**. Brickwall LPF ~2–4 kHz; HPF ~140 Hz. Primary sidechain-pump target. Loop length incommensurate with all other stems (see §4). [M fractal forum detune; SD modeaudio.com]

**Bass** — sine + tanh saturation for audible harmonics (the sub carries power, harmonics carry audibility — matches our <62 Hz fingerprint). Root-only below MIDI 48; line restricted to root/5th/leading-tone. Dead center, mono. Soft attack; ducked by kick (kick wins the transient, sub swells behind). [SD edmprod.com/lofi-hip-hop/; G devpost lofigen; M joeysturgistones.com]

**Keys (EP pluck)** — the genre's core voice. Steal the Rhodes physics: **displacement/velocity-dependent asymmetric waveshaping that raises the 2nd harmonic** as velocity rises = "bell vs bark" (conforg.fr/isma2014 paper); very short attack transient. Register 48–72. HPF 60–100 Hz, brickwall LPF ~2 kHz; **notch 200–700 Hz if low notes ring** (the documented procedural-lofi failure). Chords 10–30 ms late. Optional: 180° stereo-opposed sine tremolo at 1/4–1/2-note rate; felt variant = quieter attack partials + per-note filtered-noise thump layer. [SD; G devpost; SD spectrasonics Keyscape manual]

**Sparkle (melody)** — MIDI 72–84, pentatonic, 2–4 onsets/bar. Fast attack / medium decay AR. **Bitcrush this layer (8–12 bit)** — the top layer is where the crunch reads. Any reverb return HPF ≥300 Hz. [G Sonic Pi kalimba-through-bitcrusher recipe, in-thread.sonic-pi.net; SD unison.audio; voxbooster.com]

**Drums** — 12-bit / ~26 kHz sample-rate-reduction reference (SP-1200, en.wikipedia.org/wiki/E-mu_SP-1200); saturate **then** LPF 3–6 kHz; bus HPF ~100 Hz; **through the tape chain, not around it** (sat 25–30%, wow/flutter ~15%). Kick = sub boom + mid punch; **lengthening the kick attack envelope is a groove parameter** ("slurped"). Snare tight/clap-like, distorted then filtered; brush/rim for sleepy states, −6 dB further back. [R native-instruments, attackmagazine.com, unison.audio]

**Texture (crackle/hiss/foley)** — crackle bed **−24 to −18 dB below master, HPF ~1 kHz** (hiss peaks 4–8 kHz, must sit *on top*, not as broadband mud). Per-click spectral variation (constant-timbre clicks read as static — arxiv.org/pdf/2206.06259); density = a "record age" param, a few clicks/s. **Sidechain-duck the crackle keyed off kick+snare**; automate it up in sparse sections. Foley (rain/café) sparse and section-signaling only; the one published ratio: rain ≈ −2.5 dB, café ≈ −7.6 dB vs music (sleep-video levels — in-track sit well below). Crackle doubles as room tone masking loop seams. [M selektaudio.com; SD beatproduction.net, songnara.com]

**Tape chain (shared)** — wow **0.5–2 Hz** (Goodhertz default 0.55 Hz/15%), flutter **6–20 Hz**, randomized "flux" rate/depth + wow→flutter cross-modulation (the difference between vibrato and tape), **stereo-opposed per channel** for width; audible warble = depth >~0.1–0.2%, broken-walkman ≈0.5–2%. Portable physics if wanted: Chowdhury DAFx-19 loss FIR + Jiles-Atherton constants (ccrma.stanford.edu/~jatin/420/tape/TapeModel_DAFx.pdf, GPL impl — port the math, not the code). [SD; M goodhertz manual]

## 4. MIX targets (measurement-reconciled)

- **Band shares: 55–65% of power below 62 Hz, centroid 140–160 Hz — the ratified target (MEASURED).** Literature is directionally consistent (AES 8960: ~5 dB/oct LTAS decay from 100 Hz puts ~half of power below 62 Hz; Trackscore: 81% below 250 Hz for electronic) but generic-EDM sub shares run lower — **prefer the measurement**; the low end is also the least-standardized region (10 dB σ between pro masters, DiVA thesis), which is exactly why reference-matching beats curve-matching. [M]
- **Enforce by subtraction, not boost**: HPF every non-bass stem ~140 Hz; HPF master at 30 Hz; sub register belongs to the bass stem alone (R7). [M lofiweekly.com]
- **Head-bump conflict — flagged**: our chain bumps 80–120 Hz; literature pins 15 ips ≈ 60 Hz / 30 ips ≈ 120 Hz. A ~60 Hz bump feeds the measured <62 Hz share directly; 80–120 Hz feeds the band *above* it. The fingerprint (the reference's output) outranks both → **retune the bump toward ~60 Hz** (or widen 60–120) and re-measure against the fingerprint. [SD help.uaudio.com Oxide manual; M]
- **6.5 kHz rolloff: keep.** Real cassette rolls off 10–12 kHz — ours is deliberately darker, matching genre practice, the measured centroid, and anti-fatigue (2–5 kHz is the ear's fatigue band). [SD ultraferric.com; M babyaud.io]
- **Mud guard**: the most common defect is excess 250–500 Hz, not missing bass; keys notch 200–700 Hz on ringing. Confine tanh saturation to 100 Hz–10 kHz (low-end intermodulation guard). [M trackscore.ai; G; M masteringthemix.com]
- **Bus order & glue**: corrective EQ → **glue comp (1.5–2:1, attack 10–30 ms, release ~150 ms, ≤2 dB GR, total chain <4 dB)** → tape chain (sat/wow/flutter) → limiter. [M audiospectra.net]
- **Loudness**: **−16 to −14 LUFS integrated, ≤−1 dBTP**, no heavy limiting — an office bed plays under speech and quieter/more dynamic than a release master; preserved dynamics are themselves anti-fatigue. [M veniamastering.studio, widenisland.com]
- **Stereo**: sub + kick dead center; **side-channel HPF ~150 Hz @ 6 dB/oct** (vinyl elliptical EQ — a slope, not a brickwall); width comes from the top: stereo-opposed wow/flutter, 180° EP autopan, slightly-panned duplicate noise beds. [SD gearspace mastering thread, flotownmastering.com]
- **Anti-repetition**: stem loop lengths mutually **incommensurate** (Eno's 23½ s / 25⅞ s / 29 15/16 s model — never re-align); texture layers get ±2% pitch / ±3 dB per-instance variation; QA gate = **listen ≥4 repetitions** (the brain catches the seam on rep 3–4). [M reverbmachine.com, soundcy.com, audioedit.io]

## 5. GENERATIVE lessons (prior art)

- **The #1 repeated failure: naive Markov/stochastic harmony is directionless and never resolves.** Two independent authors hit it and abandoned it; fixed **jazz-turnaround grammar + substitution operators** (secondary dominants; base strings I–vi–ii–V, ii–V–I–vi) won. If stochastic flavor is ever wanted: forward-chain + backward-chain-from-resolution joined at a pivot. Our §1 ruleset already encodes the winning design. [G devpost.com/software/lofi-generator; medium.com/@w.patrick.kelly]
- **Chord-relative melody encoding** (jacbz/Lofi, Apache-2.0): melody as scale degrees 0–15 against the *current chord*, 0=rest — quantizing to the chord's scale is the trick that keeps generated melodies consonant. Adopt the schema (Roman-numeral chord ints 0–8, valence/energy conditioning scalars map to busy-ness). [G github.com/jacbz/Lofi]
- **Structure ceiling is low, deliberately**: best prior art tops out at 32-bar AABA with per-section stem add/drop; Magenta chose lofi *because* constrained structure makes "always sounds acceptable" achievable. Our busy-ness stem gating already exceeds prior art — **adding degrees of freedom is the main way these systems start sounding wrong.** [G magenta.withgoogle.com/lofi-player]
- **Static FX beat automated FX**: the Devpost author abandoned algorithmic effect automation as sounding worse than fixed settings — keep the tape chain parameters static per track, vary per seed. [G]
- **The hard bugs are mix gremlins, not note choice** (low-piano ringing → the 200–700 Hz notch; missing transduction noise). Musicology confirms rolled-off highs + hiss/crackle ARE the genre's authenticity signal — our tape chain + crackle bed is the load-bearing layer, not garnish. [G Winston & Saywood 2019, IASPM; Neal, Organised Sound]
- **Licenses**: safe to port — jacbz/Lofi (Apache-2.0), magenta/lofi-player (Apache-2.0, archived), **sharp11-jza** (BSD-2; probabilistic jazz-harmony automaton trained on the iRb 1,000+-standards corpus — the one shippable empirical transition grammar; port the automaton, not the JS), pieces-alex-bainter (MIT). **Do NOT bake tables derived from the Hooktheory dataset (CC BY-NC-SA, NonCommercial) or wayne391/lead-sheet-dataset (academic-only) into pixtuoid.** ChowTapeModel repo is GPL — reuse the paper's math only. [G]

---

**Single most actionable deltas vs the current engine**: (1) retune the head bump toward ~60 Hz and re-fingerprint; (2) add differential swing (hats 56–62% vs kick/snare 52–54%) + the +10–30 ms kick/snare drag; (3) make the four stem loop lengths incommensurate; (4) velocity-keyed even-harmonic waveshaping on the EP pluck; (5) sidechain-duck the crackle bed off kick+snare; (6) glue comp (1.5:1/20 ms/150 ms/≤2 dB) ahead of the tanh stage; (7) enforce register clamps + the 140 Hz non-bass HPF so the measured 55–65%-below-62 Hz share is structural, not accidental.
