//! The window's chrome over the office: its footer, tooltip and panels, and the panel geometry a press maps through.

use pixtuoid_core::state::SceneState;
use pixtuoid_scene::cutaway::{CellPx, Face, GridInk, paint_grid};
use pixtuoid_scene::display::cells::{CARD_SHADOW, CellGrid, CellRect};
use pixtuoid_scene::footer::FooterModel;
use pixtuoid_scene::pack::OfficeArt;
use pixtuoid_scene::render_scale::PixelFit;
use pixtuoid_scene::theme::Theme;

use super::compose::{XrgbSurface, pack_xrgb};
use super::geometry::{FOOTER_MARGIN_PX, footer_band};

/// Everything a presented frame shows beside the office: the window's size,
/// its footer, and the tooltip by the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Overlays {
    pub(crate) window: (u32, u32),
    pub(crate) footer: FooterModel,
    pub(crate) tooltip: Option<(pixtuoid_scene::tooltip::Tooltip, (i32, i32))>,
    /// The open panels, as the TUI paints them: [`panels_grid`].
    pub(crate) panels: Option<CellGrid>,
}

/// `frames`' open panels over a window `cols`×`rows` screen cells big, as
/// the TUI paints them over its terminal, read back as a grid; `None` when
/// none is open.
pub(crate) fn panels_grid(
    frames: &crate::panels::ui_state::RenderFrames,
    (cols, rows): (u16, u16),
    now: std::time::SystemTime,
    theme: &Theme,
) -> Option<CellGrid> {
    let overlays = frames.overlays();
    if !overlays.any_open() {
        return None;
    }
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(cols, rows)).ok()?;
    term.draw(|f| crate::panels::paint_overlays(f, &overlays, now, f.area(), theme))
        .ok()?;
    let buf = term.backend().buffer();
    Some(crate::panels::widgets::grid_of(buf, buf.area))
}

/// A window `window` physical px big, in screen cells of `cell`: the
/// surface the panels are laid out on.
pub(crate) fn panel_cells((w, h): (u32, u32), cell: CellPx) -> (u16, u16) {
    let fits = |px: u32, size: u16| u16::try_from(px / u32::from(size.max(1))).unwrap_or(u16::MAX);
    (fits(w, cell.w), fits(h, cell.h))
}

/// The screen cell of `cell` under window point `(x, y)`: the panels paint
/// from the window's top-left ([`paint_panels_into_surface`]).
pub(crate) fn cell_at((x, y): (f64, f64), cell: CellPx) -> (u16, u16) {
    let at = |p: f64, size: u16| {
        u16::try_from(p.max(0.0) as u32 / u32::from(size.max(1))).unwrap_or(u16::MAX)
    };
    (at(x, cell.w), at(y, cell.h))
}

/// What the open panels make of a left press at window point `cursor` on a
/// `window`-px window showing `shown`: [`crate::panels::modal_mouse`] in its
/// panel cells, the popup drawn whole.
pub(crate) fn modal_press(
    ui: &mut crate::panels::ui_state::UiState,
    cursor: (f64, f64),
    window: (u32, u32),
    shown: Option<PixelFit>,
) -> crate::panels::ModalMouse {
    let cells = shown.map(|at| {
        let cell = Face::chrome(at);
        (cell_at(cursor, cell), panel_cells(window, cell))
    });
    let popup_scale = crate::panels::ui_state::popup_whole(ui.modal().version_popup);
    crate::panels::modal_mouse(ui, popup_scale, cells.map(|(at, _)| at), || {
        cells.map(|(_, screen)| screen)
    })
}

