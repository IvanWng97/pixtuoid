use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::grid::Grid;
use crate::sprite::{
    Frame, IndexedFrame, Palette, PaletteIndex, Pixel, Rgb, Sprite, PALETTE_CAPACITY,
};

/// Parse a `.sprite` text file. Returns one Frame per `@frame N` block.
pub fn parse_sprite_file(src: &str, palette: &Palette) -> Result<Vec<Frame>> {
    let pixels = palette.resolved();
    Ok(parse_indexed(src, palette)?
        .iter()
        .map(|f| f.resolve(&pixels))
        .collect())
}

fn parse_indexed(src: &str, palette: &Palette) -> Result<Vec<IndexedFrame>> {
    let mut frames: Vec<IndexedFrame> = Vec::new();
    let mut current: Option<Vec<Vec<PaletteIndex>>> = None;
    let mut last_lineno = 0;

    for (lineno, raw) in src.lines().enumerate() {
        let line = strip_comment_and_trim(raw);
        if line.is_empty() {
            continue;
        }
        last_lineno = lineno;

        if let Some(rest) = line.strip_prefix("@frame") {
            if let Some(rows) = current.take() {
                frames.push(rows_to_frame(rows).map_err(|e| anyhow!("{e} (line {})", lineno + 1))?);
            }
            let _ = rest
                .trim()
                .parse::<u32>()
                .map_err(|_| anyhow!("@frame requires a number (line {})", lineno + 1))?;
            current = Some(Vec::new());
            continue;
        }

        let rows = current
            .as_mut()
            .ok_or_else(|| anyhow!("pixel data before any @frame (line {})", lineno + 1))?;

        let row = parse_row(line, palette).map_err(|e| anyhow!("{e} (line {})", lineno + 1))?;
        rows.push(row);
    }

    if let Some(rows) = current.take() {
        frames.push(rows_to_frame(rows).map_err(|e| anyhow!("{e} (line {})", last_lineno + 1))?);
    }

    if frames.is_empty() {
        bail!("sprite file contains no frames");
    }
    Ok(frames)
}

