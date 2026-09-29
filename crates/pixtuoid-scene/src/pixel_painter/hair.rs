//! Per-agent hairstyles: which of the pack's `[hairstyles]` an agent wears, and
//! a character frame dressed in it, its layers laid mark on mark over the
//! frame's `@head`. Only art a style's density draws is dressed, so the classic
//! `1x` art, which carries no mark, renders exactly as before.

use pixtuoid_core::id::splitmix64;
use pixtuoid_core::sprite::format::{Hairstyle, Pack};
use pixtuoid_core::sprite::{Frame, HeadMark, Pixel, Rgb, Sprite};
use pixtuoid_core::AgentId;

/// Keeps the hairstyle pick independent of every other per-agent draw seeded
/// from the same id (hair colour, outfit): without it, two agents that share a
/// hair colour would tend to share a style too.
const HAIRSTYLE_SALT: u64 = 0x6861_6972_7374_796c;

/// The style `agent` wears among the pack's styles at `density`: the one that
/// scores highest under rendezvous hashing, so adding a style to a pack moves
/// only the agents that now prefer it and leaves every other agent's look be.
pub(crate) fn pick(pack: &Pack, agent: AgentId, density: u16) -> Option<&Hairstyle> {
    let seed = splitmix64(splitmix64(agent.raw() ^ HAIRSTYLE_SALT));
    pack.hairstyles()
        .filter(|s| s.density() == density)
        .max_by_key(|s| splitmix64(seed ^ fnv1a(s.name())))
}

/// FNV-1a over a style's name: a key that stays put when the pack reorders or
/// grows its styles.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A layer with its own `@head`, and the first row it draws on.
fn marked(layer: &Sprite) -> Option<(&Frame, HeadMark)> {
    Some((layer.frames().first()?, layer.head(0)?))
}

/// The first row of `f` holding an opaque pixel.
fn opaque_top(f: &Frame) -> Option<u16> {
    (0..f.height()).find(|&y| (0..f.width()).any(|x| f.get(x, y).copied().flatten().is_some()))
}

/// How many art rows a frame dressed in `style` rises above the body's own
/// top: the hair that reaches over it, and the outline's row above that.
pub(crate) fn rise(head: HeadMark, style: &Hairstyle, outlined: bool) -> u16 {
    let Some(layers) = style.layers(head.view) else {
        return 0;
    };
    let top = [layers.behind(), layers.over()]
        .into_iter()
        .flatten()
        .filter_map(marked)
        .filter_map(|(f, mark)| {
            Some(i32::from(head.y) - i32::from(mark.y) + i32::from(opaque_top(f)?))
        })
        .min();
    top.map_or(0, |top| {
        u16::try_from((i32::from(outlined) - top).max(0)).unwrap_or(0)
    })
}

