use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU16;
#[cfg(feature = "native")]
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use crate::grid::Grid;
use crate::sprite::error::{ColorError, KeySite, LineError, PackError, SpriteError};
use crate::sprite::{
    Frame, HEAD_MARK, HeadMark, HeadView, IndexedFrame, Mark, PALETTE_CAPACITY, Palette,
    PaletteIndex, Pixel, Rgb, Sprite,
};

type Result<T, E = PackError> = std::result::Result<T, E>;

struct Block {
    header: usize,
    rows: Vec<Vec<PaletteIndex>>,
    marks: Vec<(Mark, usize)>,
}

/// Parse a `.sprite` text file: one indexed frame per `@frame N` block, each
/// with the `@mark <name> <x> <y>` lines its block carries.
fn parse_indexed(
    src: &str,
    palette: &Palette,
) -> Result<Vec<(IndexedFrame, Vec<Mark>)>, SpriteError> {
    let mut frames = Vec::new();
    let mut current: Option<Block> = None;
    let at = |lineno: usize| {
        move |kind| SpriteError::Line {
            line: lineno + 1,
            kind,
        }
    };
    let finish = |block: Block| -> Result<(IndexedFrame, Vec<Mark>), SpriteError> {
        let frame = rows_to_frame(block.rows).map_err(at(block.header))?;
        let grid = &frame.0;
        if let Some((m, lineno)) = block
            .marks
            .iter()
            .find(|(m, _)| m.x() >= grid.width() || m.y() >= grid.height())
        {
            return Err(at(*lineno)(LineError::MarkOutside {
                name: m.name().to_owned(),
                x: m.x(),
                y: m.y(),
                width: grid.width(),
                height: grid.height(),
            }));
        }
        Ok((frame, block.marks.into_iter().map(|(m, _)| m).collect()))
    };

    for (lineno, raw) in src.lines().enumerate() {
        let line = strip_comment_and_trim(raw);
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("@frame") {
            if let Some(block) = current.take() {
                frames.push(finish(block)?);
            }
            let _ = rest
                .trim()
                .parse::<u32>()
                .map_err(|_| at(lineno)(LineError::FrameNumber))?;
            current = Some(Block {
                header: lineno,
                rows: Vec::new(),
                marks: Vec::new(),
            });
            continue;
        }

        let Block { rows, marks, .. } = current
            .as_mut()
            .ok_or_else(|| at(lineno)(LineError::DataBeforeFrame))?;

        if let Some(rest) = line.strip_prefix("@mark") {
            let mark = parse_mark(rest).map_err(at(lineno))?;
            let head = |m: &Mark| m.name().starts_with(HEAD_MARK);
            if marks
                .iter()
                .any(|(m, _)| m.name() == mark.name() || (head(m) && head(&mark)))
            {
                return Err(at(lineno)(LineError::DuplicateMark));
            }
            marks.push((mark, lineno));
            continue;
        }

        let row = parse_row(line, palette).map_err(at(lineno))?;
        push_row(rows, row).map_err(at(lineno))?;
    }

    if let Some(block) = current.take() {
        frames.push(finish(block)?);
    }

    if frames.is_empty() {
        return Err(SpriteError::NoFrames);
    }
    Ok(frames)
}