fn strip_comment_and_trim(line: &str) -> &str {
    let line = match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    };
    line.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_value_rejects_sign_prefixed_or_non_hex() {
        assert!(parse_palette_value("#+f0102").is_err());
        assert!(parse_palette_value("#-f0102").is_err());
        assert!(parse_palette_value("#abXY12").is_err());
        assert!(parse_palette_value("#Ff0102").unwrap().is_some());
        assert!(parse_palette_value("transparent").unwrap().is_none());
    }

    #[test]
    fn final_frame_width_error_carries_line_context() {
        let mut pal = Palette::new();
        pal.insert('X', Some(Rgb { r: 1, g: 1, b: 1 }));
        let src = "@frame 0\nX X\nX\n";
        let err = parse_sprite_file(src, &pal).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("line"),
            "final-frame parse error needs line context: {msg}"
        );
    }

    fn ramp_pack(palette: &str, ramps: &str, sprite: &str) -> Result<Pack> {
        let toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n{palette}\n\
             [ramps]\n{ramps}\n\
             [animations.seated]\nframes=[\"f.sprite\"]\nframe_ms=100\n"
        );
        load_pack_from_strings(&toml, &[("f.sprite", sprite)])
    }

    const HAIR: Rgb = Rgb {
        r: 40,
        g: 20,
        b: 10,
    };

    #[test]
    fn ramp_keys_draw_as_a_step_of_their_base() {
        let pack = ramp_pack(
            "\"H\"=\"#28140a\"",
            "\"h\" = { of = \"H\", level = -1 }",
            "@frame 0\nH h",
        )
        .expect("pack builds");
        let frame = &pack.animation("seated").expect("anim").frames()[0];
        assert_eq!(frame.get(0, 0).copied().flatten(), Some(HAIR));
        assert_eq!(frame.get(1, 0).copied().flatten(), Some(HAIR.ramp(-1)));
    }

    /// `X` shares `B`'s color and stays; `h` is never named and follows `H`;
    /// `.` is transparent and stays so.
    #[test]
    fn a_recolor_replaces_keys_not_colors() {
        let pack = ramp_pack(
            "\"B\"=\"#2e62cf\"\n\"X\"=\"#2e62cf\"\n\"H\"=\"#28140a\"\n\".\"=\"transparent\"",
            "\"h\" = { of = \"H\", level = -1 }",
            "@frame 0\nB X h .",
        )
        .expect("pack builds");
        let seated = pack.animation("seated").expect("anim");
        let (red, blond) = (
            Rgb { r: 200, g: 0, b: 0 },
            Rgb {
                r: 200,
                g: 160,
                b: 80,
            },
        );
        let out = seated.recolorable(0).expect("frame 0").recolored(&[
            ('B', Some(red)),
            ('H', Some(blond)),
            ('.', Some(red)),
            ('Q', None),
        ]);
        let shirt = pack.palette().get('B').flatten();
        assert_eq!(
            out.as_slice(),
            &[Some(red), shirt, Some(blond.ramp(-1)), None][..]
        );
        assert!(seated.recolorable(1).is_none());
        assert_eq!(
            seated.frames()[0].as_slice(),
            &[shirt, shirt, Some(HAIR.ramp(-1)), None][..],
            "the pack's own colors are untouched"
        );
    }

    #[test]
    fn a_ramp_may_step_as_far_as_the_bound_either_way() {
        for level in [MAX_RAMP_LEVEL, -MAX_RAMP_LEVEL] {
            let ramps = format!("\"h\" = {{ of = \"H\", level = {level} }}");
            ramp_pack("\"H\"=\"#28140a\"", &ramps, "@frame 0\nh").expect(&ramps);
        }
    }

    #[test]
    fn a_ramp_rejects_a_malformed_declaration() {
        for (ramps, needle) in [
            ("\"h\" = { of = \"Q\", level = -1 }", "no opaque color"),
            ("\"h\" = { of = \"P\", level = -1 }", "no opaque color"),
            (
                "\"h\" = { of = \"H\", level = -1 }\n\"k\" = { of = \"h\", level = -1 }",
                "no opaque color",
            ),
            ("\"H2\" = { of = \"H\", level = -1 }", "one character"),
            ("\"X\" = { of = \"H\", level = -1 }", "both"),
            ("\"h\" = { of = \"H\", level = 0 }", "nonzero"),
            ("\"h\" = { of = \"H\", level = 11 }", "within"),
            ("\"h\" = { of = \"H\", level = -11 }", "within"),
            (
                "\"h\" = { of = \"H\", level = -1, typo = 1 }",
                "unknown field",
            ),
        ] {
            let err = ramp_pack(
                "\"H\"=\"#28140a\"\n\"X\"=\"#010203\"\n\"P\"=\"transparent\"",
                ramps,
                "@frame 0\nH",
            )
            .expect_err(ramps);
            assert!(format!("{err:#}").contains(needle), "{ramps}: {err:#}");
        }
    }

    /// Pack keys are untrusted, and these errors reach the terminal through
    /// `validate-pack`: a key that is an ESC or a bidi override must come out
    /// escaped, never raw.
    #[test]
    fn a_control_character_key_is_escaped_in_every_ramp_error() {
        for ramps in [
            "\"\\u001B\" = { of = \"Q\", level = -1 }",
            "\"h\" = { of = \"\\u202E\", level = -1 }",
            "\"\\u001B\\u001B\" = { of = \"H\", level = -1 }",
            "\"\\u001B\" = { of = \"H\", level = 11 }",
        ] {
            let err = ramp_pack("\"H\"=\"#28140a\"", ramps, "@frame 0\nH").expect_err(ramps);
            let msg = format!("{err:#}");
            assert!(
                !msg.contains('\u{1b}') && !msg.contains('\u{202e}'),
                "{ramps}: raw control character in {msg:?}"
            );
        }
    }

    #[test]
    fn a_palette_past_what_a_frame_can_index_is_rejected() {
        let keys: String = ('\u{100}'..)
            .take(PALETTE_CAPACITY + 1)
            .map(|k| format!("\"{k}\"=\"#010203\"\n"))
            .collect();
        let err = ramp_pack(&keys, "", "@frame 0\n\u{100}").expect_err("over capacity");
        assert!(format!("{err:#}").contains("at most"), "{err:#}");
        let fits: String = keys
            .lines()
            .take(PALETTE_CAPACITY)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(ramp_pack(&fits, "", "@frame 0\n\u{100}").is_ok());
        let err = ramp_pack(
            &fits,
            "\"h\" = { of = \"\u{100}\", level = -1 }",
            "@frame 0\n\u{100}",
        )
        .expect_err("a ramp is a key too");
        assert!(format!("{err:#}").contains("at most"), "{err:#}");
    }
}

fn parse_row(line: &str, palette: &Palette) -> Result<Vec<PaletteIndex>> {
    let mut out = Vec::new();
    for tok in line.split_whitespace() {
        let mut chars = tok.chars();
        let key = chars.next().ok_or_else(|| anyhow!("empty token"))?;
        if chars.next().is_some() {
            bail!("each pixel must be a single character (got {tok:?})");
        }
        let index = palette
            .drawable_index(key)
            .ok_or_else(|| anyhow!("unknown palette key {key:?}"))?;
        let index = PaletteIndex::try_from(index).map_err(|_| {
            anyhow!("palette key {key:?} is past the {PALETTE_CAPACITY} a frame can index")
        })?;
        out.push(index);
    }
    Ok(out)
}

fn rows_to_frame(rows: Vec<Vec<PaletteIndex>>) -> Result<IndexedFrame> {
    if rows.is_empty() {
        bail!("frame has no rows");
    }
    // Grid dims are u16: an `as u16` truncation would wrap them while `data`
    // keeps its full length, and `Grid::from_vec`'s length assert would panic
    // on pack input instead of rejecting it.
    if rows.len() > u16::MAX as usize {
        bail!("frame has {} rows (maximum {})", rows.len(), u16::MAX);
    }
    let w = rows[0].len();
    if w > u16::MAX as usize {
        bail!("frame row width {w} exceeds the maximum {}", u16::MAX);
    }
    for (i, r) in rows.iter().enumerate() {
        if r.len() != w {
            bail!(
                "inconsistent row width at row {i} (expected {w}, got {})",
                r.len()
            );
        }
    }
    let height = rows.len() as u16;
    let width = w as u16;
    let data = rows.into_iter().flatten().collect();
    Ok(IndexedFrame(Grid::from_vec(width, height, data)))
}