/// `body` dressed in `style`'s layers for its head's view: the behind layer,
/// the body, the over layer, then one `outline` round the union, so no line
/// runs between hair and face. The layers take the body's own recolor
/// `overrides`, so the hair wears the agent's colour. The frame grows [`rise`]
/// rows upward; its width is the body's.
pub(crate) fn dress(
    body: &Frame,
    head: HeadMark,
    style: &Hairstyle,
    overrides: &[(char, Pixel)],
    outline: Option<Rgb>,
) -> Frame {
    let Some(layers) = style.layers(head.view) else {
        return body.clone();
    };
    let up = rise(head, style, outline.is_some());
    let (w, h) = (body.width(), body.height().saturating_add(up));
    let mut px: Vec<Pixel> = vec![None; usize::from(w) * usize::from(h)];
    let mut lay = |f: &Frame, dx: i32, dy: i32| {
        for y in 0..f.height() {
            for x in 0..f.width() {
                let Some(c) = f.get(x, y).copied().flatten() else {
                    continue;
                };
                let (tx, ty) = (i32::from(x) + dx, i32::from(y) + dy + i32::from(up));
                if let (Ok(tx), Ok(ty)) = (u16::try_from(tx), u16::try_from(ty)) {
                    if tx < w && ty < h {
                        px[usize::from(ty) * usize::from(w) + usize::from(tx)] = Some(c);
                    }
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
    if let Some((f, dx, dy)) = dressed(layers.behind()) {
        lay(&f, dx, dy);
    }
    lay(body, 0, 0);
    if let Some((f, dx, dy)) = dressed(layers.over()) {
        lay(&f, dx, dy);
    }
    if let Some(line) = outline {
        let at = |x: i32, y: i32| {
            (0..i32::from(w)).contains(&x)
                && (0..i32::from(h)).contains(&y)
                && px[y as usize * usize::from(w) + x as usize].is_some()
        };
        let edge: Vec<usize> = (0..i32::from(h))
            .flat_map(|y| (0..i32::from(w)).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                !at(x, y)
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|(dx, dy)| at(x + dx, y + dy))
            })
            .map(|(x, y)| y as usize * usize::from(w) + x as usize)
            .collect();
        for i in edge {
            px[i] = Some(line);
        }
    }
    Frame::from_pixels(w, h, px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::sprite::format::load_pack_from_strings;
    use pixtuoid_core::sprite::HeadView;

    const H: Rgb = Rgb {
        r: 200,
        g: 100,
        b: 50,
    };
    const LINE: Rgb = Rgb { r: 1, g: 1, b: 1 };

    /// A pack with a one-pixel body and the styles `styles` at 2x, each a
    /// front-view over layer one pixel above the head.
    fn pack(styles: &[&str]) -> Pack {
        let mut toml = String::from(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\"H\"=\"#c86432\"\n\"S\"=\"#f0c0a0\"\n\
             \"k\"=\"#010101\"\n[hair]\noutline=\"k\"\n\
             [animations.seated]\nframes=[\"b.sprite\"]\nframe_ms=100\n",
        );
        for s in styles {
            toml.push_str(&format!(
                "[hairstyles.\"{s}@2x\"]\nfront={{ over=\"o.sprite\" }}\n"
            ));
        }
        load_pack_from_strings(
            &toml,
            &[
                ("b.sprite", "@frame 0\n. . .\n. . .\n. S .\n. . .\n"),
                ("o.sprite", "@frame 0\n@head front 0 1\nH\n. \n"),
            ],
        )
        .expect("the test pack loads")
    }

    #[test]
    fn a_pick_is_stable_and_adding_a_style_only_moves_who_prefers_it() {
        let few = pack(&["mop", "bun", "crop"]);
        let more = pack(&["mop", "bun", "crop", "curls"]);
        let mut moved = 0;
        for i in 0..200u64 {
            let id = AgentId::from_parts("claude-code", &i.to_string());
            let before = pick(&few, id, 2).expect("a style").name().to_owned();
            assert_eq!(
                pick(&few, id, 2).map(Hairstyle::name),
                Some(before.as_str()),
                "stable"
            );
            let after = pick(&more, id, 2).expect("a style").name();
            if after != before {
                assert_eq!(after, "curls", "an agent only ever moves to the new style");
                moved += 1;
            }
        }
        assert!(
            (20..=80).contains(&moved),
            "about a quarter move to the new style: {moved}"
        );
        assert!(
            pick(&few, AgentId::from_parts("x", "y"), 4).is_none(),
            "no style at 4x"
        );
    }

    #[test]
    fn a_dressed_frame_lays_the_hair_mark_on_mark_and_outlines_the_union() {
        let pack = pack(&["mop"]);
        let style = pack.hairstyles().next().expect("the style");
        let body = pack.animation("seated").expect("the body").frames()[0].clone();
        let head = HeadMark {
            view: HeadView::Front,
            x: 1,
            y: 0,
        };
        // The layer's pixel sits a row above its mark, so above the body's top:
        // the frame rises a row for the hair and one more for its outline.
        assert_eq!(rise(head, style, true), 2);
        let dressed = dress(&body, head, style, &[('H', Some(H))], Some(LINE));
        assert_eq!((dressed.width(), dressed.height()), (3, 6));
        let at = |x, y| dressed.get(x, y).copied().flatten();
        assert_eq!(
            at(1, 1),
            Some(H),
            "the hair, in the agent's colour, over the mark"
        );
        assert_eq!(at(1, 0), Some(LINE), "the outline above the hair");
        assert_eq!(at(0, 1), Some(LINE), "and beside it");
        assert_eq!(
            at(1, 4),
            Some(Rgb {
                r: 240,
                g: 192,
                b: 160
            }),
            "the body, moved down with it"
        );
        assert_eq!(
            at(1, 3),
            Some(LINE),
            "one outline round hair and body alike"
        );
    }
}