/// The fields of an `@mark <name> <x> <y>` line after its keyword. A head mark
/// must name a view.
fn parse_mark(fields: &str) -> Result<Mark, LineError> {
    let mut it = fields.split_whitespace();
    let (Some(name), Some(x), Some(y), None) = (it.next(), it.next(), it.next(), it.next()) else {
        return Err(LineError::MarkFields);
    };
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
    {
        return Err(LineError::MarkName {
            name: name.to_owned(),
        });
    }
    let coord = |v: &str| {
        v.parse::<u16>().map_err(|_| LineError::MarkCoordinate {
            value: v.to_owned(),
        })
    };
    let mark = Mark::new(name.to_owned(), coord(x)?, coord(y)?);
    if name.starts_with(HEAD_MARK) && HeadMark::of(&mark).is_none() {
        return Err(LineError::HeadWithoutView {
            name: name.to_owned(),
        });
    }
    Ok(mark)
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

    /// A pack of one animation plus `extra` tables, over a palette of the seven
    /// material keys and a stray `x`; `files` holds the buildings' sprites.
    fn city_pack(extra: &str, files: &[(&str, &str)]) -> Result<Pack> {
        let mut all = vec![("f.sprite", "@frame 0\nF\n")];
        all.extend_from_slice(files);
        load_pack_from_strings(
            &format!(
                "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
                 \"F\"=\"#202020\"\n\"f\"=\"#181818\"\n\"R\"=\"#303030\"\n\"W\"=\"#404040\"\n\
                 \"M\"=\"#101010\"\n\"D\"=\"#282828\"\n\"L\"=\"#d08050\"\n\"x\"=\"#ffffff\"\n\
                 [animations.seated]\nframes=[\"f.sprite\"]\nframe_ms=100\n{extra}"
            ),
            &all,
        )
    }

    const CITY: &str = "[city]\nfacade=\"F\"\nshade=\"f\"\nroof=\"R\"\nglass=\"W\"\n\
                        mullion=\"M\"\ndetail=\"D\"\nsign=\"L\"\n";
    /// A 2x3 tower: one window of two glass pixels over a wall.
    const TOWER: &str = "@frame 0\nR R\nW W\nF f\n";
    /// The tower at 2x, drawn with a mullion splitting its window in two.
    const TOWER_2X: &str = "@frame 0\nR R R R\nR R R R\nW M W W\nW M W W\nF F f f\nF F f f\n";

    #[test]
    fn a_building_is_its_base_and_its_density_variants() {
        let pack = city_pack(
            &format!(
                "{CITY}[buildings.tower]\nsprite=\"t.sprite\"\nplanes=[\"mid\", \"near\"]\n\
                 [buildings.\"tower@2x\"]\nsprite=\"t2.sprite\"\n"
            ),
            &[("t.sprite", TOWER), ("t2.sprite", TOWER_2X)],
        )
        .expect("the city pack loads");
        let tower = pack.buildings().next().expect("the tower");
        assert_eq!((tower.name(), tower.size()), ("tower", (2, 3)));
        assert!(tower.stands_in(CityPlane::Mid) && tower.stands_in(CityPlane::Near));
        let d = |n| Density::new(n).expect("nonzero");
        let base = tower.base();
        assert!(tower.variant(d(1)).is_none(), "the base is no variant");
        assert_eq!(
            base.windows(),
            [vec![(0, 1), (1, 1)]],
            "one run of glass, one window"
        );
        let dense = tower.variant(d(2)).expect("the 2x variant");
        assert_eq!(
            dense.windows().len(),
            2,
            "each density finds its own windows: the mullion splits this one"
        );
        assert!(tower.variant(d(4)).is_none());
        let materials = pack.city_materials().expect("[city]");
        assert_eq!(materials.key(Material::Glass), 'W');
    }

    #[test]
    fn every_material_has_its_own_place() {
        for (i, m) in Material::ALL.into_iter().enumerate() {
            assert_eq!(m.index(), i, "{m:?}");
        }
    }

    #[test]
    fn a_building_outside_its_rules_is_rejected() {
        let base = "[buildings.tower]\nsprite=\"t.sprite\"\nplanes=[\"near\"]\n";
        let load = |extra: &str, art: &str, dense: &str| {
            city_pack(extra, &[("t.sprite", art), ("t2.sprite", dense)])
        };
        let ok = format!("{CITY}{base}[buildings.\"tower@2x\"]\nsprite=\"t2.sprite\"\n");
        assert!(load(&ok, TOWER, TOWER_2X).is_ok());
        let rejected = [
            (
                format!("{CITY}{base}"),
                "@frame 0\nF x\n",
                "a key no material names",
            ),
            (base.to_owned(), TOWER, "no [city] to name the materials"),
            (
                format!("{CITY}[buildings.tower]\nsprite=\"t.sprite\"\nplanes=[\"far\"]\n"),
                TOWER,
                "the far plane is the painter's own",
            ),
            (
                format!("{CITY}[buildings.tower]\nsprite=\"t.sprite\"\n"),
                TOWER,
                "a base naming no planes",
            ),
            (
                format!("{CITY}[buildings.tower]\nsprite=\"t.sprite\"\nplanes=[]\n"),
                TOWER,
                "a base standing in no plane",
            ),
            (
                format!("{CITY}[buildings.\"tower@1x\"]\nsprite=\"t.sprite\"\nplanes=[\"near\"]\n"),
                TOWER,
                "a base named like a variant",
            ),
            (
                format!(
                    "{CITY}[buildings.\"tower@big\"]\nsprite=\"t.sprite\"\nplanes=[\"near\"]\n"
                ),
                TOWER,
                "a base named with a variant's mark",
            ),
            (
                format!("{CITY}{base}"),
                "@frame 0\nR R\nW W\nF f\n@frame 1\nR R\nW W\nF f\n",
                "a building of two frames",
            ),
            (
                format!("{}{base}", CITY.replace("sign=\"L\"", "sign=\".\"")),
                TOWER,
                "a material drawn in a transparent key",
            ),
            (
                format!("{CITY}[buildings.\"tower@2x\"]\nsprite=\"t2.sprite\"\n"),
                TOWER,
                "a variant with no base",
            ),
            (
                format!(
                    "{CITY}{base}[buildings.\"tower@2x\"]\nsprite=\"t2.sprite\"\nplanes=[\"mid\"]\n"
                ),
                TOWER,
                "a variant naming its own planes",
            ),
            (
                format!("{CITY}{base}[buildings.\"tower@2x\"]\nsprite=\"t.sprite\"\n"),
                TOWER,
                "a variant not twice its base",
            ),
            (
                format!("{}{base}", CITY.replace("sign=\"L\"", "sign=\"F\"")),
                TOWER,
                "two materials in one key",
            ),
            (
                format!("{}{base}", CITY.replace("sign=\"L\"\n", "")),
                TOWER,
                "a material with no key",
            ),
            (
                format!("{CITY}neon=\"L\"\n{base}"),
                TOWER,
                "a key naming no material",
            ),
        ];
        for (extra, art, why) in rejected {
            assert!(load(&extra, art, TOWER_2X).is_err(), "{why}");
        }
    }

    /// A pack whose one animation, `seated`, draws `f.sprite`, plus `extra`
    /// tables; `frames` holds every file by name.
    /// As a caller sees the failure: through `anyhow`, whose `{:#}` walks the
    /// chain.
    fn hair_pack(extra: &str, frames: &[(&str, &str)]) -> anyhow::Result<Pack> {
        let toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
             \"H\"=\"#28140a\"\n\"k\"=\"#101010\"\n\
             [animations.seated]\nframes=[\"f.sprite\"]\nframe_ms=100\n{extra}"
        );
        Ok(load_pack_from_strings(&toml, frames)?)
    }

    #[test]
    fn a_mark_names_a_point_of_its_own_frame() {
        let pack = hair_pack(
            "",
            &[(
                "f.sprite",
                "@frame 0\n@mark head.front 1 0\n@mark cup 0 1\nH H\nH H\n@frame 1\nH H\nH H\n",
            )],
        )
        .expect("loads");
        let seated = pack.animation("seated").expect("the animation");
        let head = seated.head(0).expect("frame 0's head");
        assert_eq!((head.view, head.x, head.y), (HeadView::Front, 1, 0));
        let names: Vec<_> = seated.marks(0).iter().map(Mark::name).collect();
        assert_eq!(names, ["head.front", "cup"]);
        assert_eq!(seated.head(1), None, "an unmarked frame has no head");
        assert!(seated.marks(1).is_empty());
    }

    #[test]
    fn a_mark_outside_its_frame_or_out_of_its_rules_is_rejected() {
        for bad in [
            "@mark cup 2 0",
            "@mark head.up 0 0",
            "@mark cup 0",
            "@mark Cup 0 0",
            "@mark cup 0 0\n@mark cup 1 0",
            "@mark head.front 0 0\n@mark head.back 1 0",
        ] {
            let err =
                hair_pack("", &[("f.sprite", &format!("@frame 0\n{bad}\nH H\n"))]).expect_err(bad);
            assert!(format!("{err:#}").contains("mark"), "{bad}: {err:#}");
        }
    }

    #[test]
    fn a_pack_without_a_city_inherits_the_whole_city() {
        let city = city_pack(
            &format!("{CITY}[buildings.tower]\nsprite=\"t.sprite\"\nplanes=[\"near\"]\n"),
            &[("t.sprite", TOWER)],
        )
        .expect("the city pack loads");
        let mut bare = city_pack("", &[]).expect("a pack with no city loads");
        bare.merge_from(&city);
        assert_eq!(bare.buildings().count(), 1, "its buildings");
        assert!(
            bare.city_materials().is_some(),
            "with the materials they are drawn in"
        );

        let mut own = city_pack(
            &format!("{CITY}[buildings.walkup]\nsprite=\"t.sprite\"\nplanes=[\"mid\"]\n"),
            &[("t.sprite", TOWER)],
        )
        .expect("a pack with its own city loads");
        own.merge_from(&city);
        let names: Vec<_> = own.buildings().map(Building::name).collect();
        assert_eq!(names, ["walkup"], "a city of its own is kept whole");
    }

    #[test]
    fn a_hairstyle_loads_its_layers_per_view_at_its_density() {
        let pack = hair_pack(
            "[characters]\noutline=\"k\"\n\
             [hairstyles.\"mop@2x\"]\nfront={ behind=\"b.sprite\", over=\"o.sprite\" }\nback={ over=\"o2.sprite\" }\n",
            &[
                ("f.sprite", "@frame 0\nH\n"),
                ("b.sprite", "@frame 0\n@mark head.front 0 1\nH\nH\n"),
                ("o.sprite", "@frame 0\n@mark head.front 0 0\nH\n"),
                ("o2.sprite", "@frame 0\n@mark head.back 0 0\nH\n"),
            ],
        )
        .expect("loads");
        let styles: Vec<_> = pack.hairstyles().collect();
        assert_eq!(styles.len(), 1);
        let mop = styles[0];
        assert_eq!((mop.name(), mop.density().get()), ("mop", 2));
        assert!(pack.hairstyle("mop", mop.density()).is_some());
        let front = mop.layers(HeadView::Front).expect("a front view");
        assert_eq!(front.behind().and_then(|l| l.head(0)).map(|h| h.y), Some(1));
        assert!(front.over().is_some());
        assert!(
            mop.layers(HeadView::Back)
                .is_some_and(|l| l.behind().is_none())
        );
        assert!(mop.layers(HeadView::Side).is_none());
        assert_eq!(
            pack.character_outline(),
            Some(Rgb {
                r: 16,
                g: 16,
                b: 16
            })
        );
    }

    #[test]
    fn a_hairstyle_out_of_its_rules_is_rejected() {
        let f = ("f.sprite", "@frame 0\nH\n");
        let front = ("o.sprite", "@frame 0\n@mark head.front 0 0\nH\n");
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
        let cases: [Case; 5] = [
            (
                "[hairstyles.\"mop@2x\"]\nfront={ over=\"o.sprite\" }\n",
                &[f, ("o.sprite", "@frame 0\n@mark head.back 0 0\nH\n")],
                "a back head on a front layer",
            ),
            (
                "[hairstyles.mop]\nfront={ over=\"o.sprite\" }\n",
                &[f, front],
                "a style without a density",
            ),
            (
                "[hairstyles.\"mop@2x\"]\nfront={ over=\"o.sprite\" }\n\
                 [hairstyles.\"bun@4x\"]\nfront={ over=\"o.sprite\" }\n",
                &[f, front],
                "styles differing between densities",
            ),
            (
                "[characters]\noutline=\".\"\n",
                &[f],
                "a transparent outline",
            ),
            (
                "[characters]\noutline=\"q\"\n",
                &[f],
                "an outline no palette key names",
            ),
        ];
        for (extra, files, why) in cases {
            assert!(hair_pack(extra, files).is_err(), "{why}");
        }
        assert!(
            hair_pack(
                "[hairstyles.\"mop@2x\"]\nfront={ over=\"o.sprite\" }\n\
                 [hairstyles.\"mop@4x\"]\nfront={ over=\"o.sprite\" }\n",
                &[f, front],
            )
            .is_ok(),
            "one style at two densities"
        );
    }

    #[test]
    fn merge_from_never_dresses_a_pack_in_anothers_styles() {
        let styled = hair_pack(
            "[hairstyles.\"mop@2x\"]\nfront={ over=\"o.sprite\" }\n",
            &[
                ("f.sprite", "@frame 0\nH\n"),
                ("o.sprite", "@frame 0\n@mark head.front 0 0\nH\n"),
            ],
        )
        .expect("loads");
        let mut bare = hair_pack("", &[("f.sprite", "@frame 0\nH\n")]).expect("loads");
        bare.merge_from(&styled);
        assert!(bare.hairstyles().next().is_none());
    }

    fn ramp_pack(palette: &str, ramps: &str, sprite: &str) -> anyhow::Result<Pack> {
        let toml = format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n{palette}\n\
             [ramps]\n{ramps}\n\
             [animations.seated]\nframes=[\"f.sprite\"]\nframe_ms=100\n"
        );
        Ok(load_pack_from_strings(&toml, &[("f.sprite", sprite)])?)
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

fn parse_row(line: &str, palette: &Palette) -> Result<Vec<PaletteIndex>, LineError> {
    let mut out = Vec::new();
    for tok in line.split_whitespace() {
        let mut chars = tok.chars();
        let (Some(key), None) = (chars.next(), chars.next()) else {
            return Err(LineError::PixelToken {
                token: tok.to_owned(),
            });
        };
        let index = palette
            .drawable_index(key)
            .ok_or(LineError::UnknownKey { key })?;
        let index =
            PaletteIndex::try_from(index).map_err(|_| LineError::KeyPastCapacity { key })?;
        out.push(index);
    }
    Ok(out)
}

/// Append `row` to a frame's `rows`, rejected where it breaks the frame's
/// shape, so the error names the row's own line.
fn push_row(rows: &mut Vec<Vec<PaletteIndex>>, row: Vec<PaletteIndex>) -> Result<(), LineError> {
    // Grid dims are u16: past them a frame's `data` would outgrow its grid.
    if u16::try_from(rows.len() + 1).is_err() {
        return Err(LineError::TooManyRows {
            rows: rows.len() + 1,
        });
    }
    match rows.first() {
        None if u16::try_from(row.len()).is_err() => {
            return Err(LineError::TooWide { width: row.len() });
        }
        Some(first) if row.len() != first.len() => {
            return Err(LineError::Ragged {
                row: rows.len(),
                expected: first.len(),
                got: row.len(),
            });
        }
        _ => {}
    }
    rows.push(row);
    Ok(())
}

fn rows_to_frame(rows: Vec<Vec<PaletteIndex>>) -> Result<IndexedFrame, LineError> {
    let height =
        u16::try_from(rows.len()).map_err(|_| LineError::TooManyRows { rows: rows.len() })?;
    let first = rows.first().ok_or(LineError::NoRows)?;
    let width =
        u16::try_from(first.len()).map_err(|_| LineError::TooWide { width: first.len() })?;
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
    #[serde(default)]
    city: Option<BTreeMap<String, String>>,
    #[serde(default)]
    buildings: BTreeMap<String, BuildingToml>,
    #[serde(default)]
    characters: Option<CharactersToml>,
    #[serde(default)]
    hairstyles: BTreeMap<String, HairstyleToml>,
}

/// One `[buildings.<name>]` table (the base art, with the planes the building
/// stands in) or `[buildings."<name>@<N>x"]` (a density variant of it).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildingToml {
    sprite: String,
    planes: Option<Vec<String>>,
}

