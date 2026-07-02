use std::path::Path;

use pixtuoid_core::sprite::format::{
    load_pack, load_pack_from_strings, parse_sprite_file, validate_pack_animations,
};
use pixtuoid_core::sprite::{Palette, Rgb};

fn palette() -> Palette {
    let mut p = Palette::new();
    p.insert('A', Some(Rgb { r: 1, g: 2, b: 3 }));
    p.insert('B', Some(Rgb { r: 4, g: 5, b: 6 }));
    p.insert('.', None);
    p
}

#[test]
fn parses_two_frame_mini_sprite() {
    let src = std::fs::read_to_string("tests/render/fixtures/mini.sprite").unwrap();
    let frames = parse_sprite_file(&src, &palette()).unwrap();

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].width(), 4);
    assert_eq!(frames[0].height(), 2);
    assert_eq!(frames[0].as_slice()[0], Some(Rgb { r: 1, g: 2, b: 3 }));
    assert_eq!(frames[0].as_slice()[1], None);
    assert_eq!(frames[0].as_slice()[2], Some(Rgb { r: 4, g: 5, b: 6 }));
    assert_eq!(frames[0].as_slice()[3], None);
}

#[test]
fn rejects_unknown_palette_key() {
    let palette = palette();
    let src = "@frame 0\nA . ? .";
    let err = parse_sprite_file(src, &palette).unwrap_err();
    assert!(
        err.to_string().contains("unknown palette key"),
        "got: {err}"
    );
}

#[test]
fn rejects_inconsistent_row_widths() {
    let palette = palette();
    let src = "@frame 0\nA . B .\nA . B";
    let err = parse_sprite_file(src, &palette).unwrap_err();
    assert!(err.to_string().contains("row width"), "got: {err}");
}

// ---- malformed-input error bails (the guards the parser exists to enforce) --

#[test]
fn rejects_empty_source_with_no_frames() {
    let err = parse_sprite_file("", &palette()).unwrap_err();
    assert!(
        err.to_string().contains("contains no frames"),
        "empty source must bail with 'contains no frames'; got: {err}"
    );
}

#[test]
fn rejects_multi_char_pixel_token() {
    // "AB" is two characters in one whitespace-delimited token.
    let err = parse_sprite_file("@frame 0\nAB . .", &palette()).unwrap_err();
    assert!(
        err.to_string().contains("single character"),
        "a multi-char pixel token must bail; got: {err}"
    );
}

#[test]
fn rejects_frame_block_with_no_rows() {
    // Back-to-back @frame headers: the first block has zero rows, so the
    // header-handling `rows_to_frame(vec![])` path bails with 'no rows'.
    let err = parse_sprite_file("@frame 0\n@frame 1\nA", &palette()).unwrap_err();
    assert!(
        err.to_string().contains("no rows"),
        "an empty frame block must bail with 'frame has no rows'; got: {err}"
    );
}

#[test]
fn rejects_palette_key_longer_than_one_char() {
    // build_palette is reached via load_pack_from_strings; key "AB" is 2 chars.
    let pack_toml = "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"AB\"=\"#010203\"\n\
         [animations.idle]\nframes=[\"i.sprite\"]\nframe_ms=100\n";
    let err = load_pack_from_strings(pack_toml, &[("i.sprite", "@frame 0\nA")]).unwrap_err();
    assert!(
        format!("{err:#}").contains("exactly one character"),
        "a >1-char palette key must bail; got: {err:#}"
    );
}

#[test]
fn rejects_palette_value_not_six_hex_digits() {
    let pack_toml = "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"A\"=\"#12345\"\n\
         [animations.idle]\nframes=[\"i.sprite\"]\nframe_ms=100\n";
    let err = load_pack_from_strings(pack_toml, &[("i.sprite", "@frame 0\nA")]).unwrap_err();
    assert!(
        format!("{err:#}").contains("6 hex digits"),
        "a non-6-hex-digit color must bail; got: {err:#}"
    );
}