#[derive(Debug, Deserialize)]
struct PackToml {
    pack: PackMeta,
    /// Ordered, like `ramps`, so a pack loads the same way every time: the same
    /// indices, and the same key reported first when several are bad.
    palette: BTreeMap<String, String>,
    #[serde(default)]
    ramps: BTreeMap<String, RampToml>,
    animations: HashMap<String, AnimationToml>,
}

/// One `[ramps]` entry, loaded by [`Palette::insert_ramp`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RampToml {
    of: String,
    level: i8,
}

#[derive(Debug, Deserialize)]
struct PackMeta {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct AnimationToml {
    frames: Vec<String>,
    frame_ms: u32,
}

/// A loaded sprite pack: a named, versioned palette plus its animations.
#[derive(Debug, Clone)]
pub struct Pack {
    /// Pack name from the `[pack]` table in `pack.toml`.
    pub name: String,
    /// Pack version string from the `[pack]` table in `pack.toml`.
    pub version: String,
    palette: Arc<Palette>,
    animations: HashMap<String, Sprite>,
}

impl Pack {
    /// The palette the pack's own frames were drawn with. An animation
    /// inherited by [`merge_from`](Self::merge_from) keeps its own.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The animation registered under `key`, if the pack defines one.
    pub fn animation(&self, key: &str) -> Option<&Sprite> {
        self.animations.get(key)
    }

    /// The names of every animation in this pack.
    pub fn animation_names(&self) -> Vec<String> {
        self.animations.keys().cloned().collect()
    }

    /// The highest density any of this pack's variants is drawn at, or 1 when
    /// it ships none.
    ///
    /// A painter picks its render scale from the TERMINAL, but a variant only
    /// lands at a scale its density divides — so the scale has to be chosen
    /// knowing this. A Retina cell 17px wide makes 17 the natural scale, 17 is
    /// prime, and every variant in the pack would sit unused.
    /// Only furniture variants count: `<base>@<N>x` parses for ANY base, so a
    /// stray key in a user's pack.toml is a well-formed variant name for a
    /// piece no painter asks for.
    pub fn max_density_variant(&self) -> u16 {
        self.animations
            .keys()
            .filter(|n| is_optional_furniture_animation(n))
            .filter_map(|n| split_density_variant(n).map(|(_, d)| d))
            .max()
            .unwrap_or(1)
    }

    /// Merge OPTIONAL_FURNITURE_ANIMATIONS — and their density variants — from
    /// `base` into self. Character animations are never inherited: a robot pack
    /// must not fall back to human sprites.
    ///
    /// Driven by what `base` HAS rather than by the registry, because the
    /// density axis is open: the registry names PIECES, not the grids each may
    /// be drawn on, so enumerating variants would have to guess a ceiling.
    pub fn merge_from(&mut self, base: &Pack) {
        for (name, sprite) in &base.animations {
            if !is_optional_furniture_animation(name) {
                continue;
            }
            self.animations
                .entry(name.clone())
                .or_insert_with(|| sprite.clone());
        }
    }
}

/// Assemble a `Pack` from parsed TOML, resolving each frame's source text via
/// `get_src(frame_name)`. The path-traversal guard MUST stay inside
/// [`load_pack`]'s closure: [`load_pack_from_strings`] has no filesystem and no
/// untrusted paths to escape.
fn build_pack(parsed: PackToml, mut get_src: impl FnMut(&str) -> Result<String>) -> Result<Pack> {
    let palette = Arc::new(build_palette(&parsed.palette, &parsed.ramps)?);
    let mut animations = HashMap::new();
    for (anim_name, anim) in parsed.animations {
        let mut frames = Vec::new();
        for fname in &anim.frames {
            let src = get_src(fname)?;
            let mut decoded =
                parse_indexed(&src, &palette).with_context(|| format!("decoding {fname}"))?;
            frames.append(&mut decoded);
        }
        animations.insert(
            anim_name,
            Sprite::new(frames, Arc::clone(&palette), anim.frame_ms),
        );
    }

    Ok(Pack {
        name: parsed.pack.name,
        version: parsed.pack.version,
        palette,
        animations,
    })
}

/// Load a `Pack` from `dir/pack.toml` and its on-disk frame files, guarding
/// each frame path against directory traversal outside `dir`.
pub fn load_pack(dir: &Path) -> Result<Pack> {
    let toml_path = dir.join("pack.toml");
    let toml_src = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let parsed: PackToml =
        toml::from_str(&toml_src).with_context(|| format!("parsing {}", toml_path.display()))?;

    let canon_dir = dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", dir.display()))?;

    build_pack(parsed, |fname| {
        if Path::new(fname)
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            bail!("frame path {:?} contains '..' and is not allowed", fname);
        }
        let path = dir.join(fname);
        let canon_path = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        if !canon_path.starts_with(&canon_dir) {
            bail!("frame path {:?} escapes the pack directory", fname);
        }
        std::fs::read_to_string(&canon_path)
            .with_context(|| format!("reading {}", canon_path.display()))
    })
}

