/// Macro to create all the mutable locals and a `DrawCtx` for test rendering.
/// Expands to variable bindings in the caller's scope so borrows stay valid.
///
/// Usage:
///   `make_draw_ctx!(ctx);`                          — defaults (NORMAL theme, ground floor)
///   `make_draw_ctx!(ctx, theme: &CYBERPUNK);`       — custom theme
///   `make_draw_ctx!(ctx, floor_seed: 42);`          — custom floor seed
///   `make_draw_ctx!(ctx, floor_info: Some((1,2)));`  — custom floor info
///   `make_draw_ctx!(ctx, theme: t, floor_seed: 6);` — combine overrides
#[macro_export]
macro_rules! make_draw_ctx {
    ($name:ident $(, $key:ident : $val:expr)* ) => {
        let mut _buf = pixtuoid_core::sprite::RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb(0, 0, 0));
        let mut _cache = pixtuoid::tui::frame_cache::FrameCache::new();
        let mut _router = pixtuoid::tui::pathfind::AStarRouter::new();
        let mut _overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut _history = pixtuoid::tui::pose::PoseHistory::new();
        let mut _motion: std::collections::HashMap<pixtuoid_core::AgentId, pixtuoid::tui::motion::MotionState> = std::collections::HashMap::new();
        let mut _light = pixtuoid::tui::floor::LightingState::new();
        let _ticker = pixtuoid::tui::renderer::TickerQueue::new();
        let mut _chitchat_state = std::collections::HashMap::new();

        // Defaults
        let mut _theme: &pixtuoid::tui::theme::Theme = &pixtuoid::tui::theme::NORMAL;
        let mut _floor = pixtuoid::tui::floor::FloorMeta::ground();
        let mut _floor_info: Option<pixtuoid::tui::renderer::FloorInfo> = None;

        // Apply overrides
        $(
            make_draw_ctx!(@override _theme, _floor, _floor_info, $key, $val);
        )*

        let mut $name = pixtuoid::tui::renderer::DrawCtx {
            buf: &mut _buf,
            cache: &mut _cache,
            router: &mut _router,
            overlay: &mut _overlay,
            history: &mut _history,
            motion: &mut _motion,
            door_anim_max_ms: 0,
            light: &mut _light,
            mouse_pos: None,
            pinned_agent: None,
            debug_walkable: false,
            ticker: &_ticker,
            theme: _theme,
            theme_picker: None,
            floor_info: _floor_info,
            floor: _floor,
            active_pet: None,
            last_pet_pos: None,
            floor_pet_kind: None,
            chitchat_state: &mut _chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee_holders: &std::collections::HashSet::new(),
            coffee_fetched_at: &std::collections::HashMap::new(),
            new_coffee_carriers: Vec::new(),
            popup_scale: 0.0,
            help_open: false,
        };
    };

    (@override $theme:ident, $floor:ident, $floor_info:ident, theme, $val:expr) => {
        $theme = $val;
    };
    (@override $theme:ident, $floor:ident, $floor_info:ident, floor_seed, $val:expr) => {
        $floor.floor_seed = $val;
    };
    (@override $theme:ident, $floor:ident, $floor_info:ident, floor_info, $val:expr) => {
        $floor_info = $val;
    };
}
