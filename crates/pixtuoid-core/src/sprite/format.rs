use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::grid::Grid;
use crate::sprite::{
    Frame, IndexedFrame, Palette, PaletteIndex, Pixel, Rgb, Sprite, PALETTE_CAPACITY,
};

/// Parse a `.sprite` text file: one indexed frame per `@frame N` block.
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
        let err = parse_indexed(src, &pal).unwrap_err();
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

    /// The animation registered under `key`, or, when the pack lacks a derived
    /// piece (`desk_north`), the piece it is drawn to match.
    pub fn animation_or_source(&self, key: &str) -> Option<&Sprite> {
        self.animation(self.piece_or_source(key)?)
    }

    /// The name of the animation that draws `key`: [`animation_or_source`]'s
    /// pick. A painter that takes a piece's density variants looks them up under
    /// this name, since a derived piece the pack lacks draws its source's.
    ///
    /// [`animation_or_source`]: Self::animation_or_source
    pub fn piece_or_source<'k>(&self, key: &'k str) -> Option<&'k str> {
        if self.animation(key).is_some() {
            return Some(key);
        }
        derived_source(key).filter(|source| self.animation(source).is_some())
    }

    /// The names of every animation in this pack.
    pub fn animation_names(&self) -> Vec<String> {
        self.animations.keys().cloned().collect()
    }

    /// The densest of [`Pack::density_variants`], or 1 when the pack ships none.
    pub fn max_density_variant(&self) -> u16 {
        self.density_variants().first().copied().unwrap_or(1)
    }

    /// The densities this pack's variants are drawn at, densest first, each
    /// once.
    ///
    /// A painter picks its render scale against these (the scene's
    /// `RenderScale::fit`), since a variant only lands at a scale its density
    /// divides. Only a variant of a registered animation that redraws its base
    /// ([`variant_redraws`]) counts: a stray key names nothing a painter asks
    /// for, and every renderer skips a variant that does not redraw its base.
    pub fn density_variants(&self) -> Vec<u16> {
        let densities: std::collections::BTreeSet<u16> = self
            .animations
            .iter()
            .filter_map(|(name, variant)| {
                let RegisteredKey { base, density } = RegisteredKey::parse(name)?;
                let density = density?;
                variant_redraws(self.animation(base)?, density, variant).then_some(density)
            })
            .collect();
        densities.into_iter().rev().collect()
    }

    /// Merge [`OPTIONAL_FURNITURE_ANIMATIONS`] — and their density variants —
    /// from `base` into self: the keys `RegisteredKey::is_inherited` passes.
    ///
    /// Driven by what `base` HAS rather than by the registry: the registry names
    /// PIECES, not the densities each is drawn at, so enumerating from it would
    /// probe every piece at every density to find the few `base` ships.
    pub fn merge_from(&mut self, base: &Pack) {
        let inherited: Vec<(String, Sprite)> = base
            .animations
            .iter()
            .filter(|(name, _)| RegisteredKey::parse(name).is_some_and(RegisteredKey::is_inherited))
            .filter(|(name, _)| {
                !self.animations.contains_key(*name) && self.own_redrawn_piece(name).is_none()
            })
            .map(|(name, sprite)| (name.clone(), sprite.clone()))
            .collect();
        self.animations.extend(inherited);
    }

    /// The piece of this pack's own that `name` redraws, if it ships one. Art
    /// that redraws another piece only comes along with that piece: over this
    /// pack's own `desk`, the default's `desk@4x` or `desk_north` would draw the
    /// default's desk wherever it is picked, so [`Pack::merge_from`] inherits
    /// nothing a piece of this pack's own answers for.
    fn own_redrawn_piece<'n>(&self, name: &'n str) -> Option<&'n str> {
        redrawn_pieces(name).find(|piece| self.animations.contains_key(*piece))
    }
}

/// Furniture drawn to match another piece (`desk_north` is `desk` with its
/// monitor raised), as `(derived, source)`.
const DERIVED_PIECES: &[(&str, &str)] = &[("desk_north", "desk")];

/// The piece `piece` is drawn to match, if it is a derived one.
fn derived_source(piece: &str) -> Option<&'static str> {
    DERIVED_PIECES
        .iter()
        .find(|&&(derived, _)| derived == piece)
        .map(|&(_, source)| source)
}

