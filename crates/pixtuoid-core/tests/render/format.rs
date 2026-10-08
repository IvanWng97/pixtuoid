use pixtuoid_core::sprite::error::{LineError, PackError, SpriteError};
use pixtuoid_core::sprite::format::{
    Pack, PackContract, load_pack_from_strings, validate_pack_animations,
};
use pixtuoid_core::sprite::{Frame, Rgb};

/// The `mini_pack` fixture: one one-frame animation, short of what a pack needs.
fn mini_pack() -> Pack {
    load_pack_from_strings(
        include_str!("fixtures/mini_pack/pack.toml"),
        &[(
            "idle.sprite",
            include_str!("fixtures/mini_pack/idle.sprite"),
        )],
    )
    .expect("mini pack loads")
}

/// `src` as the one sprite file of a pack whose palette is `A`, `B` and `.`.
fn parse(src: &str) -> anyhow::Result<Vec<Frame>> {
    let pack = load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n\
         [palette]\n\"A\"=\"#010203\"\n\"B\"=\"#040506\"\n\".\"=\"transparent\"\n\
         [animations.idle]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        &[("f.sprite", src)],
    )?;
    Ok(pack.animation("idle").expect("idle").frames().to_vec())
}

/// A caller can match the failure, and `{:#}` names each step of it once.
#[test]
fn a_bad_pixel_is_matchable_and_its_chain_prints_each_step_once() {
    let err = load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
         [animations.idle]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        &[("f.sprite", "@frame 0\nA z")],
    )
    .unwrap_err();
    assert!(
        matches!(
            &err,
            PackError::Decode {
                file,
                source: SpriteError::Line {
                    line: 2,
                    kind: LineError::UnknownKey { key: 'z', .. },
                    ..
                },
                ..
            } if file == "f.sprite"
        ),
        "{err:?}"
    );
    assert_eq!(
        format!("{:#}", anyhow::Error::from(err)),
        "decoding f.sprite: unknown palette key 'z' (line 2)"
    );
}

#[test]
fn a_shape_error_names_the_line_at_fault() {
    let line_of = |sprite: &str| match load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
         [animations.idle]\nframes=[\"f.sprite\"]\nframe_ms=100\n",
        &[("f.sprite", sprite)],
    )
    .unwrap_err()
    {
        PackError::Decode {
            source: SpriteError::Line { line, .. },
            ..
        } => line,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        line_of("@frame 0\nA A\nA\nA A\n@frame 1\nA A"),
        3,
        "the ragged row"
    );
    assert_eq!(
        line_of("@frame 0\n@mark hat 5 0\nA A\nA A\n"),
        2,
        "the mark outside"
    );
    assert_eq!(
        line_of("@frame 0\nA\n@frame 1\n@frame 2\nA"),
        3,
        "the empty frame"
    );
}

#[test]
fn parses_two_frame_mini_sprite() {
    let src = std::fs::read_to_string("tests/render/fixtures/mini.sprite").unwrap();
    let frames = parse(&src).unwrap();

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
    let src = "@frame 0\nA . ? .";
    let err = parse(src).unwrap_err();
    assert!(
        format!("{err:#}").contains("unknown palette key"),
        "got: {err:#}"
    );
}

#[test]
fn rejects_inconsistent_row_widths() {
    let src = "@frame 0\nA . B .\nA . B";
    let err = parse(src).unwrap_err();
    assert!(format!("{err:#}").contains("row width"), "got: {err:#}");
}

#[test]
fn rejects_empty_source_with_no_frames() {
    let err = parse("").unwrap_err();
    assert!(
        format!("{err:#}").contains("contains no frames"),
        "empty source must bail with 'contains no frames'; got: {err:#}"
    );
}

#[test]
fn rejects_multi_char_pixel_token() {
    let err = parse("@frame 0\nAB . .").unwrap_err();
    assert!(
        format!("{err:#}").contains("single character"),
        "a multi-char pixel token must bail; got: {err:#}"
    );
}

#[test]
fn rejects_frame_block_with_no_rows() {
    // Back-to-back @frame headers: the first block has zero rows.
    let err = parse("@frame 0\n@frame 1\nA").unwrap_err();
    assert!(
        format!("{err:#}").contains("no rows"),
        "an empty frame block must bail with 'frame has no rows'; got: {err:#}"
    );
}

#[test]
fn rejects_palette_key_longer_than_one_char() {
    let pack_toml = "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"AB\"=\"#010203\"\n\
         [animations.idle]\nframes=[\"i.sprite\"]\nframe_ms=100\n";
    let err = anyhow::Error::from(
        load_pack_from_strings(pack_toml, &[("i.sprite", "@frame 0\nA")]).unwrap_err(),
    );
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
    let err = anyhow::Error::from(
        load_pack_from_strings(pack_toml, &[("i.sprite", "@frame 0\nA")]).unwrap_err(),
    );
    assert!(
        format!("{err:#}").contains("6 hex digits"),
        "a non-6-hex-digit color must bail; got: {err:#}"
    );
}

#[test]
fn validate_reports_insufficient_frames_for_single_frame_typing() {
    // `typing` requires >= 2 frames (MULTI_FRAME_REQUIREMENTS).
    let pack_toml = "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"A\"=\"#010203\"\n\
         [animations.typing]\nframes=[\"t.sprite\"]\nframe_ms=100\n";
    let pack = load_pack_from_strings(pack_toml, &[("t.sprite", "@frame 0\nA")]).unwrap();
    let report = validate_pack_animations(&pack, &PackContract::default());
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
    let pack = mini_pack();
    let idle = pack.animation("idle").expect("idle animation");
    assert_eq!(idle.frame_ms(), 500);
    assert_eq!(idle.frames().len(), 1);
    assert_eq!(idle.frames()[0].width(), 4);
}

#[test]
fn missing_animation_returns_none() {
    let pack = mini_pack();
    assert!(pack.animation("nope").is_none());
}

#[test]
fn mini_pack_reports_missing_required() {
    let pack = mini_pack();
    let report = validate_pack_animations(&pack, &PackContract::default());
    assert!(
        !report.missing_required.is_empty(),
        "mini pack should be missing required animations"
    );
    assert!(report.has_errors());
}

#[test]
fn frame_wider_than_u16_max_errors_instead_of_truncating() {
    // The width half of `push_row`'s u16 guard.
    let mut src = String::with_capacity(2 * (u16::MAX as usize + 2) + 16);
    src.push_str("@frame 0\n");
    for _ in 0..=u16::MAX as usize {
        src.push_str("A ");
    }
    src.push('\n');
    let err = parse(&src).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("width") && msg.contains("line"),
        "oversized width must error with line context, got: {msg}"
    );
}

#[test]
fn frame_taller_than_u16_max_errors_instead_of_truncating() {
    // Its row-count half.
    let mut src = String::with_capacity(2 * (u16::MAX as usize + 2) + 16);
    src.push_str("@frame 0\n");
    for _ in 0..=u16::MAX as usize {
        src.push_str("A\n");
    }
    let err = parse(&src).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("rows") && msg.contains("line"),
        "oversized height must error with line context, got: {msg}"
    );
}