/// Same as `load_pack` but takes in-memory strings — used by binaries that
/// `include_str!` their assets at compile time.
pub fn load_pack_from_strings(pack_toml: &str, frames: &[(&str, &str)]) -> Result<Pack> {
    let parsed: PackToml = toml::from_str(pack_toml).context("parsing pack.toml")?;
    let frame_lookup: HashMap<&str, &str> = frames.iter().copied().collect();

    build_pack(parsed, |fname| {
        frame_lookup
            .get(fname)
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("missing embedded frame {fname}"))
    })
}

fn single_char(k: &str, what: &str) -> Result<char> {
    let mut it = k.chars();
    let (Some(key), None) = (it.next(), it.next()) else {
        bail!("{what} {k:?} must be exactly one character");
    };
    Ok(key)
}

/// The furthest a `[ramps]` level may step either way. Past it the darkest
/// colors stop changing from one level to the next in 8 bits.
pub const MAX_RAMP_LEVEL: i8 = 10;

fn build_palette(
    colors: &BTreeMap<String, String>,
    ramps: &BTreeMap<String, RampToml>,
) -> Result<Palette> {
    let keys = colors.len() + ramps.len();
    if keys > PALETTE_CAPACITY {
        bail!("the palette declares {keys} keys; a frame can index at most {PALETTE_CAPACITY}");
    }
    let mut palette = Palette::new();
    for (k, v) in colors {
        let key = single_char(k, "palette key")?;
        let pixel = parse_palette_value(v).with_context(|| format!("palette key {k:?}"))?;
        palette.insert(key, pixel);
    }
    for (k, ramp) in ramps {
        let key = single_char(k, "ramp key")?;
        let of = single_char(&ramp.of, "ramp `of`")?;
        if colors.contains_key(k) {
            bail!("{key:?} is declared in both [palette] and [ramps]");
        }
        // `colors`, not the palette being built, names the possible bases, so
        // an earlier-loaded ramp is never one.
        if !colors.contains_key(&ramp.of) || !matches!(palette.get(of), Some(Some(_))) {
            bail!("ramp {key:?} steps from {of:?}, which has no opaque color in [palette]");
        }
        // Zero is a second name for the base itself.
        if ramp.level == 0 || ramp.level.unsigned_abs() > MAX_RAMP_LEVEL.unsigned_abs() {
            bail!(
                "ramp {key:?} level {} must be nonzero and within ±{MAX_RAMP_LEVEL}",
                ramp.level
            );
        }
        palette.insert_ramp(key, of, ramp.level);
    }
    Ok(palette)
}

/// Character animation names every pack MUST provide.
pub const REQUIRED_CHARACTER_ANIMATIONS: &[&str] = &[
    "seated",
    "typing",
    "standing",
    "walking",
    "walking_back",
    "seated_sleeping",
    "seated_sleeping_alt",
    "holding_coffee",
    "back_couch",
];

/// Character animation names a pack MAY omit — the renderer degrades
/// gracefully (`side_seated`/`seated_back` fall back to the front `seated`).
pub const OPTIONAL_CHARACTER_ANIMATIONS: &[&str] = &[
    "walking_coffee",
    "side_seated",
    "seated_back",
    "typing_back",
];

/// Separator joining a furniture animation to the density it is drawn at:
/// `desk@4x` is the `desk` piece drawn on a 4x grid, for a painter rendering
/// at a scale where the base art would otherwise be block-upscaled.
///
/// The SCALE is in the name, following the prevailing asset convention
/// (`@2x`/`@3x` on Apple platforms, `scale-200` on Windows). A name that says
/// only "denser" cannot express a pack shipping BOTH a 2x and a 4x variant of
/// one piece, and leaves the file's meaning dependent on whichever render
/// scale happens to measure it.
pub(crate) const DENSITY_VARIANT_SEP: char = '@';

/// The animation name for `base` drawn at `density`x.
pub fn density_variant_name(base: &str, density: u16) -> String {
    let mut out = String::with_capacity(base.len() + 4);
    density_variant_name_into(&mut out, base, density);
    out
}

/// [`density_variant_name`] into a caller-owned buffer.
///
/// The lookup happens per divisor, per piece, per frame, and the key is only
/// borrowed for a map probe — so the hot path reuses one buffer rather than
/// allocating a `String` it immediately drops.
pub fn density_variant_name_into(out: &mut String, base: &str, density: u16) {
    use std::fmt::Write;
    // Writing to a String is infallible.
    let _ = write!(out, "{base}{DENSITY_VARIANT_SEP}{density}x");
}

/// The largest density a variant name may claim.
///
/// A pack author types this number, so it is untrusted input to arithmetic that
/// multiplies it by a frame dimension. `desk@60000x` parsed happily and then
/// overflowed `base.width() * density` — a panic in any debug build (the crash
/// hook fires, offering to file a bug for a pack typo) and a wrapped, wrong
/// dimension in release. Bounding it HERE fixes every downstream multiply at
/// once, and keeps a nonsense density out of `RenderScale::fit`, where
/// `(available / density) * density` would floor to zero and silently withdraw
/// the richer profile.
///
/// 64 is far past any authoring grid: the bundled art is 4x, and a 64x variant
/// of the 14px desk would already be a 896px sprite.
pub(crate) const MAX_DENSITY_VARIANT: u16 = 64;