/// The `[characters]` table: what every marked character frame is finished with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CharactersToml {
    /// The palette key of the one line round a marked frame, dressed or bare.
    outline: String,
}

/// One `[hairstyles."<name>@<N>x"]` table: a pair of layers per view.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HairstyleToml {
    front: Option<HairLayersToml>,
    back: Option<HairLayersToml>,
    side: Option<HairLayersToml>,
    crown: Option<HairLayersToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HairLayersToml {
    behind: Option<String>,
    over: Option<String>,
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

/// A loaded sprite pack: a named, versioned palette, its animations, the
/// hairstyles that dress them, and the city behind the windows.
#[derive(Debug, Clone)]
pub struct Pack {
    /// Pack name from the `[pack]` table in `pack.toml`.
    pub name: String,
    /// Pack version string from the `[pack]` table in `pack.toml`.
    pub version: String,
    palette: Arc<Palette>,
    animations: HashMap<String, Sprite>,
    buildings: BTreeMap<String, Building>,
    city_materials: Option<CityMaterials>,
    hairstyles: BTreeMap<String, Hairstyle>,
    character_outline: Option<Rgb>,
}

/// A material a `[buildings]` sprite is drawn in. A painter gives each its
/// colour from the sky, the theme and the building's depth, so one drawing
/// serves day and night alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Material {
    /// The lit face of a wall.
    Facade,
    /// The face turned from the light.
    Shade,
    /// A roof line or cornice.
    Roof,
    /// A window's glass: each connected run of it is one window, lit or not.
    Glass,
    /// The frame between panes.
    Mullion,
    /// Rooftop plant: a tank, a mast, a unit.
    Detail,
    /// A sign's panel, lit after dark.
    Sign,
}

impl Material {
    /// Its name as `[city]` keys it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Material::Facade => "facade",
            Material::Shade => "shade",
            Material::Roof => "roof",
            Material::Glass => "glass",
            Material::Mullion => "mullion",
            Material::Detail => "detail",
            Material::Sign => "sign",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Material::ALL.into_iter().find(|m| m.name() == name)
    }

    /// Its place in [`Material::ALL`]; a new material fails to compile here
    /// until it has one.
    fn index(self) -> usize {
        match self {
            Material::Facade => 0,
            Material::Shade => 1,
            Material::Roof => 2,
            Material::Glass => 3,
            Material::Mullion => 4,
            Material::Detail => 5,
            Material::Sign => 6,
        }
    }

    /// Every material, in declaration order.
    pub const ALL: [Material; 7] = [
        Material::Facade,
        Material::Shade,
        Material::Roof,
        Material::Glass,
        Material::Mullion,
        Material::Detail,
        Material::Sign,
    ];
}

/// The palette key each [`Material`] is drawn in, from `[city]`.
#[derive(Debug, Clone)]
pub struct CityMaterials([char; Material::ALL.len()]);

impl CityMaterials {
    /// The key `material` is drawn in.
    pub fn key(&self, material: Material) -> char {
        self.0[material.index()]
    }
}

/// A depth plane of the city a building may stand in: the far plane is the
/// painter's own silhouettes, so a pack draws only these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CityPlane {
    /// The middle distance.
    Mid,
    /// The nearest buildings, their feet below the sill.
    Near,
}

impl CityPlane {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "mid" => Some(CityPlane::Mid),
            "near" => Some(CityPlane::Near),
            _ => None,
        }
    }
}

/// One building of the city behind the office's windows, loaded from
/// `[buildings.<name>]` and its `[buildings."<name>@<N>x"]` variants: drawn
/// only in the `[city]` materials, and the planes it stands in.
#[derive(Debug, Clone)]
pub struct Building {
    name: String,
    planes: Vec<CityPlane>,
    base: BuildingArt,
    variants: BTreeMap<Density, BuildingArt>,
}

impl Building {
    /// The building's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether it may stand in `plane`.
    pub fn stands_in(&self, plane: CityPlane) -> bool {
        self.planes.contains(&plane)
    }

    /// Its size in logical units: its base art's.
    pub fn size(&self) -> (u16, u16) {
        self.base
            .sprite
            .frames()
            .first()
            .map_or((0, 0), |f| (f.width(), f.height()))
    }