/// Paint `tip` by the pointer at `cursor` (physical px): the shared
/// [`card`](pixtuoid_scene::tooltip::Tooltip::card) where
/// [`place`](pixtuoid_scene::tooltip::place) opens it, in screen cells of
/// `cell`, as the TUI draws it in terminal cells.
pub fn paint_tooltip_into_surface(
    sb: &mut XrgbSurface<'_>,
    tip: &pixtuoid_scene::tooltip::Tooltip,
    cursor: (f64, f64),
    (theme, pack): (&Theme, &OfficeArt),
    cell: CellPx,
) {
    let card = tip.card(theme);
    let cells =
        |px: usize, size: u16| u16::try_from(px / usize::from(size.max(1))).unwrap_or(u16::MAX);
    let area = CellRect {
        x: 0,
        y: 0,
        w: cells(sb.w, cell.w),
        h: cells(sb.h, cell.h),
    };
    let pointer = (
        cells(cursor.0.max(0.0) as usize, cell.w),
        cells(cursor.1.max(0.0) as usize, cell.h),
    );
    let placed = pixtuoid_scene::tooltip::place(card.rect(), pointer, area, tip.anchor);
    let at = (
        i32::from(placed.x) * i32::from(cell.w),
        i32::from(placed.y) * i32::from(cell.h),
    );
    let ink = GridInk {
        text: theme.ui.tooltip_text,
        halo: None,
        shadow: Some(CARD_SHADOW),
    };
    paint_grid(sb, &card, (at, cell), (Face::Screen, pack), ink);
}

/// The panel `name` (`help`, `dashboard`, `sources` or `theme`) open over
/// `scene` in a `window`-px window of screen cells `cell`, for a snapshot to
/// paint; `None` for a name it doesn't know.
#[doc(hidden)]
pub fn panel_preview(
    name: &str,
    scene: &SceneState,
    theme: &'static Theme,
    (window, cell): ((u32, u32), CellPx),
    now: std::time::SystemTime,
) -> Option<CellGrid> {
    let mut ui = crate::panels::ui_state::UiState::new(
        theme,
        crate::panels::welcome::WelcomeUi::from_detected(&[]),
        false,
        std::path::PathBuf::new(),
        None,
        crate::doctor::DriftSeen::default(),
    );
    match name {
        "help" => ui.toggle_help(),
        "dashboard" => ui.toggle_dashboard(scene),
        "sources" => ui.open_connection(crate::panels::connection::build_rows(
            &crate::runtime::ConnectedSources::default().snapshot(),
            &ui.read_conn_log(),
        )),
        "theme" => ui.open_theme_picker(),
        _ => return None,
    }
    panels_grid(
        &ui.build_frames(now, scene, &[]),
        panel_cells(window, cell),
        now,
        theme,
    )
}

/// Paint `panels` over the window from its top-left, in screen cells of
/// `cell`: the TUI's panels, cell for cell.
pub fn paint_panels_into_surface(
    sb: &mut XrgbSurface<'_>,
    panels: &CellGrid,
    (theme, pack): (&Theme, &OfficeArt),
    cell: CellPx,
) {
    let ink = GridInk {
        text: theme.ui.tooltip_text,
        halo: None,
        shadow: None,
    };
    paint_grid(sb, panels, ((0, 0), cell), (Face::Screen, pack), ink);
}

/// How many screen cells of `cell` fit across a `win_w`-pixel window
/// between the footer's margins: its column budget.
pub fn footer_budget(win_w: usize, cell: CellPx) -> u16 {
    let room = win_w.saturating_sub(2 * usize::from(FOOTER_MARGIN_PX));
    u16::try_from(room / usize::from(cell.w.max(1))).unwrap_or(u16::MAX)
}