/// The base piece and density a variant name denotes, if it is one.
///
/// `1x` is deliberately NOT a variant: it would be a second name for the base
/// piece, and one thing with two names is how a pack ends up shipping both.
pub(crate) fn split_density_variant(name: &str) -> Option<(&str, u16)> {
    let (base, density) = name.rsplit_once(DENSITY_VARIANT_SEP)?;
    let digits = density.strip_suffix('x')?;
    // Digits only. `u16::from_str` accepts a leading `+`, which would give one
    // density two spellings — and this name is a lookup KEY, so two spellings
    // is two files a renderer picks between arbitrarily.
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u16 = digits.parse().ok()?;
    (2..=MAX_DENSITY_VARIANT).contains(&n).then_some((base, n))
}

/// Whether `name` is a furniture animation a pack may provide — either a
/// registry entry or one of their density variants.
///
/// Variants are legal BY DERIVATION rather than by their own registry rows, so
/// authoring one is a sprite file and nothing else. A second list would have
/// to be kept in step with the first, and forgetting an entry fails QUIETLY in
/// its least visible direction: the variant loads for the bundled pack but
/// `Pack::merge_from` never inherits it, so a `--pack-dir` user silently drops
/// back to the upscale.
pub(crate) fn is_optional_furniture_animation(name: &str) -> bool {
    let base = split_density_variant(name).map_or(name, |(base, _)| base);
    OPTIONAL_FURNITURE_ANIMATIONS.contains(&base)
}

/// Environment/furniture animation names a pack MAY provide; `Pack::merge_from`
/// inherits any that are missing from the base pack, density variants included.
pub const OPTIONAL_FURNITURE_ANIMATIONS: &[&str] = &[
    "desk",
    "desk_north",
    "filing_cabinet",
    "plant",
    "plant_tall",
    "plant_flower",
    "plant_succulent",
    "floor_lamp",
    "door",
    "cat_walk",
    "cat_sit",
    "cat_sleep",
    "dog_walk",
    "dog_sit",
    "dog_sleep",
    "lobster_walk",
    "lobster_rest",
    "meeting_sofa",
    "meeting_screen",
    "pantry",
    "pantry_small",
    "whiteboard",
    "bookshelf",
    "snack_shelf",
    "tv_stand",
    "phone_booth",
    "standing_desk",
    "bulletin_board",
    "exit_sign",
    "desk_chair",
];

const MULTI_FRAME_REQUIREMENTS: &[(&str, usize)] = &[
    ("typing", 2),
    ("walking", 2),
    ("walking_back", 2),
    ("door", 3),
    ("cat_walk", 2),
    ("dog_walk", 2),
    ("lobster_walk", 2),
];

/// A density variant whose frame size is not what its name claims.
///
/// Worse than an absent variant: a renderer picking it up by name draws the
/// piece at the wrong size, so `validate_pack_animations` calls it an error.
///
/// `claimed != found` is what makes an instance mean anything, and it is held
/// by CONSTRUCTION: the one producer (`validate_pack_animations`) pushes only
/// inside that comparison, and nothing else in the workspace builds one. The
/// fields stay `pub` because the `validate-pack` presenter reads all three to
/// print them. Encoding the invariant in the type would mean a private
/// constructor plus three accessors for a struct with one producer and one
/// consumer — cost with no reachable failure to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DensityMismatch {
    /// The variant's animation name, e.g. `desk@4x`.
    pub name: String,
    /// The size the name claims: the base piece's, times that density.
    pub claimed: (u16, u16),
    /// The size the variant's first frame actually is.
    pub found: (u16, u16),
}

/// Per-category tally of a pack's animation discrepancies.
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Required character-animation names absent from the pack — an error.
    pub missing_required: Vec<String>,
    /// Optional animation names absent from the pack — reported, not an error.
    pub missing_optional: Vec<String>,
    /// `(name, need, have)` — REQUIRED count first — for each animation with
    /// fewer frames than its minimum.
    pub insufficient_frames: Vec<(String, usize, usize)>,
    /// Animation names present in the pack but in none of the known registries.
    pub unknown: Vec<String>,
    /// Each density variant whose frame size is not its base piece's times the
    /// density its NAME claims.
    pub mismatched_density: Vec<DensityMismatch>,
    /// Each density variant whose BASE piece the pack does not ship.
    ///
    /// The size claim is unprovable without the base, so the variant would load
    /// and then be validated against whatever the default pack supplies — an
    /// author who renamed `desk.sprite` to `desk@4x.sprite` instead of adding it
    /// otherwise gets a clean bill of health from the one tool whose job is to
    /// tell them.
    pub orphan_variants: Vec<String>,
}

impl ValidationReport {
    /// True when the pack is unusable — a required animation is missing, one
    /// has too few frames, or a density variant is not the size it claims (or
    /// has no base to claim it against). Missing OPTIONAL animations do not
    /// count.
    pub fn has_errors(&self) -> bool {
        !self.missing_required.is_empty()
            || !self.insufficient_frames.is_empty()
            || !self.mismatched_density.is_empty()
            || !self.orphan_variants.is_empty()
    }
}

