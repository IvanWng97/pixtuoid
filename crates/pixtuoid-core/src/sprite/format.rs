use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::sprite::{Frame, Palette, Pixel, Rgb, Sprite};

/// Parse a `.sprite` text file. Returns one Frame per `@frame N` block.
pub fn parse_sprite_file(src: &str, palette: &Palette) -> Result<Vec<Frame>> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut current: Option<Vec<Vec<Pixel>>> = None;
    let mut last_lineno = 0; // last non-empty content line — for the final-frame flush diagnostic

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
        // Wrap with line context like the in-loop flush, so an inconsistent-width
        // or empty final @frame block gets the same diagnostic as every other.
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
        // u8::from_str_radix accepts a leading '+'/'-', so a 6-char value like
        // `#+f0102` would parse to a valid color without an explicit hex check.
        assert!(parse_palette_value("#+f0102").is_err());
        assert!(parse_palette_value("#-f0102").is_err());
        assert!(parse_palette_value("#abXY12").is_err());
        // Valid hex + transparent still parse.
        assert!(parse_palette_value("#Ff0102").unwrap().is_some());
        assert!(parse_palette_value("transparent").unwrap().is_none());
    }

    #[test]
    fn final_frame_width_error_carries_line_context() {
        let mut pal = Palette::new();
        pal.insert('X', Some(Rgb { r: 1, g: 1, b: 1 }));
        // The LAST @frame block has rows of differing widths → rows_to_frame
        // errors; the diagnostic must carry a line number like in-loop frames.
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
        // Distinct recolor keys (+ a transparent one, which never participates) → ok.
        let mut ok = Palette::new();
        ok.insert('B', red);
        ok.insert('H', Some(Rgb { r: 0, g: 200, b: 0 }));
        ok.insert('S', Some(Rgb { r: 0, g: 0, b: 200 }));
        ok.insert('P', None);
        assert!(validate_recolor_palette(&ok).is_ok());

        // Two recolor keys sharing an RGB → bail (recolor would silently fail one).
        let mut bad = ok.clone();
        bad.insert('H', red); // collides with 'B'
        let err = validate_recolor_palette(&bad).unwrap_err();
        assert!(format!("{err:#}").contains("share RGB"), "{err:#}");

        // A NON-recolor key sharing a recolor key's RGB is REJECTED: `recolor_frame`
        // substitutes by RGB equality over EVERY pixel, so an 'X' pixel equal to
        // 'B' would get swapped to the agent's shirt color (artifacts). The load
        // guard must cover this, not just recolor-vs-recolor — else a custom
        // `--pack-dir` pack ships artifacts while `validate-pack` prints OK.
        let mut other = ok.clone();
        other.insert('X', red); // collides with the recolor base 'B'
        let err = validate_recolor_palette(&other).unwrap_err();
        assert!(format!("{err:#}").contains("share RGB"), "{err:#}");

        // A non-recolor key with its OWN distinct color, or a transparent one, is fine.
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
    // Frame dims are u16; a silent `as u16` truncation would wrap the dims
    // while `pixels` keeps the full flattened length, breaking Frame's
    // documented `pixels.len() == width * height` contract that blit/mirror
    // index against. Pathological pack input only — bail like the
    // inconsistent-row-width case below.
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

#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub version: String,
    pub palette: Palette,
    animations: HashMap<String, Sprite>,
}

impl Pack {
    pub fn animation(&self, key: &str) -> Option<&Sprite> {
        self.animations.get(key)
    }

    pub fn animation_names(&self) -> Vec<String> {
        self.animations.keys().cloned().collect()
    }

    /// Merge furniture/environment animations from `base` into self.
    /// Only fills animations listed in OPTIONAL_FURNITURE_ANIMATIONS —
    /// character animations are never inherited so a robot pack doesn't
    /// accidentally show human sprites for missing optional poses.
    pub fn merge_from(&mut self, base: &Pack) {
        for &name in OPTIONAL_FURNITURE_ANIMATIONS {
            if !self.animations.contains_key(name) {
                if let Some(sprite) = base.animations.get(name) {
                    self.animations.insert(name.to_string(), sprite.clone());
                }
            }
        }
    }
}

/// Assemble a `Pack` from parsed TOML, resolving each frame's source text via
/// `get_src(frame_name)`. The two public loaders differ ONLY in how a frame's
/// text is fetched — filesystem IO (with the path-traversal guard) for
/// [`load_pack`] vs an in-memory lookup for [`load_pack_from_strings`] — so that
/// closure is the one thing they don't share. The traversal guard MUST stay
/// inside `load_pack`'s closure: `load_pack_from_strings` has no filesystem and
/// no untrusted paths to escape.
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
        // Path-traversal guard — load from disk only within the pack dir.
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
/// (shirt/hair/skin/pants). The SINGLE source of truth: the tui's `recolor_frame`
/// consumes this exact set, and `validate_recolor_palette` guards it — so the
/// substitution and the guard can't drift (add a 5th key here, once). They MUST
/// map to distinct RGBs: if two share a color, recolor swaps only the first and
/// the other key silently fails (no panic — just the wrong color on the
/// overlapping character). Enforced at LOAD so a `--pack-dir` custom pack can't
/// violate it undetectably — the embedded pack is also test-pinned.
pub const RECOLOR_KEYS: [char; 4] = ['B', 'H', 'S', 'P'];

