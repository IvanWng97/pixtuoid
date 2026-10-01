#![allow(
    dead_code,
    reason = "every test crate includes `common` whole, and none uses every helper"
)]

/// Binds `$name` to a `DrawCtx::offscreen` of `$scene`; a macro so the stores it
/// borrows live in the caller's scope.
#[macro_export]
macro_rules! make_draw_ctx {
    ($name:ident, $scene:expr, $pack:expr, $now:expr) => {
        let mut _floor = pixtuoid_scene::floor::PerFloor::new();
        let mut _chitchat_state = std::collections::HashMap::new();
        let mut $name = pixtuoid::tui::renderer::DrawCtx::offscreen(
            &mut _floor,
            &mut _chitchat_state,
            &pixtuoid_scene::theme::NORMAL,
            $scene,
            $pack,
            $now,
            pixtuoid_scene::floor::FloorMeta::ground(),
        );
    };
}

pub(crate) fn fixture_scene(now: std::time::SystemTime) -> pixtuoid_core::SceneState {
    use pixtuoid_core::state::{ActivityState, ToolKind};
    use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
    use std::sync::Arc;

    let mut s = pixtuoid_core::SceneState::uniform(12);
    let age_offset = std::time::Duration::from_secs(60);
    let cases: &[(&str, ActivityState)] = &[
        (
            "agent-a",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write".into()),
                kind: ToolKind::Edit,
            },
        ),
        ("agent-b", ActivityState::Idle),
        (
            "agent-c",
            ActivityState::Waiting {
                reason: "perm?".into(),
            },
        ),
        ("agent-d", ActivityState::Idle),
    ];
    for (i, (key, state)) in cases.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        let created_at = now - age_offset;
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("session-{i}").as_str()),
                cwd: Arc::from(std::path::PathBuf::from("/demo").as_path()),
                label: (*key).into(),
                state: state.clone(),
                state_started_at: now,
                last_event_at: now,
                created_at,
                exiting_at: None,
                pending_idle_at: None,

                desk_index: GlobalDeskIndex(i),
                floor_idx: 0,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    s
}

pub(crate) fn render_hash(
    scene: &pixtuoid_core::SceneState,
    now: std::time::SystemTime,
    theme: &pixtuoid_scene::theme::Theme,
    floor: pixtuoid_scene::floor::FloorMeta,
) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 36)).unwrap();
    let pack = pixtuoid_scene::pack::load_bundled_pack().unwrap();
    make_draw_ctx!(draw_ctx, scene, &pack, now);
    draw_ctx.theme = theme;
    draw_ctx.world.floor = floor;
    pixtuoid::tui::renderer::draw_scene(&mut term, &mut draw_ctx).unwrap();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for px in draw_ctx.buf.as_slice() {
        px.r.hash(&mut hasher);
        px.g.hash(&mut hasher);
        px.b.hash(&mut hasher);
    }
    hasher.finish()
}

/// `pixtuoid args` with its env cleared to `home`, so nothing reads the
/// developer's real config or CLI dirs. PATH is replaced with a minimal one so
/// the spawn works, and `LLVM_PROFILE_FILE` survives: `just coverage` sets it so
/// an instrumented child writes its profile where the run collects it rather
/// than into the crate dir.
#[cfg(unix)]
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