/// The pieces whose art `name` redraws: a density variant's base, then the
/// source that base is derived from.
fn redrawn_pieces(name: &str) -> impl Iterator<Item = &str> {
    let variant_base = split_density_variant(name).map(|(base, _)| base);
    let source = derived_source(variant_base.unwrap_or(name));
    variant_base.into_iter().chain(source)
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

/// Same as [`load_pack`] but takes in-memory strings — used by the embedded
/// default pack, which `include_str!`s its assets at compile time.
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

/// Separator joining an animation to the density it is drawn at: `desk@4x` is
/// the `desk` piece drawn on a 4x grid, for a painter rendering at a scale
/// where the base art would otherwise be block-upscaled.
///
/// The SCALE is in the name: a name that says only "denser" cannot express a
/// pack shipping BOTH a 2x and a 4x variant of one piece, and leaves the file's
/// meaning dependent on whichever render scale happens to measure it.
pub(crate) const DENSITY_VARIANT_SEP: char = '@';

/// The animation name for `base` drawn at `density`x.
pub fn density_variant_name(base: &str, density: u16) -> String {
    let mut out = String::with_capacity(base.len() + 4);
    density_variant_name_into(&mut out, base, density);
    out
}

/// [`density_variant_name`] into a caller-owned buffer, so a lookup loop reuses
/// one allocation.
pub fn density_variant_name_into(out: &mut String, base: &str, density: u16) {
    use std::fmt::Write;
    // Writing to a String is infallible.
    let _ = write!(out, "{base}{DENSITY_VARIANT_SEP}{density}x");
}

/// The largest density a variant name may claim.
///
/// A pack author types this number, so a claim past any real authoring grid is
/// a typo, and a bound there costs no real pack anything. It keeps
/// `desk@60000x` an unknown name rather than a variant, which would become one
/// of [`Pack::density_variants`] that no real render scale lands on.
pub(crate) const MAX_DENSITY_VARIANT: u16 = 64;

/// The base animation and density a variant name denotes, if it is one.
///
/// `1x` is deliberately NOT a variant: it would be a second name for the base
/// animation, and one thing with two names is how a pack ends up shipping both.
pub(crate) fn split_density_variant(name: &str) -> Option<(&str, u16)> {
    let (base, density) = name.rsplit_once(DENSITY_VARIANT_SEP)?;
    let digits = density.strip_suffix('x')?;
    // Digits only, with no leading zero. `u16::from_str` accepts a leading `+`
    // or `0`, which would give one density two spellings — and this name is a
    // lookup KEY, so two spellings is two files a renderer picks between
    // arbitrarily, or one it never looks up.
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u16 = digits.parse().ok()?;
    (2..=MAX_DENSITY_VARIANT).contains(&n).then_some((base, n))
}

/// Every registered animation: the required and optional character poses and
/// the optional furniture.
fn registered_animation_names() -> impl Iterator<Item = &'static str> {
    REQUIRED_CHARACTER_ANIMATIONS
        .iter()
        .chain(OPTIONAL_CHARACTER_ANIMATIONS)
        .chain(OPTIONAL_FURNITURE_ANIMATIONS)
        .copied()
}

/// A pack key that names a registered animation: the animation itself, or a
/// density variant of it (`desk@4x`). Any registered animation takes variants,
/// since a character is redrawn at density like furniture; only furniture is
/// inherited ([`RegisteredKey::is_inherited`]).
///
/// Variants are legal BY DERIVATION rather than by their own registry rows, so
/// authoring one needs no registry row. A second list would have to be kept in
/// step with the first, and forgetting an entry fails QUIETLY in its least
/// visible direction: the variant loads for the bundled pack but
/// [`Pack::merge_from`] never inherits it, so a `--pack-dir` user silently
/// drops back to the upscale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegisteredKey {
    /// The registered animation the key names or redraws.
    base: &'static str,
    /// The density a variant is drawn at; `None` for the animation itself.
    density: Option<u16>,
}

impl RegisteredKey {
    /// `name` as a registered key, or `None` when it names nothing registered.
    fn parse(name: &str) -> Option<Self> {
        let (base, density) =
            split_density_variant(name).map_or((name, None), |(base, d)| (base, Some(d)));
        registered_animation_names()
            .find(|&known| known == base)
            .map(|base| Self { base, density })
    }

    /// Whether [`Pack::merge_from`] inherits this key: furniture and its
    /// variants only, because a robot pack must not fall back to human sprites.
    fn is_inherited(self) -> bool {
        OPTIONAL_FURNITURE_ANIMATIONS.contains(&self.base)
    }
}

/// Environment/furniture animation names a pack MAY provide: the ones
/// [`Pack::merge_from`] inherits.
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
    "meeting_sofa_north",
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

/// The size a `<base>@<N>x` variant must be: `base`'s times `density`, exactly.
///
/// Wider than a frame dimension: a claim past `u16::MAX` stays a size no frame
/// can meet, where a saturated one would equal a `u16::MAX`-wide frame.
pub fn claimed_variant_size(base: &Frame, density: u16) -> (u32, u32) {
    (
        u32::from(base.width()) * u32::from(density),
        u32::from(base.height()) * u32::from(density),
    )
}

/// Whether `variant` is exactly the size its density claims over `base`
/// ([`claimed_variant_size`]).
pub fn variant_fits(base: &Frame, density: u16, variant: &Frame) -> bool {
    claimed_variant_size(base, density) == (u32::from(variant.width()), u32::from(variant.height()))
}

/// Whether `variant` redraws `base` at `density`: frame for frame, each frame
/// exactly the size its density claims over the matching base frame
/// ([`variant_fits`]). The one rule a renderer takes a variant by and
/// [`validate_pack_animations`] passes one by.
pub fn variant_redraws(base: &Sprite, density: u16, variant: &Sprite) -> bool {
    let (base, variant) = (base.frames(), variant.frames());
    !variant.is_empty()
        && variant.len() == base.len()
        && base
            .iter()
            .zip(variant)
            .all(|(base, variant)| variant_fits(base, density, variant))
}

