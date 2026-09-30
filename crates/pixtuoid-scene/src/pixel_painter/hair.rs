//! Per-agent hairstyles: which of the pack's `[hairstyles]` an agent wears, and
//! a character frame dressed in it, its layers laid mark on mark over the
//! frame's head. A frame is dressed only at a density of 2 and up
//! ([`dress_for`]), so the classic 1x art never is.

use std::num::NonZeroU16;

use pixtuoid_core::AgentId;
use pixtuoid_core::id::{fnv1a, splitmix64};
use pixtuoid_core::sprite::format::{Hairstyle, Pack};
use pixtuoid_core::sprite::{Frame, HeadMark, Pixel, Rgb, Sprite};

/// Separates the pick's seed from the other per-agent draws that finalize the
/// same id through `splitmix64` (`physics`, `pose::pure`), so the style is no
/// function of theirs.
const HAIRSTYLE_SALT: u64 = 0x6861_6972_7374_796c;

/// The name of the style `agent` wears: the one that scores highest under
/// rendezvous hashing, so adding a style to a pack moves only the agents that
/// now prefer it. It is picked by name, whatever the density, so an agent keeps
/// their style when the renderer lands on another.
fn pick(pack: &Pack, agent: AgentId) -> Option<&str> {
    let seed = splitmix64(splitmix64(agent.raw() ^ HAIRSTYLE_SALT));
    pack.hairstyles()
        .map(Hairstyle::name)
        .max_by_key(|name| splitmix64(seed ^ fnv1a(name.bytes().map(u64::from))))
}

/// How a character frame is dressed: its head, the style its agent wears at
/// its density where the pack draws that style there, and how many art rows
/// the dressed frame rises over the frame as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Dress {
    pub(crate) head: HeadMark,
    pub(crate) style: Option<String>,
    pub(crate) rise: u16,
}

/// The [`Dress`] of `body`, frame `head`'s art at `density`, for `agent`:
/// `None` at 1x, in a build without `density-art`, or on an unmarked frame,
/// which is drawn as it is.
pub(crate) fn dress_for(
    pack: &Pack,
    agent: AgentId,
    body: &Frame,
    head: Option<HeadMark>,
    density: NonZeroU16,
) -> Option<Dress> {
    let head = head.filter(|_| cfg!(feature = "density-art") && density.get() > 1)?;
    let style = pick(pack, agent).and_then(|name| pack.hairstyle(name, density));
    Some(Dress::of(
        body,
        head,
        style,
        pack.character_outline().is_some(),
    ))
}

impl Dress {
    /// `body`, its head `head`, dressed in `style` and outlined or not.
    pub(crate) fn of(
        body: &Frame,
        head: HeadMark,
        style: Option<&Hairstyle>,
        outlined: bool,
    ) -> Self {
        Dress {
            head,
            style: style.map(|s| s.name().to_owned()),
            rise: rise(body, head, style, outlined),
        }
    }
}

/// A layer's one frame and the head mark it is laid by.
fn marked(layer: &Sprite) -> Option<(&Frame, HeadMark)> {
    Some((layer.frames().first()?, layer.head(0)?))
}

/// The first row of `f` holding an opaque pixel.
fn opaque_top(f: &Frame) -> Option<u16> {
    (0..f.height()).find(|&y| (0..f.width()).any(|x| f.get(x, y).copied().flatten().is_some()))
}

/// How many art rows `body` dressed in `style` rises over its frame: the hair
/// that reaches above the frame's top, and the outline's row above that.
fn rise(body: &Frame, head: HeadMark, style: Option<&Hairstyle>, outlined: bool) -> u16 {
    let hair = style
        .and_then(|s| s.layers(head.view))
        .into_iter()
        .flat_map(|layers| {
            [layers.behind(), layers.over()]
                .into_iter()
                .flatten()
                .filter_map(marked)
                .filter_map(|(f, mark)| {
                    Some(i32::from(head.y) - i32::from(mark.y) + i32::from(opaque_top(f)?))
                })
        });
    let top = hair.chain(opaque_top(body).map(i32::from)).min();
    top.map_or(0, |top| {
        u16::try_from((i32::from(outlined) - top).max(0)).unwrap_or(0)
    })
}