/// Paint the shared status footer in `at`'s `footer_band` at the window's
/// foot: the theme's ground, the TUI footer row's terminal background, and
/// the line in `at`'s screen cells over it — the window's twin of the TUI's
/// status row, from the same [`build_footer`](pixtuoid_scene::footer::build_footer) model.
pub fn paint_footer_into_surface(
    sb: &mut XrgbSurface<'_>,
    model: &FooterModel,
    (theme, pack): (&Theme, &OfficeArt),
    at: PixelFit,
) {
    let cell = Face::chrome(at);
    let top = sb.h.saturating_sub(usize::from(footer_band(at)));
    let ground = pack_xrgb(theme.surface.bg_fallback);
    sb.px[top * sb.w..].fill(ground);
    let margin = i32::from(FOOTER_MARGIN_PX);
    let y = i32::try_from(top).unwrap_or(i32::MAX) + margin;
    let ink = GridInk {
        text: theme.ui.label_idle,
        halo: None,
        shadow: None,
    };
    paint_grid(
        sb,
        &model.line(theme),
        ((margin, y), cell),
        (Face::Screen, pack),
        ink,
    );
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{active_on, density, scene_with};
    use super::super::geometry::window_geometry;
    use super::*;
    use pixtuoid_scene::footer::{FooterInputs, build_footer};
    use winit::dpi::PhysicalSize;

    /// A tooltip paints its box in the theme's tooltip background, inside the
    /// window wherever the pointer is: a label near the top flips below it,
    /// and a card at the right edge shifts left.
    #[test]
    fn a_tooltip_stays_in_the_window_and_flips_off_the_edges() {
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let bg = pack_xrgb(theme.ui.tooltip_bg);
        let (w, h) = (320usize, 200usize);
        let pack = pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads");
        let painted = |tip: &pixtuoid_scene::tooltip::Tooltip, cursor: (f64, f64)| {
            let mut px = vec![0u32; w * h];
            let mut sb = XrgbSurface::new(&mut px, w, h).expect("sized");
            let look = (theme, &pack);
            paint_tooltip_into_surface(&mut sb, tip, cursor, look, Face::Screen.cell(1));
            let rows: Vec<usize> = (0..h)
                .filter(|&y| px[y * w..(y + 1) * w].contains(&bg))
                .collect();
            let cols: Vec<usize> = (0..w)
                .filter(|&x| (0..h).any(|y| px[y * w + x] == bg))
                .collect();
            (rows, cols)
        };
        let label = pixtuoid_scene::tooltip::coffee();
        let (rows, _) = painted(&label, (100.0, 4.0));
        assert!(!rows.is_empty(), "the label painted nothing");
        assert!(
            rows[0] > 4,
            "a label at the top flips below the pointer: {rows:?}"
        );
        let (rows, _) = painted(&label, (100.0, 150.0));
        assert!(
            *rows.last().expect("painted") < 150,
            "a label opens above the pointer"
        );
        let (_, cols) = painted(&label, (318.0, 100.0));
        assert!(
            *cols.last().expect("painted") < w,
            "the label shifted inside the right edge"
        );
        assert!(
            cols[0] < 318,
            "the label shifted left of a right-edge pointer"
        );
        // An agent's card opens below the pointer, flips above at the bottom
        // edge, and shifts left at the right one.
        let agent = active_on("/p/a.jsonl", 0, 0);
        let id = agent.agent_id;
        let scene = scene_with(vec![agent], 16);
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let card = pixtuoid_scene::tooltip::agent(&scene, id, now).expect("the agent's card");
        let (rows, _) = painted(&card, (100.0, 20.0));
        assert!(rows[0] > 20, "a card opens below the pointer: {rows:?}");
        let (rows, _) = painted(&card, (100.0, 195.0));
        assert!(
            *rows.last().expect("painted") < 195 && rows[0] > 0,
            "a card at the bottom flips above, inside the window: {rows:?}"
        );
        let (_, cols) = painted(&card, (318.0, 20.0));
        assert!(
            cols[0] < 318 && *cols.last().expect("painted") < w,
            "a card at the right edge shifts left, inside the window"
        );
    }

    /// The open panels read back as the TUI paints them, and nothing when
    /// none is open.
    #[test]
    fn the_window_shows_the_tuis_panels() {
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ui = crate::panels::ui_state::UiState::new(
            theme,
            crate::panels::welcome::WelcomeUi::from_detected(&[]),
            false,
            tmp.path().join("sock"),
            None,
            crate::doctor::DriftSeen::default(),
        );
        let now = std::time::SystemTime::UNIX_EPOCH;
        let size = (100, 40);
        let shut = ui.build_frames(now, &scene, &[]);
        assert_eq!(panels_grid(&shut, size, now, theme), None);
        ui.toggle_help();
        let help = ui.build_frames(now, &scene, &[]);
        let grid = panels_grid(&help, size, now, theme).expect("help is open");
        let text: String = (0..grid.height())
            .flat_map(|y| (0..grid.width()).map(move |x| (x, y)))
            .filter_map(|(x, y)| grid.get(x, y))
            .map(|c| c.symbol.as_str())
            .collect();
        assert!(text.contains("switch floor"), "{text}");
    }

    #[test]
    fn paint_footer_blits_into_the_bottom_band_and_tones_via_the_shared_authority() {
        use pixtuoid_scene::footer::{FooterTone, RungKind};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let slot = active_on("/p/a.jsonl", 0, 0);
        scene.agents.insert(slot.agent_id, slot);
        let inputs = FooterInputs::new(
            &scene,
            crate::panels::widgets::footer_context(&scene, None, true, None, None),
        );
        let (w, h) = (640usize, 400usize);
        let at = window_geometry(PhysicalSize::new(w as u32, h as u32), density());
        let model = build_footer(&inputs, footer_budget(w, Face::chrome(at)));
        let pack = pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads");
        // The office as the window draws it, then the footer over it.
        const OFFICE: u32 = 0x0012_3456;
        let mut sb = vec![OFFICE; w * h];
        paint_footer_into_surface(
            &mut XrgbSurface::new(&mut sb, w, h).expect("sized"),
            &model,
            (theme, &pack),
            at,
        );
        let top = h - usize::from(footer_band(at));
        assert!(sb[..top * w].iter().all(|&p| p == OFFICE), "above the band");
        assert!(
            sb[top * w..].iter().all(|&p| p != OFFICE),
            "no office pixel under the band"
        );
        assert!(
            sb[top * w..].contains(&pack_xrgb(theme.surface.bg_fallback)),
            "the band's ground"
        );
        assert!(
            sb.contains(&pack_xrgb(FooterTone::Rung(RungKind::Active).rgb(theme))),
            "the ●A rung paints the shared label_active hue"
        );
    }

    /// A window point reads as the panel cell painted under it.
    #[test]
    fn a_window_point_reads_as_its_panel_cell() {
        let cell = CellPx { w: 8, h: 16 };
        assert_eq!(panel_cells((80, 160), cell), (10, 10));
        assert_eq!(cell_at((0.0, 0.0), cell), (0, 0));
        assert_eq!(cell_at((15.9, 16.0), cell), (1, 1));
        assert_eq!(cell_at((-3.0, 40.0), cell), (0, 2));
    }

    /// A press reaches the office only with no panel open; the help closes
    /// on it, and the popup, drawn whole, swallows a press off its link.
    #[test]
    fn a_press_meets_the_open_panels_first() {
        use crate::panels::ModalMouse;
        let window = (960, 640);
        let shown = Some(window_geometry(
            PhysicalSize::new(window.0, window.1),
            density(),
        ));
        let boot = |popup: bool| {
            crate::panels::ui_state::UiState::new(
                pixtuoid_scene::theme::ALL_THEMES[0],
                crate::panels::welcome::WelcomeUi::from_detected(&[]),
                popup,
                std::path::PathBuf::new(),
                None,
                crate::doctor::DriftSeen::default(),
            )
        };
        let corner = (1.0, 1.0);
        let mut ui = boot(false);
        assert_eq!(
            modal_press(&mut ui, corner, window, shown),
            ModalMouse::Office
        );
        ui.toggle_help();
        assert_eq!(
            modal_press(&mut ui, corner, window, shown),
            ModalMouse::Took
        );
        assert!(!ui.help_open());
        ui.open_theme_picker();
        assert_eq!(
            modal_press(&mut ui, corner, window, shown),
            ModalMouse::Inert
        );
        let mut ui = boot(true);
        assert_eq!(
            modal_press(&mut ui, corner, window, shown),
            ModalMouse::Inert
        );
    }
}