    /// Its base art, at 1x.
    pub fn base(&self) -> &BuildingArt {
        &self.base
    }

    /// Its variant drawn at `density`, above its [`base`](Self::base).
    pub fn variant(&self, density: Density) -> Option<&BuildingArt> {
        self.variants.get(&density)
    }
}

/// A building drawn at one density: its one-frame sprite, and its windows.
#[derive(Debug, Clone)]
pub struct BuildingArt {
    sprite: Sprite,
    windows: Vec<Vec<(u16, u16)>>,
}

impl BuildingArt {
    /// The one-frame drawing, in the `[city]` materials.
    pub fn sprite(&self) -> &Sprite {
        &self.sprite
    }

    /// Its windows: each a connected run of [`Material::Glass`] pixels, which
    /// lights as one.
    pub fn windows(&self) -> &[Vec<(u16, u16)>] {
        &self.windows
    }
}

/// A hairstyle: per view, the layers that dress a character frame whose head
/// faces that way, laid mark on mark. Loaded from `[hairstyles."<name>@<N>x"]`,
/// so it only ever dresses art at its own density: the classic `1x` art is
/// never dressed.
#[derive(Debug, Clone)]
pub struct Hairstyle {
    name: String,
    density: Density,
    views: [Option<HairLayers>; HeadView::ALL.len()],
}

impl Hairstyle {
    /// The style's name, without its density.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The density of the art it dresses.
    pub fn density(&self) -> Density {
        self.density
    }

    /// Its layers for a head facing `view`.
    pub fn layers(&self, view: HeadView) -> Option<&HairLayers> {
        self.views[view.index()].as_ref()
    }
}

/// One view's layers: `behind` goes under the body, `over` on top. Each is a
/// one-frame sprite whose head mark lands on the body's.
#[derive(Debug, Clone)]
pub struct HairLayers {
    behind: Option<Sprite>,
    over: Option<Sprite>,
}

impl HairLayers {
    /// The layer under the body.
    pub fn behind(&self) -> Option<&Sprite> {
        self.behind.as_ref()
    }

    /// The layer over the body.
    pub fn over(&self) -> Option<&Sprite> {
        self.over.as_ref()
    }
}

impl Pack {
    /// The buildings of the city behind the windows, in name order.
    /// [`merge_from`](Self::merge_from) inherits them only with their
    /// [`city_materials`](Self::city_materials), into a pack with no city of
    /// its own.
    pub fn buildings(&self) -> impl Iterator<Item = &Building> {
        self.buildings.values()
    }

    /// The keys the buildings are drawn in, from `[city]`.
    pub fn city_materials(&self) -> Option<&CityMaterials> {
        self.city_materials.as_ref()
    }

    /// The pack's hairstyles, every density of each, in name order.
    /// [`merge_from`](Self::merge_from) never inherits one: a pack's characters
    /// are dressed only in its own.
    pub fn hairstyles(&self) -> impl Iterator<Item = &Hairstyle> {
        self.hairstyles.values()
    }

    /// The style `name` at `density`, where the pack draws one.
    pub fn hairstyle(&self, name: &str, density: Density) -> Option<&Hairstyle> {
        self.hairstyles.get(&format!("{name}@{density}x"))
    }

    /// The colour of the one line round every marked character frame at a
    /// density of 2 and up, dressed or bare, from `[characters]`.
    pub fn character_outline(&self) -> Option<Rgb> {
        self.character_outline
    }

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

    /// The name of the animation that draws `key`:
    /// [`animation_or_source`](Self::animation_or_source)'s pick. A painter that
    /// takes a piece's density variants looks them up under this name, since a
    /// derived piece the pack lacks draws its source's.
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

    /// The densest of [`Pack::density_variants`], or [`Density::ONE`] when the
    /// pack ships none.
    pub fn max_density_variant(&self) -> Density {
        self.density_variants()
            .first()
            .copied()
            .unwrap_or(Density::ONE)
    }

    /// The densities this pack's variants are drawn at, densest first, each
    /// once.
    ///
    /// A painter picks its render scale against these (the scene's
    /// `RenderScale::fit`), since a variant only lands at a scale its density
    /// divides. Only a variant of a registered animation that redraws its base
    /// ([`variant_redraws`]) counts: a stray key names nothing a painter asks
    /// for, and every renderer skips a variant that does not redraw its base.
    pub fn density_variants(&self) -> Vec<Density> {
        let densities: BTreeSet<Density> = self
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

    /// Merge [`OPTIONAL_FURNITURE_ANIMATIONS`] and [`OPTIONAL_CREATURE_ANIMATIONS`]
    /// — and their density variants —
    /// from `base` into self: the keys `RegisteredKey::is_inherited` passes;
    /// and `base`'s whole city, its buildings and `[city]`, when self has no
    /// buildings.
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
        // A city comes whole or not at all: its buildings are drawn in its own
        // `[city]` materials, which a pack with a city of its own renames.
        if self.buildings.is_empty() {
            self.buildings = base.buildings.clone();
            self.city_materials = base.city_materials.clone();
        }
    }

    /// The piece of this pack's own that `name` redraws, if it ships one. Art
    /// that redraws another piece only comes along with that piece: over this
    /// pack's own `desk`, the default's `desk@Nx` or `desk_north` would draw the
    /// default's desk wherever it is picked, so [`Pack::merge_from`] inherits
    /// nothing a piece of this pack's own answers for.
    fn own_redrawn_piece<'n>(&self, name: &'n str) -> Option<&'n str> {
        redrawn_pieces(name).find(|piece| self.animations.contains_key(*piece))
    }
}

/// Furniture drawn to match another piece (`desk_north` is `desk` with its
/// monitor raised, `meeting_sofa_north` is `meeting_sofa` from behind), as
/// `(derived, source)`.
const DERIVED_PIECES: &[(&str, &str)] = &[
    ("desk_north", "desk"),
    ("meeting_sofa_north", "meeting_sofa"),
];

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

