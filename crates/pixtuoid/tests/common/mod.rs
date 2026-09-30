/// Expands to variable bindings in the caller's scope so the borrows `DrawCtx`
/// takes stay valid.
#[macro_export]
macro_rules! make_draw_ctx {
    ($name:ident, $scene:expr, $pack:expr, $now:expr $(, $key:ident : $val:expr)* ) => {
        let mut _buf = pixtuoid_core::sprite::RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        let mut _store = pixtuoid_scene::floor::FloorCtx::new();
        let mut _chitchat_state = std::collections::HashMap::new();

        let mut _theme: &pixtuoid_scene::theme::Theme = &pixtuoid_scene::theme::NORMAL;
        let mut _floor = pixtuoid_scene::floor::FloorMeta::ground();
        let mut _floor_info: Option<pixtuoid::tui::renderer::FloorInfo> = None;

        $(
            make_draw_ctx!(@override _theme, _floor, _floor_info, $key, $val);
        )*

        let mut $name = pixtuoid::tui::renderer::DrawCtx {
            world: pixtuoid_scene::floor::FloorInputs {
                scene: $scene,
                pack: $pack,
                now: $now,
                floor: _floor,
                pets: Default::default(),
            },
            buf: &mut _buf,
            store: &mut _store,
            mouse_pos: None,
            debug_walkable: false,
            theme: _theme,
            theme_picker: None,
            floor_info: _floor_info,
            per_floor: Default::default(),
            gateway: None,
            audio_audible: false,
            volume_flash: None,
            last_pet_pos: None,
            last_mascots: Vec::new(),
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

/// `pixtuoid args` with its env cleared to `home`, so nothing reads the
/// developer's real config or CLI dirs. PATH is replaced with a minimal one so
/// the spawn works, and `LLVM_PROFILE_FILE` survives: `just coverage` sets it so
/// an instrumented child writes its profile where the run collects it rather
/// than into the crate dir.
#[cfg(unix)]
#[allow(dead_code)] // every test crate includes `common` whole; not every one spawns
pub(crate) fn isolated(args: &[&str], home: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin");
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    cmd
}