#[test]
fn validate_reports_insufficient_frames_for_single_frame_typing() {
    // `typing` requires >= 2 frames (MULTI_FRAME_REQUIREMENTS). A 1-frame
    // typing animation must populate insufficient_frames and set has_errors().
    let pack_toml = "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"A\"=\"#010203\"\n\
         [animations.typing]\nframes=[\"t.sprite\"]\nframe_ms=100\n";
    let pack = load_pack_from_strings(pack_toml, &[("t.sprite", "@frame 0\nA")]).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        report
            .insufficient_frames
            .contains(&("typing".to_string(), 2, 1)),
        "single-frame typing must report (typing, 2, 1); got: {:?}",
        report.insufficient_frames
    );
    assert!(report.has_errors());
}

#[test]
fn loads_mini_pack() {
    let pack = load_pack(Path::new("tests/render/fixtures/mini_pack")).unwrap();
    let idle = pack.animation("idle").expect("idle animation");
    assert_eq!(idle.frame_ms, 500);
    assert_eq!(idle.frames.len(), 1);
    assert_eq!(idle.frames[0].width(), 4);
}

#[test]
fn missing_animation_returns_none() {
    let pack = load_pack(Path::new("tests/render/fixtures/mini_pack")).unwrap();
    assert!(pack.animation("nope").is_none());
}

#[test]
fn default_pack_loads_with_required_animations() {
    let pack = load_pack(Path::new("../pixtuoid-scene/sprites/default")).unwrap();
    for name in &[
        "seated",
        "typing",
        "standing",
        "walking",
        "desk",
        "plant",
        "pantry",
        "whiteboard",
        "bookshelf",
    ] {
        assert!(pack.animation(name).is_some(), "missing animation: {name}");
    }
    let seated = pack.animation("seated").unwrap();
    assert_eq!(seated.frames[0].width(), 8);
    assert_eq!(seated.frames[0].height(), 10);

    let standing = pack.animation("standing").unwrap();
    assert_eq!(standing.frames[0].width(), 8);
    assert_eq!(standing.frames[0].height(), 12);

    let walking = pack.animation("walking").unwrap();
    assert_eq!(walking.frames.len(), 2);
}

#[test]
fn default_pack_passes_validation() {
    let pack = load_pack(Path::new("../pixtuoid-scene/sprites/default")).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        report.missing_required.is_empty(),
        "missing required: {:?}",
        report.missing_required
    );
    assert!(
        report.insufficient_frames.is_empty(),
        "insufficient frames: {:?}",
        report.insufficient_frames
    );
}

#[test]
fn robot_pack_passes_validation() {
    let pack = load_pack(Path::new("../pixtuoid/sprites/robot")).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        report.missing_required.is_empty(),
        "missing required: {:?}",
        report.missing_required
    );
    assert!(
        report.insufficient_frames.is_empty(),
        "insufficient frames: {:?}",
        report.insufficient_frames
    );
}

#[test]
fn skeleton_pack_passes_validation() {
    let pack = load_pack(Path::new("../pixtuoid/sprites/skeleton")).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        report.missing_required.is_empty(),
        "missing required: {:?}",
        report.missing_required
    );
}

#[test]
fn mini_pack_reports_missing_required() {
    let pack = load_pack(Path::new("tests/render/fixtures/mini_pack")).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        !report.missing_required.is_empty(),
        "mini pack should be missing required animations"
    );
    assert!(report.has_errors());
}

#[test]
fn validation_detects_unknown_animations() {
    let pack = load_pack(Path::new("tests/render/fixtures/mini_pack")).unwrap();
    let report = validate_pack_animations(&pack);
    assert!(
        report.unknown.contains(&"idle".to_string()),
        "mini pack's 'idle' animation should be flagged as unknown"
    );
}

// ---- path-traversal guards (security-relevant; previously untested) -------