/// `body` dressed as `dress` says: `style`'s behind layer for its head's view,
/// the body, the over layer, then one `outline` round the union, so no line runs
/// between hair and face. A bare head, with no style or none for its view, gets
/// the outline alone. The layers take the body's own recolor `overrides`, so the
/// hair wears the agent's colour. The frame grows [`Dress::rise`] rows upward;
/// its width is the body's.
pub(crate) fn dress(
    body: &Frame,
    dress: &Dress,
    style: Option<&Hairstyle>,
    overrides: &[(char, Pixel)],
    outline: Option<Rgb>,
) -> Frame {
    let (head, up) = (dress.head, dress.rise);
    let layers = style.and_then(|s| s.layers(head.view));
    let (w, h) = (body.width(), body.height().saturating_add(up));
    let mut px: Vec<Pixel> = vec![None; usize::from(w) * usize::from(h)];
    let mut lay = |f: &Frame, dx: i32, dy: i32| {
        for y in 0..f.height() {
            for x in 0..f.width() {
                let Some(c) = f.get(x, y).copied().flatten() else {
                    continue;
                };
                let (tx, ty) = (i32::from(x) + dx, i32::from(y) + dy + i32::from(up));
                if let (Ok(tx), Ok(ty)) = (u16::try_from(tx), u16::try_from(ty))
                    && tx < w
                    && ty < h
                {
                    px[usize::from(ty) * usize::from(w) + usize::from(tx)] = Some(c);
                }
            }
        }
    };
    let dressed = |layer: Option<&Sprite>| {
        let sprite = layer?;
        let mark = sprite.head(0)?;
        let frame = sprite.recolorable(0)?.recolored(overrides);
        Some((
            frame,
            i32::from(head.x) - i32::from(mark.x),
            i32::from(head.y) - i32::from(mark.y),
        ))
    };
    if let Some((f, dx, dy)) = dressed(layers.and_then(|l| l.behind())) {
        lay(&f, dx, dy);
    }
    lay(body, 0, 0);
    if let Some((f, dx, dy)) = dressed(layers.and_then(|l| l.over())) {
        lay(&f, dx, dy);
    }
    if let Some(line) = outline {
        let (w_i, h_i) = (i32::from(w), i32::from(h));
        let idx = |x: i32, y: i32| y as usize * usize::from(w) + x as usize;
        let opaque = |px: &[Pixel], x: i32, y: i32| {
            (0..w_i).contains(&x) && (0..h_i).contains(&y) && px[idx(x, y)].is_some()
        };
        let cells = || (0..h_i).flat_map(move |y| (0..w_i).map(move |x| (x, y)));
        let edge: Vec<usize> = cells()
            .filter(|&(x, y)| {
                !opaque(&px, x, y)
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|(dx, dy)| opaque(&px, x + dx, y + dy))
            })
            .map(|(x, y)| idx(x, y))
            .collect();
        for i in edge {
            px[i] = Some(line);
        }
        // A gap the line closes to one pixel takes the line too: a style laid
        // on a body it was not drawn against can leave one between chin and
        // hair, and no author can see it to close it.
        let holes: Vec<usize> = cells()
            .filter(|&(x, y)| {
                !opaque(&px, x, y)
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .all(|(dx, dy)| opaque(&px, x + dx, y + dy))
            })
            .map(|(x, y)| idx(x, y))
            .collect();
        for i in holes {
            px[i] = Some(line);
        }
    }
    Frame::from_pixels(w, h, px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::sprite::format::load_pack_from_strings;

    #[cfg(feature = "density-art")]
    const H: Rgb = Rgb {
        r: 200,
        g: 100,
        b: 50,
    };
    #[cfg(feature = "density-art")]
    const S: Rgb = Rgb {
        r: 240,
        g: 192,
        b: 160,
    };
    #[cfg(feature = "density-art")]
    const LINE: Rgb = Rgb { r: 1, g: 1, b: 1 };
    #[cfg(feature = "density-art")]
    const TWO: NonZeroU16 = NonZeroU16::MIN.saturating_add(1);

    /// A pack of `body` marked `head` (the `head.front` mark's column and row),
    /// outlined, and the styles `styles` at 2x, each a front-view over layer of
    /// one pixel a row above its mark.
    fn pack_of(body: &str, head: (u16, u16), styles: &[&str]) -> Pack {
        let mut toml = String::from(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\"H\"=\"#c86432\"\n\"S\"=\"#f0c0a0\"\n\
             \"k\"=\"#010101\"\n[characters]\noutline=\"k\"\n\
             [animations.seated]\nframes=[\"b.sprite\"]\nframe_ms=100\n",
        );
        for s in styles {
            toml.push_str(&format!(
                "[hairstyles.\"{s}@2x\"]\nfront={{ over=\"o.sprite\" }}\n"
            ));
        }
        let body = format!("@frame 0\n@mark head.front {} {}\n{body}", head.0, head.1);
        load_pack_from_strings(
            &toml,
            &[
                ("b.sprite", body.as_str()),
                ("o.sprite", "@frame 0\n@mark head.front 0 1\nH\n. \n"),
            ],
        )
        .expect("the test pack loads")
    }

    /// The one-skin-pixel body the tests dress.
    const BODY: &str = ". . .\n. . .\n. S .\n. . .\n";

    /// `pack`'s body dressed as it would be for `agent` at 2x.
    #[cfg(feature = "density-art")]
    fn dressed(pack: &Pack, agent: AgentId) -> (Dress, Frame) {
        let sprite = pack.animation("seated").expect("the body");
        let body = &sprite.frames()[0];
        let dress = dress_for(pack, agent, body, sprite.head(0), TWO).expect("a marked 2x frame");
        let style = dress.style.as_deref().and_then(|n| pack.hairstyle(n, TWO));
        let f = super::dress(
            body,
            &dress,
            style,
            &[('H', Some(H))],
            pack.character_outline(),
        );
        (dress, f)
    }

    #[test]
    fn a_pick_is_stable_and_adding_a_style_only_moves_who_prefers_it() {
        let few = pack_of(BODY, (1, 0), &["mop", "bun", "crop"]);
        let more = pack_of(BODY, (1, 0), &["mop", "bun", "crop", "curls"]);
        let mut moved = 0;
        for i in 0..200u64 {
            let id = AgentId::from_parts("claude-code", &i.to_string());
            let before = pick(&few, id).expect("a style");
            assert_eq!(pick(&few, id), Some(before), "stable");
            let after = pick(&more, id).expect("a style");
            if after != before {
                assert_eq!(after, "curls", "an agent only ever moves to the new style");
                moved += 1;
            }
        }
        assert!(
            (20..=80).contains(&moved),
            "about a quarter move to the new style: {moved}"
        );
    }

    #[test]
    #[cfg(feature = "density-art")]
    fn a_frame_is_dressed_only_at_2x_and_up_and_where_it_marks_a_head() {
        let pack = pack_of(BODY, (1, 0), &["mop"]);
        let sprite = pack.animation("seated").expect("the body");
        let body = &sprite.frames()[0];
        let agent = AgentId::from_parts("x", "y");
        assert!(dress_for(&pack, agent, body, sprite.head(0), NonZeroU16::MIN).is_none());
        assert!(dress_for(&pack, agent, body, None, TWO).is_none());
        let four = NonZeroU16::MIN.saturating_add(3);
        let bare = dress_for(&pack, agent, body, sprite.head(0), four).expect("marked at 4x");
        assert_eq!(bare.style, None, "no style at 4x: the head is bare");
    }

    #[test]
    #[cfg(feature = "density-art")]
    fn a_dressed_frame_lays_the_hair_mark_on_mark_and_outlines_the_union() {
        let pack = pack_of(BODY, (1, 0), &["mop"]);
        let (dress, f) = dressed(&pack, AgentId::from_parts("x", "y"));
        // The layer's pixel sits a row above its mark, so above the body's top:
        // the frame rises a row for the hair and one more for its outline.
        assert_eq!(dress.rise, 2);
        assert_eq!((f.width(), f.height()), (3, 6));
        let at = |x, y| f.get(x, y).copied().flatten();
        assert_eq!(
            at(1, 1),
            Some(H),
            "the hair, in the agent's colour, over the mark"
        );
        assert_eq!(at(1, 0), Some(LINE), "the outline above the hair");
        assert_eq!(at(0, 1), Some(LINE), "and beside it");
        assert_eq!(at(1, 4), Some(S), "the body, moved down with it");
        assert_eq!(
            at(1, 3),
            Some(LINE),
            "one outline round hair and body alike"
        );
    }

    #[test]
    #[cfg(feature = "density-art")]
    fn a_bare_head_is_outlined_as_a_dressed_one_is() {
        let pack = pack_of(". S .\n. . .\n", (1, 0), &[]);
        let (dress, f) = dressed(&pack, AgentId::from_parts("x", "y"));
        assert_eq!(
            (dress.style, dress.rise),
            (None, 1),
            "a row for the line alone"
        );
        let at = |x, y| f.get(x, y).copied().flatten();
        assert_eq!(at(1, 1), Some(S));
        assert_eq!(
            (at(1, 0), at(0, 1), at(1, 2)),
            (Some(LINE), Some(LINE), Some(LINE))
        );
    }

    #[test]
    #[cfg(feature = "density-art")]
    fn a_gap_the_outline_closes_to_one_pixel_takes_the_line() {
        // Four pixels two apart round a centre none of them touches: the line
        // round each walls the centre in.
        let pack = pack_of(
            ". . S . .\n. . . . .\nS . . . S\n. . . . .\n. . S . .\n",
            (2, 4),
            &["mop"],
        );
        let (dress, f) = dressed(&pack, AgentId::from_parts("x", "y"));
        assert_eq!(
            f.get(2, 2 + dress.rise).copied().flatten(),
            Some(LINE),
            "the centre, walled in"
        );
    }

    /// Every bundled character frame, bare and dressed in every bundled style,
    /// keeps its one outline whole: nothing but the line on a side edge, where
    /// the line has no column left to run in, and no pinhole inside it. A
    /// dressed frame first exists here, at runtime, so no generator check can
    /// see it.
    #[test]
    #[cfg(feature = "density-art")]
    fn every_bundled_character_dressed_in_every_style_keeps_its_outline_whole() {
        let pack = crate::embedded_pack::test_default_pack();
        let line = pack.character_outline();
        assert!(line.is_some(), "the bundled pack outlines its characters");
        let (mut dressed, mut flaws) = (0, Vec::new());
        for name in pack.animation_names() {
            let sprite = pack.animation(&name).expect("a listed animation");
            for (i, body) in sprite.frames().iter().enumerate() {
                let Some(head) = sprite.head(i) else {
                    continue;
                };
                let styles = pack.hairstyles().map(Some).chain([None]);
                for style in styles {
                    let d = Dress::of(body, head, style, true);
                    let f = dress(body, &d, style, &[], line);
                    let (w, h) = (f.width(), f.height());
                    let at = |x: u16, y: u16| f.get(x, y).copied().flatten();
                    let mut flaw = |what: &str, x: u16, y: u16| {
                        flaws.push(format!(
                            "{name}[{i}] in {}: {what} at ({x}, {y})",
                            style.map_or("no style", Hairstyle::name)
                        ));
                    };
                    for y in 0..h {
                        for x in [0, w - 1] {
                            if at(x, y).is_some() && at(x, y) != line {
                                flaw("unoutlined on the edge", x, y);
                            }
                        }
                    }
                    for y in 1..h - 1 {
                        for x in 1..w - 1 {
                            let walled = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                                .iter()
                                .all(|&(nx, ny)| at(nx, ny).is_some());
                            if at(x, y).is_none() && walled {
                                flaw("a pinhole", x, y);
                            }
                        }
                    }
                    dressed += 1;
                }
            }
        }
        assert!(flaws.is_empty(), "{flaws:#?}");
        assert!(dressed > 0, "the bundled pack dresses its characters");
    }
}