/// Check a pack's animations against the required/optional/multi-frame
/// registries.
pub fn validate_pack_animations(pack: &Pack) -> ValidationReport {
    let mut report = ValidationReport::default();
    let known_names = || {
        REQUIRED_CHARACTER_ANIMATIONS
            .iter()
            .chain(OPTIONAL_CHARACTER_ANIMATIONS.iter())
            .chain(OPTIONAL_FURNITURE_ANIMATIONS.iter())
            .copied()
    };

    for &name in REQUIRED_CHARACTER_ANIMATIONS {
        if pack.animation(name).is_none() {
            report.missing_required.push(name.to_string());
        }
    }

    for &name in OPTIONAL_CHARACTER_ANIMATIONS
        .iter()
        .chain(OPTIONAL_FURNITURE_ANIMATIONS.iter())
    {
        if pack.animation(name).is_none() {
            report.missing_optional.push(name.to_string());
        }
    }

    // Implicit min-1 floor: a `frames = []` entry deserializes and makes
    // `animation()` return Some (dodging the missing-required check above)
    // while every render consumer guards with `.frames().first()` and silently
    // draws nothing; an empty OPTIONAL entry additionally SHADOWS the embedded
    // default in `Pack::merge_from` (`contains_key` is true). A density variant
    // rides its BASE's minimum — same piece, bigger grid — so an empty
    // `desk@4x` shadows the default exactly as an empty `desk` does. Variants
    // are found by walking the PACK, not by enumerating densities: that axis
    // has no ceiling to enumerate to.
    let variants: Vec<(String, &str, u16)> = pack
        .animation_names()
        .into_iter()
        .filter_map(|name| {
            let (base, density) = split_density_variant(&name)?;
            let base = OPTIONAL_FURNITURE_ANIMATIONS
                .iter()
                .find(|&&b| b == base)
                .copied()?;
            Some((name, base, density))
        })
        .collect();

    let mut check_frames = |name: &str, requirement_key: &str| {
        let min_frames = MULTI_FRAME_REQUIREMENTS
            .iter()
            .find(|&&(n, _)| n == requirement_key)
            .map_or(1, |&(_, min)| min);
        if let Some(anim) = pack.animation(name) {
            if anim.frames().len() < min_frames {
                report.insufficient_frames.push((
                    name.to_string(),
                    min_frames,
                    anim.frames().len(),
                ));
            }
        }
    };
    for name in known_names() {
        check_frames(name, name);
    }
    for (name, base, _) in &variants {
        check_frames(name, base);
    }

    // The name CLAIMS a density; the frame size is what proves it. Without this
    // the claim is only ever tested by whichever renderer happens to look for
    // that density — i.e. silently, at paint time, on someone else's terminal.
    for (name, base, density) in &variants {
        let Some(base_art) = pack.animation(base).and_then(|a| a.frames().first()) else {
            // No base means no claim to check it against. Reported rather than
            // skipped: silence here is what let a renamed `desk.sprite` pass.
            report.orphan_variants.push(name.clone());
            continue;
        };
        let Some(art) = pack.animation(name).and_then(|a| a.frames().first()) else {
            // An empty variant is already `insufficient_frames`' finding.
            continue;
        };
        // Saturating, not `*`: the density is bounded but the BASE is not — a
        // pack may ship art of any size, and this number is only ever REPORTED.
        // A saturated claim still differs from any real frame size, so the
        // mismatch fires either way.
        let claimed = (
            base_art.width().saturating_mul(*density),
            base_art.height().saturating_mul(*density),
        );
        let found = (art.width(), art.height());
        if claimed != found {
            report.mismatched_density.push(DensityMismatch {
                name: name.clone(),
                claimed,
                found,
            });
        }
    }

    let all_known: std::collections::HashSet<&str> = known_names().collect();
    for name in pack.animation_names() {
        if !all_known.contains(name.as_str()) && !is_optional_furniture_animation(&name) {
            report.unknown.push(name.clone());
        }
    }

    report
}

#[cfg(test)]
mod validation_floor_tests {
    use super::*;