/// A density variant with a frame whose size is not what its name claims over
/// the matching base frame.
///
/// [`validate_pack_animations`] calls it an error: a renderer skips such a
/// variant, so the art the author shipped never shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DensityMismatch {
    /// The variant's animation name, e.g. `desk@4x`.
    pub name: String,
    /// Its first frame that misses the claim, counted from 0 in the order the
    /// pack loads them: every `@frame` block of each file its `frames` lists.
    pub frame: usize,
    /// The size the name claims: [`claimed_variant_size`].
    pub claimed: (u32, u32),
    /// The size that frame actually is.
    pub found: (u16, u16),
}

/// A density variant whose frame count is not its base's, so it cannot redraw
/// it ([`variant_redraws`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameCountMismatch {
    /// The variant's animation name, e.g. `typing@2x`.
    pub name: String,
    /// How many frames its base animation has.
    pub base_frames: usize,
    /// How many frames the variant has.
    pub variant_frames: usize,
}

/// What draws an optional animation a pack leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandIn {
    /// The default pack's piece, which [`Pack::merge_from`] inherits.
    DefaultPack,
    /// The pack's own piece that this one redraws (`desk` for `desk_north`):
    /// [`Pack::merge_from`] inherits nothing over it, and a painter draws it
    /// ([`Pack::animation_or_source`]).
    OwnPiece(&'static str),
    /// Another of the pack's own poses: character animations are never
    /// inherited.
    OwnPose,
}

/// An optional animation absent from a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingOptional {
    /// The registry name.
    pub name: &'static str,
    /// What draws in its place.
    pub stand_in: StandIn,
}

/// One of the caller's art sets that a pack ships only part of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialSet {
    /// The set's pieces the pack ships.
    pub shipped: Vec<&'static str>,
    /// The set's pieces it leaves to the default pack.
    pub missing: Vec<&'static str>,
}

/// A derived piece (`desk_north`) shipped without the piece it is drawn to
/// match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanDerived {
    /// The derived piece the pack ships.
    pub derived: &'static str,
    /// The piece it is drawn to match, which the default pack then supplies.
    pub source: &'static str,
}

/// Per-category tally of a pack's animation discrepancies.
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Required character-animation names absent from the pack — an error.
    pub missing_required: Vec<String>,
    /// Optional animations absent from the pack, except one a
    /// [`partial_sets`](Self::partial_sets) or
    /// [`orphan_derived`](Self::orphan_derived) finding already names.
    pub missing_optional: Vec<MissingOptional>,
    /// `(name, need, have)` — REQUIRED count first — for each animation with
    /// fewer frames than its minimum.
    pub insufficient_frames: Vec<(String, usize, usize)>,
    /// Animation names present in the pack that are neither registered nor a
    /// density variant of a registered animation.
    pub unknown: Vec<String>,
    /// Each density variant with a frame whose size is not its base frame's
    /// times the density its NAME claims.
    pub mismatched_density: Vec<DensityMismatch>,
    /// Each density variant whose BASE animation the pack does not ship.
    ///
    /// The size claim is unprovable without the base, so the variant would load
    /// unchecked, or, for furniture, validated against whatever the default pack
    /// supplies — an author who renamed `desk.sprite` to `desk@4x.sprite`
    /// instead of adding it otherwise gets a clean bill of health from the one
    /// tool whose job is to tell them.
    pub orphan_variants: Vec<String>,
    /// Each density variant whose frame count is not its base's.
    pub mismatched_frame_counts: Vec<FrameCountMismatch>,
    /// Each of the caller's art sets the pack ships only part of: the default
    /// pack draws the rest, in its own style.
    pub partial_sets: Vec<PartialSet>,
    /// Each derived piece the pack ships without its source: the default pack
    /// draws the source, in its own style.
    pub orphan_derived: Vec<OrphanDerived>,
}

impl ValidationReport {
    /// How many findings make the pack unusable: the fields this destructure
    /// counts. A field bound to `_` is reported, not counted.
    pub fn error_count(&self) -> usize {
        // No `..`: a new report field must be classed error-or-not here before
        // this compiles.
        let ValidationReport {
            missing_required,
            missing_optional: _,
            insufficient_frames,
            unknown: _,
            mismatched_density,
            orphan_variants,
            mismatched_frame_counts,
            partial_sets: _,
            orphan_derived: _,
        } = self;
        missing_required.len()
            + insufficient_frames.len()
            + mismatched_density.len()
            + orphan_variants.len()
            + mismatched_frame_counts.len()
    }

    /// How many findings leave the pack usable but not as authored: the fields
    /// this destructure counts. A field bound to `_` is reported, not counted.
    pub fn warning_count(&self) -> usize {
        // No `..`, for the reason `error_count` gives.
        let ValidationReport {
            missing_required: _,
            missing_optional,
            insufficient_frames: _,
            unknown: _,
            mismatched_density: _,
            orphan_variants: _,
            mismatched_frame_counts: _,
            partial_sets,
            orphan_derived,
        } = self;
        missing_optional.len() + partial_sets.len() + orphan_derived.len()
    }

    /// True when the pack is unusable; see [`error_count`](Self::error_count).
    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }
}

