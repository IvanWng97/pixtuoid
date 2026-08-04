use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::sprite::{Frame, Palette, Pixel, Rgb, Sprite};

/// Parse a `.sprite` text file. Returns one Frame per `@frame N` block.
pub fn parse_sprite_file(src: &str, palette: &Palette) -> Result<Vec<Frame>> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut current: Option<Vec<Vec<Pixel>>> = None;
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

    #[test]
    fn recolor_palette_rejects_colliding_recolor_keys() {
        let red = Some(Rgb { r: 200, g: 0, b: 0 });
        let mut ok = Palette::new();
        ok.insert('B', red);
        ok.insert('H', Some(Rgb { r: 0, g: 200, b: 0 }));
        ok.insert('S', Some(Rgb { r: 0, g: 0, b: 200 }));
        ok.insert('P', None);
        assert!(validate_recolor_palette(&ok).is_ok());

        let mut bad = ok.clone();
        bad.insert('H', red);
        let err = validate_recolor_palette(&bad).unwrap_err();
        assert!(format!("{err:#}").contains("share RGB"), "{err:#}");

        let mut other = ok.clone();
        other.insert('X', red);
        let err = validate_recolor_palette(&other).unwrap_err();
        assert!(format!("{err:#}").contains("share RGB"), "{err:#}");

        let mut fine = ok.clone();
        fine.insert('X', Some(Rgb { r: 1, g: 2, b: 3 }));
        fine.insert('q', None);
        assert!(validate_recolor_palette(&fine).is_ok());
    }
}

fn parse_row(line: &str, palette: &Palette) -> Result<Vec<Pixel>> {
    let mut out = Vec::new();
    for tok in line.split_whitespace() {
        let mut chars = tok.chars();
        let key = chars.next().ok_or_else(|| anyhow!("empty token"))?;
        if chars.next().is_some() {
            bail!("each pixel must be a single character (got {tok:?})");
        }
        let px = palette
            .get(key)
            .ok_or_else(|| anyhow!("unknown palette key '{key}'"))?;
        out.push(px);
    }
    Ok(out)
}

fn rows_to_frame(rows: Vec<Vec<Pixel>>) -> Result<Frame> {
    if rows.is_empty() {
        bail!("frame has no rows");
    }
    // Frame dims are u16: a silent `as u16` truncation would wrap them while
    // `pixels` keeps the full flattened length, breaking Frame's
    // `pixels.len() == width * height` contract that blit/mirror index against.
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
    let pixels = rows.into_iter().flatten().collect();
    Ok(Frame::from_pixels(width, height, pixels))
}

#[derive(Debug, Deserialize)]
struct PackToml {
    pack: PackMeta,
    palette: HashMap<String, String>,
    animations: HashMap<String, AnimationToml>,
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
    /// The shared color palette its frames reference by single-char code.
    pub palette: Palette,
    animations: HashMap<String, Sprite>,
}

impl Pack {
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
    pub fn max_density_variant(&self) -> u16 {
        self.animations
            .keys()
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
    let palette = build_palette(&parsed.palette)?;
    validate_recolor_palette(&palette)?;
    let mut animations = HashMap::new();
    for (anim_name, anim) in parsed.animations {
        let mut frames = Vec::new();
        for fname in &anim.frames {
            let src = get_src(fname)?;
            let mut decoded =
                parse_sprite_file(&src, &palette).with_context(|| format!("decoding {fname}"))?;
            frames.append(&mut decoded);
        }
        animations.insert(
            anim_name,
            Sprite {
                frames,
                frame_ms: anim.frame_ms,
            },
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

/// The base palette keys per-agent recoloring substitutes by RGB equality
/// (shirt/hair/skin/pants) — the single source of truth for the tui's
/// `recolor_frame` and for `validate_recolor_palette`. They MUST map to
/// distinct RGBs: if two share a color, recolor swaps only the first and the
/// other silently keeps the wrong color.
pub const RECOLOR_KEYS: [char; 4] = ['B', 'H', 'S', 'P'];

/// Fail a pack where `recolor_frame`'s by-RGB substitution would be ambiguous:
/// it swaps EVERY opaque pixel matching a recolor base, so a NON-recolor key
/// sharing that RGB would be recolored to the agent's color too. Transparent
/// keys never participate.
fn validate_recolor_palette(palette: &Palette) -> Result<()> {
    let mut recolor_rgb: HashMap<Rgb, char> = HashMap::new();
    for key in RECOLOR_KEYS {
        if let Some(Some(rgb)) = palette.get(key) {
            if let Some(prev) = recolor_rgb.insert(rgb, key) {
                bail!(
                    "palette recolor keys '{prev}' and '{key}' share RGB {rgb:?}; \
                     per-agent recoloring substitutes by color and needs them distinct"
                );
            }
        }
    }
    for (key, pixel) in palette.iter() {
        if RECOLOR_KEYS.contains(&key) {
            continue;
        }
        if let Some(rgb) = pixel {
            if let Some(&base) = recolor_rgb.get(&rgb) {
                bail!(
                    "non-recolor palette key '{key}' and recolor key '{base}' share RGB \
                     {rgb:?}; per-agent recoloring substitutes by color and would recolor \
                     '{key}' too — give it a distinct color"
                );
            }
        }
    }
    Ok(())
}

fn build_palette(map: &HashMap<String, String>) -> Result<Palette> {
    let mut palette = Palette::new();
    for (k, v) in map {
        let mut it = k.chars();
        let key = it.next();
        let (Some(key), None) = (key, it.next()) else {
            bail!("palette key {k:?} must be exactly one character");
        };
        let pixel = parse_palette_value(v).with_context(|| format!("palette key '{k}'"))?;
        palette.insert(key, pixel);
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
/// gracefully (`side_seated` falls back to the front `seated` pose).
pub const OPTIONAL_CHARACTER_ANIMATIONS: &[&str] = &["walking_coffee", "side_seated"];

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
    format!("{base}{DENSITY_VARIANT_SEP}{density}x")
}

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
    (n >= 2).then_some((base, n))
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
}

impl ValidationReport {
    /// True when the pack is unusable — a required animation is missing, one
    /// has too few frames, or a density variant is not the size it claims.
    /// Missing OPTIONAL animations do not count.
    pub fn has_errors(&self) -> bool {
        !self.missing_required.is_empty()
            || !self.insufficient_frames.is_empty()
            || !self.mismatched_density.is_empty()
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
    // while every render consumer guards with `.frames.first()` and silently
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
            if anim.frames.len() < min_frames {
                report
                    .insufficient_frames
                    .push((name.to_string(), min_frames, anim.frames.len()));
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
        let (Some(art), Some(base_art)) = (
            pack.animation(name).and_then(|a| a.frames.first()),
            pack.animation(base).and_then(|a| a.frames.first()),
        ) else {
            continue;
        };
        let claimed = (base_art.width() * density, base_art.height() * density);
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
    /// `desk` but no `_hi` variant must still inherit the default's, or the
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