    fn pack_with_animation(name: &str, frames_toml: &str) -> Pack {
        let pack_toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.{name}]\nframes={frames_toml}\nframe_ms=100\n"
        );
        load_pack_from_strings(&pack_toml, &[("f.sprite", "@frame 0\nA")]).expect("pack builds")
    }

    fn pack_with(animations: &str) -> Pack {
        let toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n{animations}"
        );
        load_pack_from_strings(&toml, &[("f.sprite", "@frame 0\nA")]).expect("pack builds")
    }

    /// The whole point of deriving: authoring `<piece>@<N>x` is a sprite
    /// file and nothing else. A second registry list would have to be kept
    /// in step with the first, and every entry someone forgets is a silent
    /// downgrade for `--pack-dir` users.
    #[test]
    fn a_density_variant_is_known_by_derivation_not_by_its_own_row() {
        assert!(is_optional_furniture_animation("desk"));
        assert!(is_optional_furniture_animation("desk@4x"));
        assert!(is_optional_furniture_animation("phone_booth@2x"));
        // The derivation is not a blanket suffix pass — the BASE still has to
        // be a real registered piece, or a typo'd `dsek@4x` would validate.
        assert!(!is_optional_furniture_animation("dsek@4x"));
        assert!(!is_optional_furniture_animation("standing@2x"));
    }

    #[test]
    fn a_variant_name_carries_the_density_it_is_drawn_at() {
        // The name is the CLAIM a renderer looks up by, so it round-trips.
        assert_eq!(density_variant_name("desk", 4), "desk@4x");
        assert_eq!(split_density_variant("desk@4x"), Some(("desk", 4)));
        assert_eq!(split_density_variant("desk@12x"), Some(("desk", 12)));
        // `1x` is the base piece under a second name — one thing with two
        // names is how a pack ends up shipping both and disagreeing.
        assert_eq!(split_density_variant("desk@1x"), None);
        assert_eq!(split_density_variant("desk@0x"), None);
        // Malformed claims are not variants; they fall through to the plain
        // name, where the registry rejects them as unknown.
        assert_eq!(split_density_variant("desk"), None);
        assert_eq!(split_density_variant("desk@x"), None);
        assert_eq!(split_density_variant("desk@4"), None);
        assert_eq!(split_density_variant("desk@-2x"), None);
    }

    /// THE failure this derivation exists to prevent, and it is invisible
    /// from inside the bundled pack: a `--pack-dir` pack that ships its own
    /// `desk` but no density variant must still inherit the default's, or the
    /// custom pack silently renders block-upscaled while the bundled one
    /// does not.
    #[test]
    fn merge_from_inherits_a_density_variant_so_a_custom_pack_keeps_the_richer_art() {
        let base = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        let mut custom = pack_with("[animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        custom.merge_from(&base);
        assert!(
            custom.animation("desk").is_some(),
            "the base piece inherits"
        );
        assert!(
            custom.animation("desk@4x").is_some(),
            "its density variant must inherit too"
        );
    }

    /// Neither "missing" nor "unknown": a density variant is authored or it
    /// is not, and every pack that has not been redrawn is the normal case.
    /// Listing them as missing optionals would put ~28 permanent lines in
    /// every `validate-pack` run.
    #[test]
    fn an_unauthored_density_variant_is_not_reported_missing() {
        let report = validate_pack_animations(&pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        ));
        assert!(
            !report
                .missing_optional
                .iter()
                .any(|n| n.contains(DENSITY_VARIANT_SEP)),
            "unauthored variants must not read as missing: {:?}",
            report.missing_optional
        );
        assert!(!report.unknown.contains(&"desk".to_string()));
    }

    /// An empty `desk@4x` is the WORSE shadow: `contains_key` is true, so
    /// `merge_from` skips the default's real art and the piece renders
    /// nothing at the density it claims to serve.
    #[test]
    fn an_empty_density_variant_still_fails_the_frame_floor() {
        let report = validate_pack_animations(&pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[]\nframe_ms=100\n",
        ));
        assert!(
            report
                .insufficient_frames
                .iter()
                .any(|(n, _, got)| n == "desk@4x" && *got == 0),
            "an empty variant must be caught: {:?}",
            report.insufficient_frames
        );
    }

    /// A painter picks its scale from the TERMINAL, but a variant only lands
    /// at a scale its density divides — so it needs ONE number from the pack
    /// to round against, and it must be the MAX because one scale serves
    /// every piece at once.
    #[test]
    fn the_packs_max_density_is_the_scale_a_painter_has_to_round_to() {
        let plain = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert_eq!(
            plain.max_density_variant(),
            1,
            "a pack with no variants must not push a painter off the cell's own scale"
        );

        let mixed = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@2x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"plant@4x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        assert_eq!(mixed.max_density_variant(), 4);

        // The bundled pack is what a default run paints with, so the number a
        // real terminal rounds against is pinned here rather than assumed.
        let bundled = load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.\"desk@4x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
            &[("f.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        assert_eq!(bundled.max_density_variant(), 4);

        // A key nothing can ever draw must not raise the number. `<base>@<N>x`
        // parses for ANY base, so a typo'd or stray entry in a user's pack.toml
        // is a well-formed variant name for a piece no painter asks for — it is
        // reported only as `unknown`, and rounding the scale to it would round
        // to art that does not exist.
        let stray = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@2x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"typo@64x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        assert_eq!(
            stray.max_density_variant(),
            2,
            "a variant of a non-furniture base must not inflate the pack's density"
        );
    }

    /// The name is a CLAIM and the size is the proof. Without this check the
    /// claim is only ever tested by whichever renderer happens to look for
    /// that density — silently, at paint time, on someone else's terminal —
    /// and the piece draws at the wrong size when it is.
    #[test]
    fn a_variant_that_lies_about_its_density_is_a_hard_error() {
        let pack = load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n",
            &[
                ("one.sprite", "@frame 0\nA A"),
                // 4 wide, not the 8 that `@4x` of a 2-wide base claims.
                ("four.sprite", "@frame 0\nA A A A"),
            ],
        )
        .expect("pack builds");
        let report = validate_pack_animations(&pack);
        assert_eq!(
            report.mismatched_density,
            vec![DensityMismatch {
                name: "desk@4x".to_string(),
                claimed: (8, 4),
                found: (4, 1),
            }],
        );
        assert!(
            report.has_errors(),
            "a lying variant must fail validate-pack, not merely be noted"
        );
    }

    /// The density is pack-author input to arithmetic that multiplies it by a
    /// frame dimension. Unbounded, `desk@60000x` panicked every debug build —
    /// including `run --pack-dir`, where the crash hook offered to file a bug
    /// for a typo — and wrapped to a nonsense dimension in release.
    #[test]
    fn a_density_past_the_ceiling_is_not_a_variant_at_all() {
        assert_eq!(split_density_variant("desk@60000x"), None);
        assert_eq!(split_density_variant("desk@65535x"), None);
        // The boundary from both sides, so a future `<` typo cannot slip through.
        assert_eq!(
            split_density_variant("desk@64x"),
            Some(("desk", MAX_DENSITY_VARIANT))
        );
        assert_eq!(split_density_variant("desk@65x"), None);
        // An out-of-range density is not a variant, so the name is simply
        // unknown — never a piece whose base the pack must supply.
        assert!(!is_optional_furniture_animation("desk@60000x"));
    }

    /// A bounded density still meets an unbounded BASE, and the claim is only
    /// ever reported — so it saturates rather than wrapping, and the mismatch
    /// still fires.
    #[test]
    fn a_huge_base_saturates_its_claim_instead_of_wrapping() {
        let wide = format!("@frame 0\n{}", "A ".repeat(2000).trim_end());
        let pack = load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.desk]\nframes=[\"wide.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@64x\"]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
            &[("wide.sprite", &wide), ("one.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        let report = validate_pack_animations(&pack);
        let m = report
            .mismatched_density
            .first()
            .expect("the variant is not 64x the base");
        assert_eq!(
            m.claimed.0,
            u16::MAX,
            "2000 * 64 saturates instead of wrapping to 62_464"
        );
        assert_ne!(m.claimed, m.found, "a saturated claim is still a mismatch");
    }

    /// Renaming `desk.sprite` to `desk@4x.sprite` instead of ADDING it used to
    /// pass clean: the loader has no base to check the claim against, and the
    /// runtime merge then validates it against the DEFAULT pack's art.
    #[test]
    fn a_variant_whose_base_the_pack_does_not_ship_is_an_error() {
        let pack = load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.\"desk@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n",
            &[("four.sprite", "@frame 0\nA A A A")],
        )
        .expect("pack builds");
        let report = validate_pack_animations(&pack);
        assert_eq!(report.orphan_variants, vec!["desk@4x".to_string()]);
        assert!(
            report.mismatched_density.is_empty(),
            "with no base there is no size claim to contradict"
        );
        assert!(report.has_errors(), "the author must be told, not passed");
    }

    #[test]
    fn empty_frames_on_a_required_animation_fails_validation() {
        let pack = pack_with_animation("seated", "[]");
        let report = validate_pack_animations(&pack);
        assert!(
            report
                .insufficient_frames
                .contains(&("seated".to_string(), 1, 0)),
            "empty seated must report (seated, 1, 0); got {:?}",
            report.insufficient_frames
        );
        let (name, need, have) = &report.insufficient_frames[0];
        assert_eq!((name.as_str(), *need, *have), ("seated", 1, 0));
        assert!(report.has_errors());
        assert!(!report.missing_required.contains(&"seated".to_string()));
    }

    #[test]
    fn empty_frames_on_an_optional_furniture_animation_fails_validation() {
        let pack = pack_with_animation("desk", "[]");
        let report = validate_pack_animations(&pack);
        assert!(
            report
                .insufficient_frames
                .contains(&("desk".to_string(), 1, 0)),
            "empty desk must report (desk, 1, 0); got {:?}",
            report.insufficient_frames
        );
        assert!(report.has_errors());
    }

    #[test]
    fn one_frame_on_a_plain_known_animation_passes_validation() {
        let pack = pack_with_animation("seated", "[\"f.sprite\"]");
        let report = validate_pack_animations(&pack);
        assert!(
            report.insufficient_frames.is_empty(),
            "a 1-frame seated must not be flagged; got {:?}",
            report.insufficient_frames
        );
    }

    #[test]
    fn multi_frame_requirements_all_name_known_animations() {
        let known: std::collections::HashSet<&str> = REQUIRED_CHARACTER_ANIMATIONS
            .iter()
            .chain(OPTIONAL_CHARACTER_ANIMATIONS.iter())
            .chain(OPTIONAL_FURNITURE_ANIMATIONS.iter())
            .copied()
            .collect();
        for (name, _) in MULTI_FRAME_REQUIREMENTS {
            assert!(
                known.contains(name),
                "MULTI_FRAME_REQUIREMENTS names unknown animation {name}"
            );
        }
    }
}

fn parse_palette_value(v: &str) -> Result<Pixel> {
    if v.eq_ignore_ascii_case("transparent") {
        return Ok(None);
    }
    let hex = v
        .strip_prefix('#')
        .ok_or_else(|| anyhow!("color must start with '#' or be 'transparent', got {v:?}"))?;
    if hex.len() != 6 {
        bail!("color {v:?} must be 6 hex digits");
    }
    // `u8::from_str_radix` accepts a leading '+', so `#+f0102` would slice to
    // `+f`/`01`/`02` and parse as a valid color without this explicit hex check.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("color {v:?} must be 6 hex digits");
    }
    let r = u8::from_str_radix(&hex[0..2], 16)?;
    let g = u8::from_str_radix(&hex[2..4], 16)?;
    let b = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Some(Rgb { r, g, b }))
}