/// The path-traversal guard MUST stay inside `load_pack`'s `get_src`:
/// [`load_pack_from_strings`] has no filesystem and no untrusted paths to escape.
fn build_pack(
    parsed: PackToml,
    // `dyn`, not generic, so this body compiles once for both loaders instead
    // of once per closure.
    get_src: &mut dyn FnMut(&str) -> Result<String>,
) -> Result<Pack> {
    let palette = Arc::new(build_palette(&parsed.palette, &parsed.ramps)?);
    let mut animations = HashMap::new();
    for (anim_name, anim) in parsed.animations {
        let mut frames = Vec::new();
        for fname in &anim.frames {
            let src = get_src(fname)?;
            let mut decoded = decode(fname, &src, &palette)?;
            frames.append(&mut decoded);
        }
        animations.insert(
            anim_name,
            Sprite::new(frames, Arc::clone(&palette), anim.frame_ms),
        );
    }

    let city_materials = parsed
        .city
        .map(|c| -> Result<CityMaterials> {
            if let Some(other) = c.keys().find(|k| Material::from_name(k).is_none()) {
                return Err(PackError::CityUnknownMaterial {
                    name: other.clone(),
                });
            }
            let mut keys = [' '; Material::ALL.len()];
            for material in Material::ALL {
                let Some(key) = c.get(material.name()) else {
                    return Err(PackError::CityMissingMaterial { material });
                };
                let what = KeySite::CityMaterial;
                let key = single_char(key, what)?;
                if !matches!(palette.get(key), Some(Some(_))) {
                    return Err(PackError::NotOpaque { what, key });
                }
                if keys[..material.index()].contains(&key) {
                    return Err(PackError::CityKeyShared { key });
                }
                keys[material.index()] = key;
            }
            Ok(CityMaterials(keys))
        })
        .transpose()?;
    let mut buildings: BTreeMap<String, Building> = BTreeMap::new();
    let (variants, bases): (Vec<_>, Vec<_>) = parsed
        .buildings
        .into_iter()
        .partition(|(key, _)| split_density_variant(key).is_some());
    for (key, building) in bases.into_iter().chain(variants) {
        let Some(materials) = city_materials.as_ref() else {
            return Err(PackError::BuildingWithoutCity { key });
        };
        let (name, density) = split_density_variant(&key).unwrap_or((&key, Density::ONE));
        if density == Density::ONE && key.contains(DENSITY_VARIANT_SEP) {
            return Err(PackError::BuildingDensity { key });
        }
        let art = building_art(&building.sprite, &palette, materials, get_src)?;
        match (density, building.planes) {
            (Density::ONE, Some(planes)) if !planes.is_empty() => {
                let planes = planes
                    .iter()
                    .map(|p| {
                        CityPlane::from_name(p).ok_or_else(|| PackError::BuildingPlane {
                            key: key.clone(),
                            plane: p.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                buildings.insert(
                    key.clone(),
                    Building {
                        name: key.clone(),
                        planes,
                        base: art,
                        variants: BTreeMap::new(),
                    },
                );
            }
            (Density::ONE, _) => return Err(PackError::BuildingWithoutPlanes { key }),
            (_, Some(_)) => return Err(PackError::VariantPlanes { key }),
            (_, None) => {
                let no_base = |key| PackError::VariantWithoutBase {
                    base: name.to_owned(),
                    key,
                };
                let Some(base) = buildings.get_mut(name) else {
                    return Err(no_base(key.clone()));
                };
                if !variant_redraws(&base.base.sprite, density, &art.sprite) {
                    let (base_w, base_h) = base.size();
                    return Err(PackError::VariantSize {
                        key,
                        density,
                        base_w,
                        base_h,
                    });
                }
                base.variants.insert(density, art);
            }
        }
    }

    let character_outline = parsed
        .characters
        .map(|c| -> Result<Rgb> {
            let what = KeySite::CharacterOutline;
            let key = single_char(&c.outline, what)?;
            match palette.get(key) {
                Some(Some(rgb)) => Ok(rgb),
                _ => Err(PackError::NotOpaque { what, key }),
            }
        })
        .transpose()?;
    let mut hairstyles = BTreeMap::new();
    for (key, style) in parsed.hairstyles {
        let Some((name, density)) = split_density_variant(&key) else {
            return Err(PackError::HairstyleDensity { key });
        };
        let mut layer = |view: HeadView, fname: &str| -> Result<Sprite> {
            let src = get_src(fname)?;
            let marked = decode(fname, &src, &palette)?;
            let [(_, marks)] = marked.as_slice() else {
                return Err(PackError::HairLayerFrames {
                    file: fname.to_owned(),
                });
            };
            if marks.iter().find_map(HeadMark::of).map(|h| h.view) != Some(view) {
                return Err(PackError::HairLayerHead {
                    file: fname.to_owned(),
                    view,
                });
            }
            Ok(Sprite::new(marked, Arc::clone(&palette), 0))
        };
        let mut views: [Option<HairLayers>; HeadView::ALL.len()] = Default::default();
        for (view, layers) in [
            (HeadView::Front, style.front),
            (HeadView::Back, style.back),
            (HeadView::Side, style.side),
            (HeadView::Crown, style.crown),
        ] {
            let Some(l) = layers else { continue };
            views[view.index()] = Some(HairLayers {
                behind: l.behind.as_deref().map(|f| layer(view, f)).transpose()?,
                over: l.over.as_deref().map(|f| layer(view, f)).transpose()?,
            });
        }
        hairstyles.insert(
            key.clone(),
            Hairstyle {
                name: name.to_owned(),
                density,
                views,
            },
        );
    }
    // A style changing with the density the renderer lands on would change an
    // agent's look with the window's size.
    let names_at = |d| -> BTreeSet<&str> {
        hairstyles
            .values()
            .filter(|s| s.density == d)
            .map(|s| s.name.as_str())
            .collect()
    };
    let densities: BTreeSet<_> = hairstyles.values().map(|s| s.density).collect();
    if let [first, rest @ ..] = densities.iter().copied().collect::<Vec<_>>().as_slice()
        && let Some(d) = rest.iter().find(|&&d| names_at(d) != names_at(*first))
    {
        return Err(PackError::HairstylesDiffer {
            first: *first,
            other: *d,
        });
    }

    Ok(Pack {
        name: parsed.pack.name,
        version: parsed.pack.version,
        palette,
        animations,
        buildings,
        city_materials,
        hairstyles,
        character_outline,
    })
}

/// A building's one-frame sprite `fname`, checked to draw only in the
/// `[city]` materials, with its windows found.
fn building_art(
    fname: &str,
    palette: &Arc<Palette>,
    materials: &CityMaterials,
    get_src: &mut dyn FnMut(&str) -> Result<String>,
) -> Result<BuildingArt> {
    let src = get_src(fname)?;
    let marked = decode(fname, &src, palette)?;
    let [(frame, _)] = marked.as_slice() else {
        return Err(PackError::BuildingFrames {
            file: fname.to_owned(),
        });
    };
    let pixels = palette.resolved();
    let index = |m: Material| palette.index_of(materials.key(m));
    let drawn: Vec<_> = Material::ALL.into_iter().filter_map(index).collect();
    let grid = &frame.0;
    for (i, &p) in grid.as_slice().iter().enumerate() {
        let p = usize::from(p);
        if pixels.get(p).copied().flatten().is_some() && !drawn.contains(&p) {
            let w = usize::from(grid.width());
            return Err(PackError::BuildingOutsideMaterials {
                file: fname.to_owned(),
                x: i % w,
                y: i / w,
            });
        }
    }
    let glass = index(Material::Glass).and_then(|i| PaletteIndex::try_from(i).ok());
    let windows = glass.map_or_else(Vec::new, |g| runs_of(grid, g));
    Ok(BuildingArt {
        sprite: Sprite::new(marked, Arc::clone(palette), 0),
        windows,
    })
}

/// Each 4-connected run of `index` in `grid`, as its pixels.
fn runs_of(grid: &Grid<PaletteIndex>, index: PaletteIndex) -> Vec<Vec<(u16, u16)>> {
    let (w, h) = (grid.width(), grid.height());
    let mut seen = vec![false; usize::from(w) * usize::from(h)];
    let mut runs = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let at = usize::from(y) * usize::from(w) + usize::from(x);
            if seen[at] || grid.get(x, y) != Some(&index) {
                continue;
            }
            seen[at] = true;
            let (mut run, mut stack) = (Vec::new(), vec![(x, y)]);
            while let Some((cx, cy)) = stack.pop() {
                run.push((cx, cy));
                let steps = [
                    (cx.checked_add(1), Some(cy)),
                    (cx.checked_sub(1), Some(cy)),
                    (Some(cx), cy.checked_add(1)),
                    (Some(cx), cy.checked_sub(1)),
                ];
                for (nx, ny) in steps {
                    let (Some(nx), Some(ny)) = (nx, ny) else {
                        continue;
                    };
                    if nx >= w || ny >= h {
                        continue;
                    }
                    let n = usize::from(ny) * usize::from(w) + usize::from(nx);
                    if !seen[n] && grid.get(nx, ny) == Some(&index) {
                        seen[n] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            run.sort_unstable();
            runs.push(run);
        }
    }
    runs
}

/// The file a pack directory's manifest is read from.
pub const PACK_MANIFEST: &str = "pack.toml";

/// Load a `Pack` from `dir`'s [`PACK_MANIFEST`] and its on-disk frame files,
/// guarding each frame path against directory traversal outside `dir`.
#[cfg(feature = "native")]
pub fn load_pack(dir: &Path) -> Result<Pack> {
    let toml_path = dir.join(PACK_MANIFEST);
    let toml_src = std::fs::read_to_string(&toml_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            PackError::NoManifest {
                dir: dir.to_owned(),
            }
        } else {
            PackError::Read {
                path: toml_path.clone(),
                source,
            }
        }
    })?;
    let parsed: PackToml = toml::from_str(&toml_src).map_err(|source| PackError::Manifest {
        path: Some(toml_path.clone()),
        source,
    })?;

    let canon_dir = dir
        .canonicalize()
        .map_err(|source| PackError::Canonicalize {
            path: dir.to_owned(),
            source,
        })?;

    build_pack(parsed, &mut |fname| {
        if Path::new(fname)
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(PackError::FramePathParent {
                file: fname.to_owned(),
            });
        }
        let path = dir.join(fname);
        let canon_path = path
            .canonicalize()
            .map_err(|source| PackError::Resolve { path, source })?;
        if !canon_path.starts_with(&canon_dir) {
            return Err(PackError::FramePathEscapes {
                file: fname.to_owned(),
            });
        }
        std::fs::read_to_string(&canon_path).map_err(|source| PackError::Read {
            path: canon_path,
            source,
        })
    })
}

/// `load_pack` from in-memory strings, so it reads no files.
pub fn load_pack_from_strings(pack_toml: &str, frames: &[(&str, &str)]) -> Result<Pack> {
    let parsed: PackToml =
        toml::from_str(pack_toml).map_err(|source| PackError::Manifest { path: None, source })?;
    let frame_lookup: HashMap<&str, &str> = frames.iter().copied().collect();

    build_pack(parsed, &mut |fname| {
        frame_lookup
            .get(fname)
            .map(|s| s.to_string())
            .ok_or_else(|| PackError::MissingEmbeddedFrame {
                file: fname.to_owned(),
            })
    })
}

/// Frame file `file`'s source, decoded against `palette`.
fn decode(file: &str, src: &str, palette: &Palette) -> Result<Vec<(IndexedFrame, Vec<Mark>)>> {
    parse_indexed(src, palette).map_err(|source| PackError::Decode {
        file: file.to_owned(),
        source,
    })
}

fn single_char(k: &str, what: KeySite) -> Result<char> {
    let mut it = k.chars();
    let (Some(key), None) = (it.next(), it.next()) else {
        return Err(PackError::NotOneChar {
            what,
            key: k.to_owned(),
        });
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
        return Err(PackError::PaletteTooLarge { keys });
    }
    let mut palette = Palette::new();
    for (k, v) in colors {
        let key = single_char(k, KeySite::Palette)?;
        let pixel = parse_palette_value(v).map_err(|source| PackError::Color {
            key: k.clone(),
            source,
        })?;
        palette.insert(key, pixel);
    }
    for (k, ramp) in ramps {
        let key = single_char(k, KeySite::Ramp)?;
        let of = single_char(&ramp.of, KeySite::RampBase)?;
        if colors.contains_key(k) {
            return Err(PackError::PaletteAndRamp { key });
        }
        // `colors`, not the palette being built, names the possible bases, so
        // an earlier-loaded ramp is never one.
        if !colors.contains_key(&ramp.of) || !matches!(palette.get(of), Some(Some(_))) {
            return Err(PackError::RampBase { key, of });
        }
        // Zero is a second name for the base itself.
        if ramp.level == 0 || ramp.level.unsigned_abs() > MAX_RAMP_LEVEL.unsigned_abs() {
            return Err(PackError::RampLevel {
                key,
                level: ramp.level,
            });
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

/// The grid art is authored on, in art pixels per logical unit: 1 for base
/// art, `N` for a `<name>@<N>x` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Density(NonZeroU16);

impl Density {
    /// The base art's density.
    pub const ONE: Self = Self(NonZeroU16::MIN);

    /// `n` as a density, or `None` for 0.
    pub const fn new(n: u16) -> Option<Self> {
        match NonZeroU16::new(n) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// The density as a number.
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl std::fmt::Display for Density {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The animation name for `base` drawn at `density`x.
pub fn density_variant_name(base: &str, density: Density) -> String {
    let mut out = String::with_capacity(base.len() + 4);
    density_variant_name_into(&mut out, base, density);
    out
}

/// [`density_variant_name`] into a caller-owned buffer, so a lookup loop reuses
/// one allocation.
pub fn density_variant_name_into(out: &mut String, base: &str, density: Density) {
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
pub(crate) fn split_density_variant(name: &str) -> Option<(&str, Density)> {
    let (base, density) = name.rsplit_once(DENSITY_VARIANT_SEP)?;
    let digits = density.strip_suffix('x')?;
    // Digits only, with no leading zero. `u16::from_str` accepts a leading `+`
    // or `0`, which would give one density two spellings — and this name is a
    // lookup KEY, so two spellings is two files a renderer picks between
    // arbitrarily, or one it never looks up.
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let density = Density::new(digits.parse().ok()?)
        .filter(|d| (2..=MAX_DENSITY_VARIANT).contains(&d.get()))?;
    Some((base, density))
}

/// Every registered animation: the required and optional character poses, then
/// the [`inherited_animation_names`].
fn registered_animation_names() -> impl Iterator<Item = &'static str> {
    REQUIRED_CHARACTER_ANIMATIONS
        .iter()
        .chain(OPTIONAL_CHARACTER_ANIMATIONS)
        .copied()
        .chain(inherited_animation_names())
}

/// The animations [`Pack::merge_from`] inherits: the optional furniture and
/// the optional creatures.
fn inherited_animation_names() -> impl Iterator<Item = &'static str> {
    OPTIONAL_FURNITURE_ANIMATIONS
        .iter()
        .chain(OPTIONAL_CREATURE_ANIMATIONS)
        .copied()
}

/// A pack key that names a registered animation: the animation itself, or a
/// density variant of it (`desk@4x`). Any registered animation takes variants,
/// since a character is redrawn at density like furniture; only furniture and
/// creatures are inherited ([`RegisteredKey::is_inherited`]).
///
/// Variants are legal BY DERIVATION rather than by their own registry rows. A
/// second list would have to be kept in step with the first, and forgetting an
/// entry fails QUIETLY in its least visible direction: the variant loads for
/// the bundled pack but [`Pack::merge_from`] never inherits it, so a
/// `--pack-dir` user silently drops back to the upscale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegisteredKey {
    /// The registered animation the key names or redraws.
    base: &'static str,
    /// The density a variant is drawn at; `None` for the animation itself.
    density: Option<Density>,
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

    /// Whether [`Pack::merge_from`] inherits this key: furniture, creatures and
    /// their variants only, because a robot pack must not fall back to human
    /// sprites.
    fn is_inherited(self) -> bool {
        inherited_animation_names().any(|n| n == self.base)
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
    "vending_machine",
    "printer",
    "meeting_table",
    "kitchen_island",
    "side_table",
    "water_cooler",
    "pantry_bin",
    "fish_tank",
    "coat_rack",
    "notice_board",
    "wall_clock",
    "meeting_chair",
];

/// The pets' and the gateway mascots' animation names a pack MAY provide, which
/// [`Pack::merge_from`] inherits like the furniture: they stand on their feet
/// wherever their frame ends, so they are kept apart from it.
pub const OPTIONAL_CREATURE_ANIMATIONS: &[&str] = &[
    "cat_walk",
    "cat_sit",
    "cat_sleep",
    "dog_walk",
    "dog_sit",
    "dog_sleep",
    "lobster_walk",
    "lobster_rest",
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
pub fn claimed_variant_size(base: &Frame, density: Density) -> (u32, u32) {
    (
        u32::from(base.width()) * u32::from(density.get()),
        u32::from(base.height()) * u32::from(density.get()),
    )
}

/// Whether `variant` is exactly the size its density claims over `base`
/// ([`claimed_variant_size`]).
pub fn variant_fits(base: &Frame, density: Density, variant: &Frame) -> bool {
    claimed_variant_size(base, density) == (u32::from(variant.width()), u32::from(variant.height()))
}

/// Whether `variant` redraws `base` at `density`: frame for frame, each frame
/// exactly the size its density claims over the matching base frame
/// ([`variant_fits`]). The one rule a renderer takes a variant by and
/// [`validate_pack_animations`] passes one by.
pub fn variant_redraws(base: &Sprite, density: Density, variant: &Sprite) -> bool {
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

/// A character frame a hairstyle would dress but for its missing head mark: it
/// is drawn bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmarkedHead {
    /// The animation, e.g. `standing@4x`.
    pub name: String,
    /// Its first unmarked frame, counted as [`DensityMismatch::frame`] is.
    pub frame: usize,
}

/// A view a hairstyle leaves out and a head at its density faces: that head is
/// drawn bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingHairView {
    /// The style's key, e.g. `mop@4x`.
    pub style: String,
    /// The view.
    pub view: HeadView,
    /// The first animation, by name, with such a head.
    pub name: String,
}

/// A hairstyle view with a layer reaching past a character frame's sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HairOverhang {
    /// The style's key, e.g. `mop@4x`.
    pub style: String,
    /// The view.
    pub view: HeadView,
    /// The first animation, by name, it overhangs.
    pub name: String,
    /// That animation's first such frame.
    pub frame: usize,
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
    /// One per character animation.
    pub unmarked_heads: Vec<UnmarkedHead>,
    /// One per hairstyle view.
    pub missing_hair_views: Vec<MissingHairView>,
    /// One per hairstyle view.
    pub overhanging_hair: Vec<HairOverhang>,
    /// Each hairstyle key at a density the pack draws no character at.
    pub orphan_hairstyles: Vec<String>,
}

impl ValidationReport {
    /// How many findings make the pack unusable: the fields this destructure counts.
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
            unmarked_heads: _,
            missing_hair_views: _,
            overhanging_hair: _,
            orphan_hairstyles,
        } = self;
        missing_required.len()
            + insufficient_frames.len()
            + mismatched_density.len()
            + orphan_variants.len()
            + mismatched_frame_counts.len()
            + orphan_hairstyles.len()
    }

    /// How many findings leave the pack usable but not as authored: the fields
    /// this destructure counts.
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
            unmarked_heads,
            missing_hair_views,
            overhanging_hair,
            orphan_hairstyles: _,
        } = self;
        missing_optional.len()
            + partial_sets.len()
            + orphan_derived.len()
            + unmarked_heads.len()
            + missing_hair_views.len()
            + overhanging_hair.len()
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
        .chain(inherited_animation_names().map(|name| {
            let stand_in = pack
                .own_redrawn_piece(name)
                .map_or(StandIn::DefaultPack, StandIn::OwnPiece);
            (name, stand_in)
        }))
        .filter(|&(name, _)| pack.animation(name).is_none() && !named_elsewhere(name))
        .map(|(name, stand_in)| MissingOptional { name, stand_in })
        .collect();
    report.missing_optional = missing_optional;

    let variants: Vec<(&str, &Sprite, &'static str, Density)> = pack
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
        if let Some(anim) = pack.animation(name)
            && anim.frames().len() < min_frames
        {
            report
                .insufficient_frames
                .push((name.to_string(), min_frames, anim.frames().len()));
        }
    };
    for name in registered_animation_names() {
        check_frames(name, name);
    }

    for &(name, variant, base, density) in &variants {
        let Some(base) = pack.animation(base).filter(|a| !a.frames().is_empty()) else {
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

    let mut characters: Vec<(&str, &Sprite, Density)> = variants
        .iter()
        .filter(|(_, _, base, _)| {
            REQUIRED_CHARACTER_ANIMATIONS.contains(base)
                || OPTIONAL_CHARACTER_ANIMATIONS.contains(base)
        })
        .map(|&(name, sprite, _, density)| (name, sprite, density))
        .collect();
    characters.sort_unstable_by_key(|&(name, ..)| name);
    let dressed_densities: BTreeSet<Density> = pack.hairstyles().map(Hairstyle::density).collect();
    for &(name, sprite, density) in &characters {
        if !dressed_densities.contains(&density) {
            continue;
        }
        if let Some(frame) = (0..sprite.frames().len()).find(|&i| sprite.head(i).is_none()) {
            report.unmarked_heads.push(UnmarkedHead {
                name: name.to_owned(),
                frame,
            });
        }
    }
    for style in pack.hairstyles() {
        let density = style.density();
        let key = density_variant_name(style.name(), density);
        let mut dressed = characters.iter().filter(|c| c.2 == density).peekable();
        if dressed.peek().is_none() {
            report.orphan_hairstyles.push(key);
            continue;
        }
        let (mut missing, mut overhung) = (Vec::new(), Vec::new());
        for &(name, sprite, _) in dressed {
            for (frame, body) in sprite.frames().iter().enumerate() {
                let Some(head) = sprite.head(frame) else {
                    continue;
                };
                let Some(layers) = style.layers(head.view) else {
                    if !missing.contains(&head.view) {
                        missing.push(head.view);
                        report.missing_hair_views.push(MissingHairView {
                            style: key.clone(),
                            view: head.view,
                            name: name.to_owned(),
                        });
                    }
                    continue;
                };
                let clipped = [layers.behind(), layers.over()]
                    .into_iter()
                    .flatten()
                    .any(|layer| overhangs(layer, body, head));
                if clipped && !overhung.contains(&head.view) {
                    overhung.push(head.view);
                    report.overhanging_hair.push(HairOverhang {
                        style: key.clone(),
                        view: head.view,
                        name: name.to_owned(),
                        frame,
                    });
                }
            }
        }
    }

    for name in pack.animation_names() {
        if RegisteredKey::parse(&name).is_none() {
            report.unknown.push(name);
        }
    }

    report
}

/// Whether `layer`, [laid on](Sprite::laid_on) `head`, puts an opaque pixel
/// past `body`'s sides: a dressed frame grows up, never wider. Not its bottom,
/// where the body's art ends at whatever hides the rest (`back_couch`'s seat back).
fn overhangs(layer: &Sprite, body: &Frame, head: HeadMark) -> bool {
    let Some((art, dx, _)) = layer.laid_on(head) else {
        return false;
    };
    (0..art.height()).any(|y| {
        (0..art.width()).any(|x| {
            let tx = i32::from(x) + dx;
            art.get(x, y).copied().flatten().is_some() && (tx < 0 || tx >= i32::from(body.width()))
        })
    })
}

#[cfg(test)]
mod validation_floor_tests {
    use super::*;

    fn d(n: u16) -> Density {
        Density::new(n).expect("nonzero")
    }

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
        assert_eq!(RegisteredKey::parse("desk@4x"), key("desk", Some(d(4))));
        assert_eq!(
            RegisteredKey::parse("standing@2x"),
            key("standing", Some(d(2)))
        );
        assert_eq!(
            RegisteredKey::parse("walking_coffee@8x"),
            key("walking_coffee", Some(d(8)))
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
        assert_eq!(pack.max_density_variant(), Density::ONE);
    }

    #[test]
    fn a_variant_name_carries_the_density_it_is_drawn_at() {
        // The name is the CLAIM a renderer looks up by, so it round-trips.
        assert_eq!(density_variant_name("desk", d(4)), "desk@4x");
        assert_eq!(split_density_variant("desk@4x"), Some(("desk", d(4))));
        assert_eq!(split_density_variant("desk@12x"), Some(("desk", d(12))));
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
        assert_eq!(pack.max_density_variant(), d(2));

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
            vec![d(4), d(2)],
            "3x does not redraw its base"
        );
        assert_eq!(pack.max_density_variant(), d(4));
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
        assert!(variant_redraws(anim("walking"), d(2), anim("walking@2x")));
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
            let verdict = variant_redraws(anim(base), d(2), anim(name));
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
        let report = validate(&format!(
            "[animations.desk]\n{ONE}[animations.meeting_sofa]\n{ONE}"
        ));
        let stand_in = |name: &str| {
            report
                .missing_optional
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{name} is reported missing"))
                .stand_in
        };
        assert_eq!(stand_in("desk_north"), StandIn::OwnPiece("desk"));
        assert_eq!(
            stand_in("meeting_sofa_north"),
            StandIn::OwnPiece("meeting_sofa")
        );
        assert_eq!(stand_in("plant"), StandIn::DefaultPack);
        assert_eq!(stand_in("cat_walk"), StandIn::DefaultPack);
        assert_eq!(stand_in("walking_coffee"), StandIn::OwnPose);

        let mut merged = pack_with(&format!(
            "[animations.desk]\n{ONE}[animations.meeting_sofa]\n{ONE}"
        ));
        merged.merge_from(&pack_with(&format!(
            "[animations.desk_north]\n{ONE}[animations.meeting_sofa_north]\n{ONE}\
             [animations.plant]\n{ONE}"
        )));
        assert!(
            merged.animation("desk_north").is_none()
                && merged.animation("meeting_sofa_north").is_none()
                && merged.animation("plant").is_some(),
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
            unmarked_heads: vec![UnmarkedHead {
                name: "standing@4x".to_string(),
                frame: 0,
            }],
            missing_hair_views: vec![MissingHairView {
                style: "mop@4x".to_string(),
                view: HeadView::Back,
                name: "walking_back@4x".to_string(),
            }],
            overhanging_hair: vec![HairOverhang {
                style: "mop@4x".to_string(),
                view: HeadView::Front,
                name: "standing@4x".to_string(),
                frame: 0,
            }],
            orphan_hairstyles: vec!["mop@2x".to_string()],
        };
        assert_eq!(report.error_count(), 6);
        assert_eq!(report.warning_count(), 6);
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
            Density::ONE,
            "a pack with no variants must not push a painter off the cell's own scale"
        );
        assert_eq!(
            pack("[animations.\"plant@4x\"]\nframes=[\"four.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            d(4)
        );
        assert_eq!(
            pack("[animations.\"typo@64x\"]\nframes=[\"one.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            d(2),
            "a variant of an unregistered base must not inflate the pack's density"
        );
        assert_eq!(
            pack("[animations.\"plant@8x\"]\nframes=[\"two.sprite\"]\nframe_ms=100\n")
                .max_density_variant(),
            d(2),
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

    /// A density is author input multiplied by a frame dimension: unbounded, a
    /// typo like `desk@60000x` overflows it.
    #[test]
    fn a_density_past_the_ceiling_is_not_a_variant_at_all() {
        assert_eq!(split_density_variant("desk@60000x"), None);
        assert_eq!(split_density_variant("desk@65535x"), None);
        // The boundary from both sides, so a future `<` typo cannot slip through.
        assert_eq!(
            split_density_variant("desk@64x"),
            Some(("desk", d(MAX_DENSITY_VARIANT)))
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

    /// A 1x `standing` and its 2x redraw `body`, dressed by `hairstyles`.
    fn dressed_pack(body: &str, hairstyles: &str, hair: &[(&str, &str)]) -> Pack {
        let mut frames = vec![("one.sprite", "@frame 0\nA"), ("body.sprite", body)];
        frames.extend_from_slice(hair);
        load_pack_from_strings(
            &format!(
                "[pack]\nname=\"t\"\nversion=\"1\"\n\
                 [palette]\n\"A\"=\"#010203\"\n\".\"=\"transparent\"\n\
                 [animations.standing]\nframes=[\"one.sprite\"]\nframe_ms=100\n\
                 [animations.\"standing@2x\"]\nframes=[\"body.sprite\"]\nframe_ms=100\n\
                 {hairstyles}"
            ),
            &frames,
        )
        .expect("pack builds")
    }

    const FRONT_BODY: &str = "@frame 0\n@mark head.front 0 0\nA A\nA A";
    const MOP: &str = "[hairstyles.\"mop@2x\"]\nfront={ over=\"o.sprite\" }\n";

    fn hair_findings(pack: &Pack) -> ValidationReport {
        let report = validate_pack_animations(pack, &[]);
        assert!(report.orphan_variants.is_empty() && report.mismatched_density.is_empty());
        report
    }

    /// `report`'s errors and warnings past those of the same pack undressed.
    fn hair_counts(report: &ValidationReport) -> (usize, usize) {
        let bare = validate_pack_animations(&dressed_pack(FRONT_BODY, "", &[]), &[]);
        (
            report.error_count() - bare.error_count(),
            report.warning_count() - bare.warning_count(),
        )
    }

    #[test]
    fn a_style_that_fits_every_head_it_dresses_is_clean() {
        let above_top = "@frame 0\n@mark head.front 0 1\nA\nA";
        let report = hair_findings(&dressed_pack(FRONT_BODY, MOP, &[("o.sprite", above_top)]));
        assert!(report.unmarked_heads.is_empty(), "{report:?}");
        assert!(report.missing_hair_views.is_empty(), "{report:?}");
        assert!(report.overhanging_hair.is_empty(), "{report:?}");
        assert!(report.orphan_hairstyles.is_empty(), "{report:?}");
    }

    #[test]
    fn a_frame_the_styles_would_dress_without_a_head_mark_is_a_warning() {
        let hair = ("o.sprite", "@frame 0\n@mark head.front 0 0\nA");
        let bald = "@frame 0\nA A\nA A";
        let report = hair_findings(&dressed_pack(bald, MOP, &[hair]));
        assert_eq!(
            report.unmarked_heads,
            vec![UnmarkedHead {
                name: "standing@2x".into(),
                frame: 0
            }]
        );
        assert_eq!(hair_counts(&report), (0, 1));

        let undressed = hair_findings(&dressed_pack(bald, "", &[]));
        assert!(undressed.unmarked_heads.is_empty(), "no style to dress it");
    }

    #[test]
    fn a_style_without_a_view_a_head_faces_is_a_warning() {
        let hair = ("o.sprite", "@frame 0\n@mark head.front 0 0\nA");
        let back = "@frame 0\n@mark head.back 0 0\nA A\nA A";
        let report = hair_findings(&dressed_pack(back, MOP, &[hair]));
        assert_eq!(
            report.missing_hair_views,
            vec![MissingHairView {
                style: "mop@2x".into(),
                view: HeadView::Back,
                name: "standing@2x".into(),
            }]
        );
        assert_eq!(hair_counts(&report), (0, 1));
    }

    #[test]
    fn hair_past_a_frames_side_is_a_warning() {
        for (hair, what) in [
            ("@frame 0\n@mark head.front 0 0\nA A A", "right"),
            ("@frame 0\n@mark head.front 1 0\nA A", "left"),
        ] {
            let report = hair_findings(&dressed_pack(FRONT_BODY, MOP, &[("o.sprite", hair)]));
            assert_eq!(
                report.overhanging_hair,
                vec![HairOverhang {
                    style: "mop@2x".into(),
                    view: HeadView::Front,
                    name: "standing@2x".into(),
                    frame: 0,
                }],
                "{what}"
            );
            assert_eq!(hair_counts(&report), (0, 1));
        }
        for (hair, what) in [
            (
                "@frame 0\n@mark head.front 0 0\nA . .",
                "transparent padding",
            ),
            ("@frame 0\n@mark head.front 0 0\nA\nA\nA", "past the bottom"),
        ] {
            let report = hair_findings(&dressed_pack(FRONT_BODY, MOP, &[("o.sprite", hair)]));
            assert!(report.overhanging_hair.is_empty(), "{what}");
        }
    }

    #[test]
    fn a_style_at_a_density_no_character_is_drawn_at_is_an_error() {
        let hair = ("o.sprite", "@frame 0\n@mark head.front 0 0\nA");
        let four = "[hairstyles.\"mop@4x\"]\nfront={ over=\"o.sprite\" }\n";
        let report = hair_findings(&dressed_pack(FRONT_BODY, four, &[hair]));
        assert_eq!(report.orphan_hairstyles, vec!["mop@4x".to_string()]);
        assert_eq!(hair_counts(&report), (1, 0));
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

fn parse_palette_value(v: &str) -> Result<Pixel, ColorError> {
    if v.eq_ignore_ascii_case("transparent") {
        return Ok(None);
    }
    let not_hex = || ColorError::Hex {
        value: v.to_owned(),
    };
    let hex = v.strip_prefix('#').ok_or_else(|| ColorError::Prefix {
        value: v.to_owned(),
    })?;
    // `u8::from_str_radix` accepts a leading '+', so `#+f0102` would slice to
    // `+f`/`01`/`02` and parse as a valid color without this explicit hex check.
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(not_hex());
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| not_hex());
    let (r, g, b) = (channel(0)?, channel(2)?, channel(4)?);
    Ok(Some(Rgb { r, g, b }))
}