/// Check a pack's animations against the required/optional/multi-frame
/// registries, each density variant against its base, each derived piece
/// against its source, and the pack against `art_sets`: the sets of pieces a
/// pack should ship whole, which only the caller's painters know.
///
/// An unauthored variant is not reported missing: a pack that has not been
/// redrawn at a density is the normal case, not a gap.
pub fn validate_pack_animations(pack: &Pack, art_sets: &[Vec<&'static str>]) -> ValidationReport {
    let mut report = ValidationReport::default();

    for &name in REQUIRED_CHARACTER_ANIMATIONS {
        if pack.animation(name).is_none() {
            report.missing_required.push(name.to_string());
        }
    }

    for set in art_sets {
        let (shipped, missing): (Vec<&'static str>, Vec<&'static str>) = set
            .iter()
            .partition(|&&name| pack.animation(name).is_some());
        if !shipped.is_empty() && !missing.is_empty() {
            report.partial_sets.push(PartialSet { shipped, missing });
        }
    }

    for &(derived, source) in DERIVED_PIECES {
        if pack.animation(derived).is_some() && pack.animation(source).is_none() {
            report
                .orphan_derived
                .push(OrphanDerived { derived, source });
        }
    }

    let named_elsewhere = |name: &str| {
        report
            .partial_sets
            .iter()
            .any(|s| s.missing.contains(&name))
            || report.orphan_derived.iter().any(|o| o.source == name)
    };
    let missing_optional: Vec<MissingOptional> = OPTIONAL_CHARACTER_ANIMATIONS
        .iter()
        .map(|&name| (name, StandIn::OwnPose))
        .chain(OPTIONAL_FURNITURE_ANIMATIONS.iter().map(|&name| {
            let stand_in = pack
                .own_redrawn_piece(name)
                .map_or(StandIn::DefaultPack, StandIn::OwnPiece);
            (name, stand_in)
        }))
        .filter(|&(name, _)| pack.animation(name).is_none() && !named_elsewhere(name))
        .map(|(name, stand_in)| MissingOptional { name, stand_in })
        .collect();
    report.missing_optional = missing_optional;

    let variants: Vec<(&str, &Sprite, &'static str, u16)> = pack
        .animations
        .iter()
        .filter_map(|(name, variant)| {
            let RegisteredKey { base, density } = RegisteredKey::parse(name)?;
            Some((name.as_str(), variant, base, density?))
        })
        .collect();

    let mut check_frames = |name: &str, requirement_key: &str| {
        let min_frames = MULTI_FRAME_REQUIREMENTS
            .iter()
            .find(|&&(n, _)| n == requirement_key)
            // Implicit min-1 floor: a `frames = []` entry deserializes and makes
            // `animation()` return Some (dodging the missing-required check)
            // while every render consumer guards with `.frames().first()` and
            // draws nothing; an empty OPTIONAL entry also SHADOWS the embedded
            // default in `Pack::merge_from` (`contains_key` is true).
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
    for name in registered_animation_names() {
        check_frames(name, name);
    }

    for &(name, variant, base, density) in &variants {
        let Some(base) = pack.animation(base).filter(|a| !a.frames().is_empty()) else {
            // No base, no claim to check: see `ValidationReport::orphan_variants`.
            report.orphan_variants.push(name.to_string());
            continue;
        };
        if variant_redraws(base, density, variant) {
            continue;
        }
        // Each defect is its own finding, so a short AND mis-sized variant
        // reports both.
        let (base_frames, variant_frames) = (base.frames(), variant.frames());
        // An empty variant lands here too, as a count of 0: it redraws nothing.
        if variant_frames.len() != base_frames.len() {
            report.mismatched_frame_counts.push(FrameCountMismatch {
                name: name.to_string(),
                base_frames: base_frames.len(),
                variant_frames: variant_frames.len(),
            });
        }
        let first_miss = base_frames
            .iter()
            .zip(variant_frames)
            .enumerate()
            .find(|(_, (base_art, art))| !variant_fits(base_art, density, art));
        if let Some((frame, (base_art, art))) = first_miss {
            report.mismatched_density.push(DensityMismatch {
                name: name.to_string(),
                frame,
                claimed: claimed_variant_size(base_art, density),
                found: (art.width(), art.height()),
            });
        }
    }

    for name in pack.animation_names() {
        if RegisteredKey::parse(&name).is_none() {
            report.unknown.push(name);
        }
    }

    report
}

#[cfg(test)]
mod validation_floor_tests {
    use super::*;

    /// A pack of `animations` whose frame files are `frames`.
    fn pack_with_frames(animations: &str, frames: &[(&str, &str)]) -> Pack {
        let toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n{animations}"
        );
        load_pack_from_strings(&toml, frames).expect("pack builds")
    }

    fn pack_with(animations: &str) -> Pack {
        pack_with_frames(animations, &[("f.sprite", "@frame 0\nA")])
    }

    fn pack_with_animation(name: &str, frames_toml: &str) -> Pack {
        pack_with(&format!(
            "[animations.{name}]\nframes={frames_toml}\nframe_ms=100\n"
        ))
    }

    /// 1x1 and 3x1 base frames, their 2x variants and a 4x of the 1x1; each
    /// also misses every other's claim.
    const SIZED_FRAMES: &[(&str, &str)] = &[
        ("one.sprite", "@frame 0\nA"),
        ("two.sprite", "@frame 0\nA A\nA A"),
        ("three.sprite", "@frame 0\nA A A"),
        ("six.sprite", "@frame 0\nA A A A A A\nA A A A A A"),
        (
            "four.sprite",
            "@frame 0\nA A A A\nA A A A\nA A A A\nA A A A",
        ),
    ];

    /// Pins [`RegisteredKey::parse`].
    #[test]
    fn a_key_names_a_registered_animation_or_a_density_variant_of_one() {
        let key = |base, density| Some(RegisteredKey { base, density });
        assert_eq!(RegisteredKey::parse("desk"), key("desk", None));
        assert_eq!(RegisteredKey::parse("desk@4x"), key("desk", Some(4)));
        assert_eq!(
            RegisteredKey::parse("standing@2x"),
            key("standing", Some(2))
        );
        assert_eq!(
            RegisteredKey::parse("walking_coffee@8x"),
            key("walking_coffee", Some(8))
        );
        // The BASE must be registered, or a typo'd `dsek@4x` would validate.
        assert_eq!(RegisteredKey::parse("dsek@4x"), None);
        assert_eq!(RegisteredKey::parse("typo"), None);
        assert_eq!(RegisteredKey::parse("desk@1x"), None);
    }

    /// Pins [`RegisteredKey::is_inherited`].
    #[test]
    fn only_furniture_and_its_variants_are_inherited() {
        let inherited = |name| {
            RegisteredKey::parse(name)
                .expect("registered")
                .is_inherited()
        };
        assert!(inherited("desk") && inherited("desk@4x") && inherited("phone_booth@2x"));
        assert!(!inherited("standing") && !inherited("standing@2x"));
    }

    /// Pins [`split_density_variant`]'s one spelling per density.
    #[test]
    fn a_density_with_a_leading_zero_is_not_a_variant() {
        assert_eq!(split_density_variant("desk@04x"), None);
        let pack = pack_with_frames(
            "[animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@02x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        assert_eq!(
            validate_pack_animations(&pack, &[]).unknown,
            vec!["desk@02x".to_string()]
        );
        assert_eq!(pack.max_density_variant(), 1);
    }

    #[test]
    fn a_variant_name_carries_the_density_it_is_drawn_at() {
        // The name is the CLAIM a renderer looks up by, so it round-trips.
        assert_eq!(density_variant_name("desk", 4), "desk@4x");
        assert_eq!(split_density_variant("desk@4x"), Some(("desk", 4)));
        assert_eq!(split_density_variant("desk@12x"), Some(("desk", 12)));
        assert_eq!(split_density_variant("desk@1x"), None);
        assert_eq!(split_density_variant("desk@0x"), None);
        // Malformed claims are not variants; they fall through to the plain
        // name, where the registry rejects them as unknown.
        assert_eq!(split_density_variant("desk"), None);
        assert_eq!(split_density_variant("desk@x"), None);
        assert_eq!(split_density_variant("desk@4"), None);
        assert_eq!(split_density_variant("desk@-2x"), None);
    }

    /// Pins [`Pack::merge_from`]'s variant inheritance, which the bundled pack
    /// alone never exercises.
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

    #[test]
    fn a_pack_that_redraws_a_piece_does_not_inherit_the_defaults_variant_of_it() {
        let base = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        let mut custom = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        custom.merge_from(&base);
        assert!(custom.animation("desk@4x").is_none());
    }

    #[test]
    fn every_derived_piece_and_its_source_are_registered_furniture() {
        let furniture = |name| RegisteredKey::parse(name).is_some_and(RegisteredKey::is_inherited);
        for &(derived, source) in DERIVED_PIECES {
            assert!(furniture(derived), "{derived}");
            assert!(furniture(source), "{source}");
        }
    }

    #[test]
    fn a_pack_without_a_derived_piece_draws_its_source() {
        let desk_only = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert!(desk_only.animation_or_source("desk_north").is_some());
        assert!(desk_only.animation_or_source("plant").is_none());
    }

    /// Pins [`Pack::piece_or_source`].
    #[test]
    fn a_pack_names_the_piece_that_draws_a_key() {
        let desk_only = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert_eq!(desk_only.piece_or_source("desk_north"), Some("desk"));
        assert_eq!(desk_only.piece_or_source("desk"), Some("desk"));
        assert_eq!(desk_only.piece_or_source("plant"), None);
        let both = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.desk_north]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        assert_eq!(both.piece_or_source("desk_north"), Some("desk_north"));
        let plant_only = pack_with("[animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert_eq!(plant_only.piece_or_source("desk_north"), None);
    }

    /// Pins a character variant: validated, counted by
    /// [`Pack::max_density_variant`], never inherited
    /// ([`RegisteredKey::is_inherited`]).
    #[test]
    fn a_character_animation_takes_density_variants_that_are_never_inherited() {
        let pack = pack_with_frames(
            "[animations.typing_back]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing_back@2x\"]\nframes=[\"two.sprite\", \"two.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        assert!(
            report.unknown.is_empty()
                && report.mismatched_density.is_empty()
                && report.mismatched_frame_counts.is_empty()
                && report.orphan_variants.is_empty(),
            "{report:?}"
        );
        assert_eq!(pack.max_density_variant(), 2);

        let mut custom = pack_with("[animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        custom.merge_from(&pack);
        assert!(custom.animation("typing_back@2x").is_none());
    }

    /// Pins [`Pack::density_variants`]: densest first, each density once, and
    /// only variants that redraw their base.
    #[test]
    fn density_variants_are_the_redrawing_densities_densest_first() {
        let pack = pack_with_frames(
            "[animations.typing]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n\
             [animations.walking]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"walking@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n\
             [animations.\"walking@3x\"]\nframes=[\"three.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        assert_eq!(
            pack.density_variants(),
            vec![4, 2],
            "3x does not redraw its base"
        );
        assert_eq!(pack.max_density_variant(), 4);
        let plain = pack_with("[animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert!(plain.density_variants().is_empty());
    }

    /// Pins [`variant_redraws`]' every-frame proof.
    #[test]
    fn every_frame_of_a_variant_is_proved_against_its_base_frame() {
        let pack = pack_with_frames(
            "[animations.typing]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"two.sprite\", \"three.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        assert_eq!(
            report.mismatched_density,
            vec![DensityMismatch {
                name: "typing@2x".to_string(),
                frame: 1,
                claimed: (2, 2),
                found: (3, 1),
            }]
        );
    }

    /// Pins [`ValidationReport::mismatched_frame_counts`], the one finding a
    /// short variant of a multi-frame base makes.
    #[test]
    fn a_short_variant_is_one_frame_count_error() {
        let pack = pack_with_frames(
            "[animations.typing]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        assert_eq!(
            report.mismatched_frame_counts,
            vec![FrameCountMismatch {
                name: "typing@2x".to_string(),
                base_frames: 2,
                variant_frames: 1,
            }]
        );
        assert!(
            report.insufficient_frames.is_empty(),
            "{:?}",
            report.insufficient_frames
        );
        assert!(report.has_errors());
    }

    /// Pins that the count and the size are each their own finding.
    #[test]
    fn a_short_and_mis_sized_variant_reports_both() {
        let pack = pack_with_frames(
            "[animations.typing]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"three.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        assert_eq!(report.mismatched_frame_counts.len(), 1, "{report:?}");
        assert_eq!(
            report.mismatched_density,
            vec![DensityMismatch {
                name: "typing@2x".to_string(),
                frame: 0,
                claimed: (2, 2),
                found: (3, 1),
            }]
        );
    }

    /// Pins the MATCHING base frame, in [`variant_redraws`] and in the
    /// validator's diagnosis of a variant that fails it.
    #[test]
    fn each_variant_frame_is_proved_against_the_matching_base_frame() {
        let pack = pack_with_frames(
            "[animations.walking]\nframes=[\"one.sprite\", \"three.sprite\"]\nframe_ms=100\n\
             [animations.\"walking@2x\"]\nframes=[\"two.sprite\", \"six.sprite\"]\nframe_ms=100\n\
             [animations.typing_back]\nframes=[\"one.sprite\", \"three.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing_back@2x\"]\nframes=[\"two.sprite\", \"six.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let anim = |n| pack.animation(n).expect("in the pack");
        assert!(variant_redraws(anim("walking"), 2, anim("walking@2x")));
        let report = validate_pack_animations(&pack, &[]);
        assert!(report.mismatched_density.is_empty(), "{report:?}");
        assert_eq!(
            report.mismatched_frame_counts,
            vec![FrameCountMismatch {
                name: "typing_back@2x".to_string(),
                base_frames: 3,
                variant_frames: 2,
            }],
            "the short variant's frames each fit their own base frame"
        );
    }

    /// Pins [`variant_redraws`] as the validator's verdict.
    #[test]
    fn variant_redraws_is_the_validators_verdict() {
        let pack = pack_with_frames(
            "[animations.typing]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"two.sprite\", \"two.sprite\"]\nframe_ms=100\n\
             [animations.walking]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"walking@2x\"]\nframes=[\"two.sprite\", \"three.sprite\"]\nframe_ms=100\n\
             [animations.walking_back]\nframes=[\"one.sprite\", \"one.sprite\"]\nframe_ms=100\n\
             [animations.\"walking_back@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n\
             [animations.standing]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"standing@2x\"]\nframes=[]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        for (name, base, redraws) in [
            ("typing@2x", "typing", true),
            ("walking@2x", "walking", false),
            ("walking_back@2x", "walking_back", false),
            ("standing@2x", "standing", false),
        ] {
            let anim = |n| pack.animation(n).expect("in the pack");
            let verdict = variant_redraws(anim(base), 2, anim(name));
            let found = report.mismatched_density.iter().any(|m| m.name == name)
                || report
                    .mismatched_frame_counts
                    .iter()
                    .any(|m| m.name == name);
            assert_eq!(verdict, redraws, "{name}");
            assert_eq!(verdict, !found, "{name}: {report:?}");
        }
    }

    #[test]
    fn a_character_variant_without_its_base_is_an_orphan() {
        let pack = pack_with_frames(
            "[animations.\"typing_back@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n",
            SIZED_FRAMES,
        );
        let report = validate_pack_animations(&pack, &[]);
        assert_eq!(report.orphan_variants, vec!["typing_back@2x".to_string()]);
        assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    }

    #[test]
    fn a_pack_that_redraws_a_desk_does_not_inherit_the_defaults_north_desk() {
        let base = pack_with(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.desk_north]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk_north@4x\"]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        );
        let mut custom = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        custom.merge_from(&base);
        assert!(custom.animation("desk_north").is_none());
        assert!(custom.animation("desk_north@4x").is_none());

        let mut bare = pack_with("[animations.plant]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        bare.merge_from(&base);
        assert!(
            bare.animation("desk_north").is_some(),
            "comes along with the desk"
        );
        assert!(bare.animation("desk_north@4x").is_some());
    }

    #[test]
    fn an_unauthored_density_variant_is_not_reported_missing() {
        let report = validate("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert!(
            !report
                .missing_optional
                .iter()
                .any(|m| m.name.contains(DENSITY_VARIANT_SEP)),
            "unauthored variants must not read as missing: {:?}",
            report.missing_optional
        );
        assert!(!report.unknown.contains(&"desk".to_string()));
    }

    /// Two sets a painter might draw as one look, for the tests that need the
    /// mechanism, not the scene's real sets.
    fn sets() -> Vec<Vec<&'static str>> {
        vec![
            vec!["pantry", "pantry_small"],
            vec!["cat_walk", "cat_sit", "cat_sleep"],
        ]
    }

    fn validate(animations: &str) -> ValidationReport {
        validate_pack_animations(&pack_with(animations), &sets())
    }

    const ONE: &str = "frames=[\"f.sprite\"]\nframe_ms=100\n";
    const TWO: &str = "frames=[\"f.sprite\", \"f.sprite\"]\nframe_ms=100\n";

    /// Pins [`ValidationReport::partial_sets`].
    #[test]
    fn a_pack_that_ships_part_of_an_art_set_is_told_which_pieces_it_left_out() {
        let report = validate(&format!(
            "[animations.cat_walk]\n{TWO}[animations.pantry]\n{ONE}"
        ));
        assert_eq!(
            report.partial_sets,
            vec![
                PartialSet {
                    shipped: vec!["pantry"],
                    missing: vec!["pantry_small"],
                },
                PartialSet {
                    shipped: vec!["cat_walk"],
                    missing: vec!["cat_sit", "cat_sleep"],
                },
            ]
        );

        let whole = validate(&format!(
            "[animations.cat_walk]\n{TWO}[animations.cat_sit]\n{ONE}[animations.cat_sleep]\n{ONE}"
        ));
        assert!(whole.partial_sets.is_empty(), "{:?}", whole.partial_sets);

        let untouched = validate(&format!("[animations.plant]\n{ONE}"));
        assert!(
            untouched.partial_sets.is_empty(),
            "a set the pack leaves out whole is the default's art throughout: {:?}",
            untouched.partial_sets
        );
    }

    /// Pins [`ValidationReport::orphan_derived`].
    #[test]
    fn a_derived_piece_shipped_without_its_source_is_reported() {
        let orphan = validate(&format!("[animations.desk_north]\n{ONE}"));
        assert_eq!(
            orphan.orphan_derived,
            vec![OrphanDerived {
                derived: "desk_north",
                source: "desk",
            }]
        );

        for animations in [
            format!("[animations.desk]\n{ONE}[animations.desk_north]\n{ONE}"),
            format!("[animations.desk]\n{ONE}"),
        ] {
            assert!(
                validate(&animations).orphan_derived.is_empty(),
                "{animations}"
            );
        }
    }

    /// Pins [`StandIn`] against [`Pack::merge_from`]'s own rule.
    #[test]
    fn a_missing_optional_piece_names_what_draws_in_its_place() {
        let report = validate(&format!("[animations.desk]\n{ONE}"));
        let stand_in = |name: &str| {
            report
                .missing_optional
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{name} is reported missing"))
                .stand_in
        };
        assert_eq!(stand_in("desk_north"), StandIn::OwnPiece("desk"));
        assert_eq!(stand_in("plant"), StandIn::DefaultPack);
        assert_eq!(stand_in("walking_coffee"), StandIn::OwnPose);

        let mut merged = pack_with(&format!("[animations.desk]\n{ONE}"));
        merged.merge_from(&pack_with(&format!(
            "[animations.desk_north]\n{ONE}[animations.plant]\n{ONE}"
        )));
        assert!(
            merged.animation("desk_north").is_none() && merged.animation("plant").is_some(),
            "the merge must agree with the classification"
        );
    }

    #[test]
    fn a_gap_another_finding_names_is_not_also_reported_missing() {
        let report = validate(&format!("[animations.cat_walk]\n{TWO}"));
        let named = |n: &str| report.missing_optional.iter().any(|m| m.name == n);
        assert!(
            !named("cat_sit") && !named("cat_sleep"),
            "{:?}",
            report.missing_optional
        );
        assert_eq!(report.partial_sets.len(), 1);

        let report = validate(&format!("[animations.desk_north]\n{ONE}"));
        assert!(
            !report.missing_optional.iter().any(|m| m.name == "desk"),
            "{:?}",
            report.missing_optional
        );
        assert_eq!(report.orphan_derived.len(), 1);
    }

    /// Pins [`ValidationReport::warning_count`] and
    /// [`ValidationReport::error_count`]: one finding in every field.
    #[test]
    fn every_finding_is_counted_once_as_an_error_or_a_warning_or_reported_only() {
        let report = ValidationReport {
            missing_required: vec!["seated".to_string()],
            missing_optional: vec![MissingOptional {
                name: "plant",
                stand_in: StandIn::DefaultPack,
            }],
            insufficient_frames: vec![("typing".to_string(), 2, 1)],
            unknown: vec!["foo".to_string()],
            mismatched_density: vec![DensityMismatch {
                name: "desk@4x".to_string(),
                frame: 0,
                claimed: (8, 4),
                found: (4, 1),
            }],
            orphan_variants: vec!["plant@2x".to_string()],
            mismatched_frame_counts: vec![FrameCountMismatch {
                name: "seated@2x".to_string(),
                base_frames: 2,
                variant_frames: 1,
            }],
            partial_sets: vec![PartialSet {
                shipped: vec!["cat_walk"],
                missing: vec!["cat_sit"],
            }],
            orphan_derived: vec![OrphanDerived {
                derived: "desk_north",
                source: "desk",
            }],
        };
        assert_eq!(report.error_count(), 5);
        assert_eq!(report.warning_count(), 3);
    }

    /// Pins the frame-count check's empty case.
    #[test]
    fn an_empty_density_variant_is_a_frame_count_mismatch() {
        let report = validate(
            "[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[]\nframe_ms=100\n",
        );
        assert_eq!(
            report.mismatched_frame_counts,
            vec![FrameCountMismatch {
                name: "desk@4x".to_string(),
                base_frames: 1,
                variant_frames: 0,
            }]
        );
        assert!(
            report.insufficient_frames.is_empty(),
            "{:?}",
            report.insufficient_frames
        );
    }

    /// Pins [`Pack::max_density_variant`].
    #[test]
    fn the_packs_max_density_is_the_scale_a_painter_has_to_round_to() {
        let pack = |extra: &str| {
            pack_with_frames(
                &format!(
                    "[animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
                     [animations.\"desk@2x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n\
                     [animations.plant]\nframes=[\"one.sprite\"]\nframe_ms=100\n{extra}"
                ),
                SIZED_FRAMES,
            )
        };
        let plain = pack_with("[animations.desk]\nframes=[\"f.sprite\"]\nframe_ms=100\n");
        assert_eq!(
            plain.max_density_variant(),
            1,
            "a pack with no variants must not push a painter off the cell's own scale"
        );
        assert_eq!(
            pack("[animations.\"plant@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            4
        );
        assert_eq!(
            pack("[animations.\"typo@64x\"]\nframes=[\"one.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            2,
            "a variant of an unregistered base must not inflate the pack's density"
        );
        assert_eq!(
            pack("[animations.\"plant@8x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            2,
            "a variant every renderer skips must not round a painter's scale to it"
        );
    }

    /// Pins [`DensityMismatch`].
    #[test]
    fn a_variant_that_lies_about_its_density_is_a_hard_error() {
        let pack = pack_with_frames(
            "[animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n",
            &[
                ("one.sprite", "@frame 0\nA A"),
                // 4 wide, not the 8 that `@4x` of a 2-wide base claims.
                ("four.sprite", "@frame 0\nA A A A"),
            ],
        );
        let report = validate_pack_animations(&pack, &[]);
        assert_eq!(
            report.mismatched_density,
            vec![DensityMismatch {
                name: "desk@4x".to_string(),
                frame: 0,
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
        assert_eq!(RegisteredKey::parse("desk@60000x"), None);
    }

    /// Pins [`claimed_variant_size`].
    #[test]
    fn a_claim_no_frame_can_meet_is_a_mismatch() {
        let row = |w: usize| format!("{}\n", "A ".repeat(w).trim_end());
        let base = format!("@frame 0\n{}", row(40_000));
        let variant = format!("@frame 0\n{0}{0}", row(u16::MAX as usize));
        let pack = pack_with_frames(
            "[animations.desk]\nframes=[\"base.sprite\"]\nframe_ms=100\n\
             [animations.\"desk@2x\"]\nframes=[\"variant.sprite\"]\nframe_ms=100\n",
            &[("base.sprite", &base), ("variant.sprite", &variant)],
        );
        let report = validate_pack_animations(&pack, &[]);
        let m = report
            .mismatched_density
            .first()
            .expect("80_000 wide is claimed, 65_535 is found");
        assert_eq!(m.claimed, (80_000, 2));
        assert_eq!(m.found, (u16::MAX, 2));
    }

    /// Pins `ValidationReport::orphan_variants`.
    #[test]
    fn a_variant_whose_base_the_pack_does_not_ship_is_an_error() {
        let pack = pack_with_frames(
            "[animations.\"desk@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n",
            &[("four.sprite", "@frame 0\nA A A A")],
        );
        let report = validate_pack_animations(&pack, &[]);
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
        let report = validate_pack_animations(&pack, &[]);
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
        let report = validate_pack_animations(&pack, &[]);
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
        let report = validate_pack_animations(&pack, &[]);
        assert!(
            report.insufficient_frames.is_empty(),
            "a 1-frame seated must not be flagged; got {:?}",
            report.insufficient_frames
        );
    }

    #[test]
    fn multi_frame_requirements_all_name_known_animations() {
        let known: std::collections::HashSet<&str> = registered_animation_names().collect();
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
