/// Binds `$name` to a `DrawCtx::headless` still of `$scene` on the ground floor
/// in the NORMAL theme; override a field by assigning it. A macro so the stores
/// the context borrows are bound in the caller's scope and outlive it.
#[macro_export]
macro_rules! make_draw_ctx {
    ($name:ident, $scene:expr) => {
        let mut _floor = pixtuoid_scene::floor::PerFloor::new();
        let mut _chitchat_state = std::collections::HashMap::new();
        let mut $name = pixtuoid::tui::renderer::DrawCtx::headless(
            &mut _floor,
            &mut _chitchat_state,
            &pixtuoid_scene::theme::NORMAL,
            pixtuoid_scene::floor::FloorMeta::ground(),
            $scene,
        );
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
