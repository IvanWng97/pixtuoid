use pixtuoid_core::sprite::blit::{
    blit_frame, blit_frame_outlined, draw_dotted_hline, draw_line, half_block_cells, HalfCell,
};
use pixtuoid_core::sprite::{Frame, Pixel, Rgb, RgbBuffer};

fn px(r: u8, g: u8, b: u8) -> Pixel {
    Some(Rgb { r, g, b })
}
fn t() -> Pixel {
    None
}

#[test]
fn blit_writes_opaque_pixels_and_skips_transparent() {
    let frame = Frame {
        width: 2,
        height: 2,
        pixels: vec![px(10, 0, 0), t(), t(), px(0, 0, 30)],
    };
    let mut buf = RgbBuffer::filled(
        4,
        4,
        Rgb {
            r: 99,
            g: 99,
            b: 99,
        },
    );
    blit_frame(&frame, 1, 1, &mut buf);

    assert_eq!(buf.get(1, 1), Rgb { r: 10, g: 0, b: 0 });
    assert_eq!(
        buf.get(2, 1),
        Rgb {
            r: 99,
            g: 99,
            b: 99
        }
    );
    assert_eq!(
        buf.get(1, 2),
        Rgb {
            r: 99,
            g: 99,
            b: 99
        }
    );
    assert_eq!(buf.get(2, 2), Rgb { r: 0, g: 0, b: 30 });
    assert_eq!(
        buf.get(0, 0),
        Rgb {
            r: 99,
            g: 99,
            b: 99
        }
    );
}

#[test]
fn blit_ignores_out_of_bounds() {
    let frame = Frame {
        width: 3,
        height: 3,
        pixels: vec![px(1, 1, 1); 9],
    };
    let mut buf = RgbBuffer::filled(2, 2, Rgb { r: 0, g: 0, b: 0 });
    blit_frame(&frame, 1, 1, &mut buf);
    assert_eq!(buf.get(1, 1), Rgb { r: 1, g: 1, b: 1 });
}

#[test]
fn half_block_cells_pairs_rows() {
    let buf = RgbBuffer {
        width: 2,
        height: 4,
        pixels: vec![
            Rgb { r: 1, g: 0, b: 0 },
            Rgb { r: 2, g: 0, b: 0 },
            Rgb { r: 3, g: 0, b: 0 },
            Rgb { r: 4, g: 0, b: 0 },
            Rgb { r: 5, g: 0, b: 0 },
            Rgb { r: 6, g: 0, b: 0 },
            Rgb { r: 7, g: 0, b: 0 },
            Rgb { r: 8, g: 0, b: 0 },
        ],
    };
    let cells = half_block_cells(&buf);
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0].len(), 2);
    assert_eq!(
        cells[0][0],
        HalfCell {
            fg: Rgb { r: 1, g: 0, b: 0 },
            bg: Rgb { r: 3, g: 0, b: 0 }
        }
    );
    assert_eq!(
        cells[0][1],
        HalfCell {
            fg: Rgb { r: 2, g: 0, b: 0 },
            bg: Rgb { r: 4, g: 0, b: 0 }
        }
    );
    assert_eq!(
        cells[1][0],
        HalfCell {
            fg: Rgb { r: 5, g: 0, b: 0 },
            bg: Rgb { r: 7, g: 0, b: 0 }
        }
    );
    assert_eq!(
        cells[1][1],
        HalfCell {
            fg: Rgb { r: 6, g: 0, b: 0 },
            bg: Rgb { r: 8, g: 0, b: 0 }
        }
    );
}