fn write_pack_files(dir: &Path, pack_toml: &str, frames: &[(&str, &str)]) {
    std::fs::write(dir.join("pack.toml"), pack_toml).unwrap();
    for (name, content) in frames {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

#[test]
fn load_pack_rejects_parent_dir_frame_path() {
    let dir = tempfile::TempDir::new().unwrap();
    write_pack_files(
        dir.path(),
        "[pack]\nname=\"x\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
         [animations.idle]\nframes=[\"../escape.sprite\"]\nframe_ms=100\n",
        &[],
    );
    let err = load_pack(dir.path()).unwrap_err();
    assert!(
        format!("{err:#}").contains("not allowed"),
        "a '..' frame path must be rejected; got: {err:#}"
    );
}

#[test]
fn load_pack_rejects_absolute_frame_path_escaping_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    // `dir.join("/etc/hosts")` resolves to `/etc/hosts` (Path::join replaces on a
    // leading '/'), which the '..'-component check does NOT catch — the
    // canonicalize + starts_with(canon_dir) guard is what must reject it.
    write_pack_files(
        dir.path(),
        "[pack]\nname=\"x\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
         [animations.idle]\nframes=[\"/etc/hosts\"]\nframe_ms=100\n",
        &[],
    );
    let err = load_pack(dir.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("escapes the pack directory") || msg.contains("resolving"),
        "an absolute path escaping the pack dir must be rejected; got: {msg}"
    );
}

// ---- Pack::merge_from (load-bearing for every custom pack; was untested) --

#[test]
fn merge_from_inherits_furniture_only_and_never_clobbers_own() {
    // base: a furniture anim custom lacks (plant), a furniture anim custom owns
    // (desk, wider here), and a REQUIRED_CHARACTER anim (standing).
    let base_dir = tempfile::TempDir::new().unwrap();
    write_pack_files(
        base_dir.path(),
        "[pack]\nname=\"base\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\"A\"=\"#010203\"\n\
         [animations.desk]\nframes=[\"d.sprite\"]\nframe_ms=100\n\
         [animations.plant]\nframes=[\"p.sprite\"]\nframe_ms=100\n\
         [animations.standing]\nframes=[\"s.sprite\"]\nframe_ms=100\n",
        &[
            ("d.sprite", "@frame 0\nA A"),
            ("p.sprite", "@frame 0\nA"),
            ("s.sprite", "@frame 0\nA"),
        ],
    );
    // custom: owns a 1-wide desk; lacks plant (furniture) and standing (character).
    let custom_dir = tempfile::TempDir::new().unwrap();
    write_pack_files(
        custom_dir.path(),
        "[pack]\nname=\"custom\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\"A\"=\"#010203\"\n\
         [animations.desk]\nframes=[\"d.sprite\"]\nframe_ms=100\n",
        &[("d.sprite", "@frame 0\nA")],
    );

    let base = load_pack(base_dir.path()).unwrap();
    let mut custom = load_pack(custom_dir.path()).unwrap();
    assert_eq!(custom.animation("desk").unwrap().frames[0].width(), 1);

    custom.merge_from(&base);

    // Own `desk` preserved (NOT clobbered by base's 2-wide desk).
    assert_eq!(
        custom.animation("desk").unwrap().frames[0].width(),
        1,
        "merge_from must not overwrite an animation the custom pack already defines"
    );
    // Furniture anim absent from custom is inherited from base.
    assert!(
        custom.animation("plant").is_some(),
        "missing OPTIONAL_FURNITURE anim should be inherited"
    );
    // REQUIRED_CHARACTER anim is NEVER inherited (a robot pack must not show
    // human sprites for poses it lacks).
    assert!(
        custom.animation("standing").is_none(),
        "REQUIRED_CHARACTER anim must never be inherited via merge_from"
    );
}

#[test]
fn frame_wider_than_u16_max_errors_instead_of_truncating() {
    // `rows_to_frame` casts the row width to u16; a 65536-wide row used to
    // wrap to width 0 against an untruncated 65536-long pixels vec, silently
    // violating Frame's `pixels.len() == width * height` contract.
    let mut src = String::with_capacity(2 * (u16::MAX as usize + 2) + 16);
    src.push_str("@frame 0\n");
    for _ in 0..=u16::MAX as usize {
        src.push_str("A ");
    }
    src.push('\n');
    let err = parse_sprite_file(&src, &palette()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("width") && msg.contains("line"),
        "oversized width must error with line context, got: {msg}"
    );
}

#[test]
fn frame_taller_than_u16_max_errors_instead_of_truncating() {
    // Same contract for the row count: 65536 rows wrapped height to 0.
    let mut src = String::with_capacity(2 * (u16::MAX as usize + 2) + 16);
    src.push_str("@frame 0\n");
    for _ in 0..=u16::MAX as usize {
        src.push_str("A\n");
    }
    let err = parse_sprite_file(&src, &palette()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("rows") && msg.contains("line"),
        "oversized height must error with line context, got: {msg}"
    );
}