/// Fail a pack where `recolor_frame`'s by-RGB substitution would be ambiguous.
/// `recolor_frame` swaps EVERY opaque pixel whose RGB equals a recolor key's base
/// color, so the guard must reject BOTH (a) two recolor keys sharing an RGB (one
/// would silently fail) AND (b) any opaque NON-recolor key sharing a recolor
/// base's RGB (its pixels would be recolored to the agent's color — artifacts).
/// Only opaque (`Some(rgb)`) keys participate; a transparent key isn't substituted.
fn validate_recolor_palette(palette: &Palette) -> Result<()> {
    // First map each recolor base RGB → its key, rejecting recolor-vs-recolor
    // collisions as we go.
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
    // Then reject any opaque non-recolor key colliding with a recolor base.
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
        // Validate-and-extract in one fallible step so the single-char invariant
        // and the bail can't drift apart in a refactor (no positional expect).
        let (Some(key), None) = (key, it.next()) else {
            bail!("palette key {k:?} must be exactly one character");
        };
        let pixel = parse_palette_value(v).with_context(|| format!("palette key '{k}'"))?;
        palette.insert(key, pixel);
    }
    Ok(palette)
}

// ---------------------------------------------------------------------------
// Animation registry — canonical list of animation names the renderer uses.
// ---------------------------------------------------------------------------

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

pub const OPTIONAL_CHARACTER_ANIMATIONS: &[&str] = &["walking_coffee"];

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
    "tv_stand",
    "phone_booth",
    "standing_desk",
    "bulletin_board",
    "exit_sign",
];

/// Multi-frame requirements: animations that must have at least N frames.
const MULTI_FRAME_REQUIREMENTS: &[(&str, usize)] = &[
    ("typing", 2),
    ("walking", 2),
    ("walking_back", 2),
    ("door", 3),
    ("cat_walk", 2),
    ("dog_walk", 2),
    ("lobster_walk", 2),
];

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub missing_required: Vec<String>,
    pub missing_optional: Vec<String>,
    pub insufficient_frames: Vec<(String, usize, usize)>,
    pub unknown: Vec<String>,
}

impl ValidationReport {
    pub fn has_errors(&self) -> bool {
        !self.missing_required.is_empty() || !self.insufficient_frames.is_empty()
    }
}

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

    // Frame-count floor: every KNOWN animation PRESENT in the pack needs at
    // least one frame — a `frames = []` entry deserializes and makes
    // `animation()` return Some (dodging the missing-required check above)
    // while every render consumer guards with `.frames.first()` and silently
    // draws nothing; an empty OPTIONAL entry additionally SHADOWS the embedded
    // default in `Pack::merge_from` (`contains_key` is true). Names in
    // MULTI_FRAME_REQUIREMENTS carry their own higher minimum.
    for name in known_names() {
        let min_frames = MULTI_FRAME_REQUIREMENTS
            .iter()
            .find(|&&(n, _)| n == name)
            .map_or(1, |&(_, min)| min);
        if let Some(anim) = pack.animation(name) {
            if anim.frames.len() < min_frames {
                report
                    .insufficient_frames
                    .push((name.to_string(), min_frames, anim.frames.len()));
            }
        }
    }

    let all_known: std::collections::HashSet<&str> = known_names().collect();
    for name in pack.animation_names() {
        if !all_known.contains(name.as_str()) {
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

    #[test]
    fn empty_frames_on_a_required_animation_fails_validation() {
        // `frames = []` deserializes, build_pack inserts `Sprite { frames:
        // vec![] }`, and `animation()` returns Some — dodging the
        // missing-required check — while every render consumer guards with
        // `.frames.first()` and silently draws nothing. An empty frame list
        // on a known animation must be a hard validation error (implicit
        // min-1), so `pixtuoid validate-pack` catches the exact authoring
        // mistake it exists for.
        let pack = pack_with_animation("seated", "[]");
        let report = validate_pack_animations(&pack);
        assert!(
            report
                .insufficient_frames
                .contains(&("seated".to_string(), 1, 0)),
            "empty seated must report (seated, 1, 0); got {:?}",
            report.insufficient_frames
        );
        assert!(report.has_errors());
        // Not double-counted as missing — the entry exists.
        assert!(!report.missing_required.contains(&"seated".to_string()));
    }

    #[test]
    fn empty_frames_on_an_optional_furniture_animation_fails_validation() {
        // An empty OPTIONAL entry is worse than an absent one: `merge_from`
        // skips the embedded-default fallback because `contains_key` is true,
        // so the empty animation SHADOWS the default furniture sprite.
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
        // The implicit floor is min-1 — a single-frame animation outside
        // MULTI_FRAME_REQUIREMENTS must not be flagged as insufficient.
        // (The pack still misses OTHER required animations; only the
        // frame-count floor is under test here.)
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
        // The frame-count floor iterates the KNOWN animation lists and reads
        // each name's stricter minimum from MULTI_FRAME_REQUIREMENTS — a row
        // naming an unknown animation would silently never be checked.
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
    // u8::from_str_radix accepts a leading '+', so reject non-hex bytes up front
    // — otherwise `#+f0102` would slice to `+f`/`01`/`02` and parse to a valid
    // color, violating the "must be 6 hex digits" contract on untrusted packs.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("color {v:?} must be 6 hex digits");
    }
    let r = u8::from_str_radix(&hex[0..2], 16)?;
    let g = u8::from_str_radix(&hex[2..4], 16)?;
    let b = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Some(Rgb { r, g, b }))
}