#[test]
fn outlined_blit_paints_halo_around_silhouette() {
    // A simple 3x3 sprite that's opaque only in the center pixel.
    // Outline should be painted at all 4 cardinal neighbors of the center.
    let frame = Frame {
        width: 3,
        height: 3,
        pixels: vec![t(), t(), t(), t(), px(200, 0, 0), t(), t(), t(), t()],
    };
    let mut buf = RgbBuffer::filled(5, 5, Rgb { r: 0, g: 0, b: 0 });
    blit_frame_outlined(
        &frame,
        1,
        1,
        &mut buf,
        Rgb {
            r: 50,
            g: 50,
            b: 50,
        },
    );

    // Center has the sprite pixel.
    assert_eq!(buf.get(2, 2), Rgb { r: 200, g: 0, b: 0 });
    // 4 cardinal neighbors got the outline color.
    assert_eq!(
        buf.get(1, 2),
        Rgb {
            r: 50,
            g: 50,
            b: 50
        }
    );
    assert_eq!(
        buf.get(3, 2),
        Rgb {
            r: 50,
            g: 50,
            b: 50
        }
    );
    assert_eq!(
        buf.get(2, 1),
        Rgb {
            r: 50,
            g: 50,
            b: 50
        }
    );
    assert_eq!(
        buf.get(2, 3),
        Rgb {
            r: 50,
            g: 50,
            b: 50
        }
    );
    // Diagonals unchanged.
    assert_eq!(buf.get(1, 1), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(buf.get(3, 3), Rgb { r: 0, g: 0, b: 0 });
}

#[test]
fn outlined_blit_does_not_outline_interior_opaque_pixels() {
    // Fully-opaque 2x2 sprite — no transparent pixels, so no internal outline.
    let frame = Frame {
        width: 2,
        height: 2,
        pixels: vec![px(100, 0, 0), px(100, 0, 0), px(100, 0, 0), px(100, 0, 0)],
    };
    let mut buf = RgbBuffer::filled(4, 4, Rgb { r: 0, g: 0, b: 0 });
    blit_frame_outlined(
        &frame,
        1,
        1,
        &mut buf,
        Rgb {
            r: 50,
            g: 50,
            b: 50,
        },
    );

    // Sprite blitted intact.
    assert_eq!(buf.get(1, 1), Rgb { r: 100, g: 0, b: 0 });
    assert_eq!(buf.get(2, 2), Rgb { r: 100, g: 0, b: 0 });
    // No outline pixels painted around the sprite — `blit_frame_outlined`
    // only paints outline at transparent positions within the frame bounds.
    assert_eq!(buf.get(0, 1), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(buf.get(3, 3), Rgb { r: 0, g: 0, b: 0 });
}

#[test]
fn line_horizontal() {
    let mut buf = RgbBuffer::filled(10, 5, Rgb { r: 0, g: 0, b: 0 });
    draw_line(&mut buf, 2, 2, 8, 2, Rgb { r: 255, g: 0, b: 0 });
    for x in 2..=8 {
        assert_eq!(buf.get(x, 2), Rgb { r: 255, g: 0, b: 0 });
    }
    assert_eq!(buf.get(1, 2), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(buf.get(2, 3), Rgb { r: 0, g: 0, b: 0 });
}

#[test]
fn line_diagonal() {
    let mut buf = RgbBuffer::filled(5, 5, Rgb { r: 0, g: 0, b: 0 });
    draw_line(
        &mut buf,
        0,
        0,
        4,
        4,
        Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    );
    for i in 0..5 {
        assert_eq!(
            buf.get(i, i),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }
}

#[test]
fn line_clips_out_of_bounds_endpoints() {
    let mut buf = RgbBuffer::filled(5, 5, Rgb { r: 0, g: 0, b: 0 });
    draw_line(
        &mut buf,
        -3,
        -3,
        7,
        7,
        Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    );
    for i in 0..5 {
        assert_eq!(
            buf.get(i, i),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }
}

#[test]
fn dotted_hline_alternates() {
    let mut buf = RgbBuffer::filled(12, 1, Rgb { r: 0, g: 0, b: 0 });
    draw_dotted_hline(&mut buf, 0, 0, 11, Rgb { r: 255, g: 0, b: 0 }, 2, 2);
    // dash dash gap gap dash dash gap gap ...
    assert_eq!(buf.get(0, 0), Rgb { r: 255, g: 0, b: 0 });
    assert_eq!(buf.get(1, 0), Rgb { r: 255, g: 0, b: 0 });
    assert_eq!(buf.get(2, 0), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(buf.get(3, 0), Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(buf.get(4, 0), Rgb { r: 255, g: 0, b: 0 });
    assert_eq!(buf.get(5, 0), Rgb { r: 255, g: 0, b: 0 });
}

#[test]
fn dotted_hline_breaks_when_dash_overruns_x1() {
    // dash=3, gap=1 → period 4. Span [0,9] is NOT a multiple of the period,
    // so the final dash (starting at x=8) wants to paint 8,9,10 but 10 > x1=9
    // fires the line-94 break. Painted set: [0,1,2, 4,5,6, 8,9].
    let mut buf = RgbBuffer::filled(12, 1, Rgb { r: 0, g: 0, b: 0 });
    let red = Rgb { r: 255, g: 0, b: 0 };
    draw_dotted_hline(&mut buf, 0, 0, 9, red, 3, 1);
    // x1 itself is painted (the dash reaches it before the break).
    assert_eq!(buf.get(9, 0), red, "x1 should be painted");
    // x1+1 is NOT painted — the break stopped the overrunning dash.
    assert_eq!(
        buf.get(10, 0),
        Rgb { r: 0, g: 0, b: 0 },
        "x1+1 must be unpainted"
    );
    // No panic; sanity-check the rest of the expected pattern.
    for x in [0, 1, 2, 4, 5, 6, 8] {
        assert_eq!(buf.get(x, 0), red, "x={x} should be painted");
    }
    assert_eq!(buf.get(3, 0), Rgb { r: 0, g: 0, b: 0 }, "x=3 is a gap");
    assert_eq!(buf.get(7, 0), Rgb { r: 0, g: 0, b: 0 }, "x=7 is a gap");
}

#[test]
fn half_block_cells_on_empty_buffers_returns_empty_grid() {
    let rgb = Rgb { r: 1, g: 2, b: 3 };
    // w == 0 arm of the degenerate guard.
    assert!(half_block_cells(&RgbBuffer::filled(0, 4, rgb)).is_empty());
    // h == 0 arm.
    assert!(half_block_cells(&RgbBuffer::filled(4, 0, rgb)).is_empty());
}

#[test]
fn half_block_cells_pads_odd_height_with_repeated_row() {
    let buf = RgbBuffer {
        width: 1,
        height: 3,
        pixels: vec![
            Rgb { r: 1, g: 0, b: 0 },
            Rgb { r: 2, g: 0, b: 0 },
            Rgb { r: 3, g: 0, b: 0 },
        ],
    };
    let cells = half_block_cells(&buf);
    assert_eq!(cells.len(), 2);
    assert_eq!(
        cells[0][0],
        HalfCell {
            fg: Rgb { r: 1, g: 0, b: 0 },
            bg: Rgb { r: 2, g: 0, b: 0 }
        }
    );
    assert_eq!(
        cells[1][0],
        HalfCell {
            fg: Rgb { r: 3, g: 0, b: 0 },
            bg: Rgb { r: 3, g: 0, b: 0 }
        }
    );
}
