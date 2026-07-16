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
        let mut _buf = pixtuoid_core::sprite::RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        // The per-floor sim/paint stores, grouped (was six separate locals):
        // DrawCtx now borrows them as ONE `store` field.
        let mut _store = pixtuoid_scene::floor::FloorCtx::new();
        let mut _chitchat_state = std::collections::HashMap::new();

        // Defaults
        let mut _theme: &pixtuoid_scene::theme::Theme = &pixtuoid_scene::theme::NORMAL;
        let mut _floor = pixtuoid_scene::floor::FloorMeta::ground();
        let mut _floor_info: Option<pixtuoid::tui::renderer::FloorInfo> = None;

        // Apply overrides
        $(
            make_draw_ctx!(@override _theme, _floor, _floor_info, $key, $val);
        )*

        let mut $name = pixtuoid::tui::renderer::DrawCtx {
            buf: &mut _buf,
            store: &mut _store,
            mouse_pos: None,
            debug_walkable: false,
            theme: _theme,
            theme_picker: None,
            floor_info: _floor_info,
            per_floor: Default::default(),
            gateway: None,
            floor: _floor,
            active_pet: None,
            last_pet_pos: None,
            last_mascot_pos: None,
            floor_pet: None,
            chitchat_state: &mut _chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee: &std::collections::HashMap::new(),
            new_coffee_carriers: Vec::new(),
            occupied_waypoints: Default::default(),
            popup_scale: 0.0,
            help_open: false,
            source_warning: None,
            dashboard: &pixtuoid::tui::dashboard::DashboardFrame::default(),
            connection: &pixtuoid::tui::connection::ConnectionFrame::default(),
            onboarding: &pixtuoid::tui::welcome::OnboardingFrame::default(),
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
