//! Integration coverage for the `init-pack` and `validate-pack` subcommand
//! cores (`pixtuoid::init_pack::init_pack` / `pixtuoid::validate::validate_pack`).
//! Both are plain synchronous filesystem code — no TTY, async, or socket — so we
//! drive them directly through the public crate API rather than spawning the
//! binary, which exercises more branches deterministically. Tests may unwrap.

use std::fs;
use std::path::Path;

use pixtuoid::init_pack::init_pack;
use pixtuoid::validate::validate_pack;
use tempfile::TempDir;

// --- init_pack ------------------------------------------------------------

#[test]
fn init_pack_writes_skeleton_into_fresh_dir() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("newpack");

    init_pack(&dest, false).unwrap();

    let pack_toml = dest.join("pack.toml");
    let placeholder = dest.join("placeholder.sprite");
    assert!(pack_toml.exists(), "pack.toml written");
    assert!(placeholder.exists(), "placeholder.sprite written");

    // Content matches the embedded skeleton assets.
    assert_eq!(
        fs::read_to_string(&pack_toml).unwrap(),
        include_str!("../sprites/skeleton/pack.toml")
    );
    assert_eq!(
        fs::read_to_string(&placeholder).unwrap(),
        include_str!("../sprites/skeleton/placeholder.sprite")
    );
}

#[test]
fn init_pack_into_populated_dir_without_force_errors() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("pack");
    // Populate it with a real extraction first.
    init_pack(&dest, false).unwrap();

    // A second call without --force must refuse rather than clobber.
    let err = init_pack(&dest, false).unwrap_err();
    assert!(
        err.to_string().contains("non-empty"),
        "expected a non-empty guard error, got: {err}"
    );
}

#[test]
fn init_pack_with_file_at_dest_errors_not_a_directory() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("a-file");
    fs::write(&dest, "i am a file, not a dir").unwrap();

    let err = init_pack(&dest, false).unwrap_err();
    assert!(
        err.to_string().contains("not a directory"),
        "expected a not-a-directory error, got: {err}"
    );
}

#[test]
fn init_pack_force_rewrites_populated_dir() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("pack");
    init_pack(&dest, false).unwrap();

    // Mutate the extracted file, then force re-extract restores the original.
    let pack_toml = dest.join("pack.toml");
    fs::write(&pack_toml, "# clobbered\n").unwrap();

    init_pack(&dest, true).unwrap();

    assert_eq!(
        fs::read_to_string(&pack_toml).unwrap(),
        include_str!("../sprites/skeleton/pack.toml"),
        "force=true overwrites the user-modified file with the embedded skeleton"
    );
}

// --- validate_pack --------------------------------------------------------

#[test]
fn validate_pack_skeleton_is_ok_and_warns_on_optionals() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("skeleton");
    init_pack(&dest, false).unwrap();

    // The skeleton defines exactly the required animations and no optionals, so
    // validation succeeds (Ok) while exercising the missing-optional WARN loop.
    validate_pack(&dest).unwrap();
}

/// Write a deliberately-broken pack: it DROPS the required `seated` animation,
/// gives `typing` only one frame (needs ≥2 → insufficient_frames), and declares
/// a bogus `[animations.foo]` (unknown). Exercises the ERROR / INFO loops + bail.
fn write_broken_pack(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    // Reuse the embedded skeleton sprite as the frame for every animation.
    fs::write(
        dir.join("placeholder.sprite"),
        include_str!("../sprites/skeleton/placeholder.sprite"),
    )
    .unwrap();

    // All required animations EXCEPT `seated`, with `typing` at a single frame,
    // plus an unknown `foo`. Palette mirrors the skeleton's keys.
    let pack_toml = r##"
[pack]
name = "broken"
version = "0.0.1"

[palette]
"." = "transparent"
B = "#4488cc"
H = "#443322"
S = "#e8c090"
P = "#334455"
W = "#ffffff"

[animations.typing]
frames = ["placeholder.sprite"]
frame_ms = 400

[animations.standing]
frames = ["placeholder.sprite"]
frame_ms = 500

[animations.walking]
frames = ["placeholder.sprite", "placeholder.sprite"]
frame_ms = 200

[animations.walking_back]
frames = ["placeholder.sprite", "placeholder.sprite"]
frame_ms = 200

[animations.seated_sleeping]
frames = ["placeholder.sprite"]
frame_ms = 500

[animations.seated_sleeping_alt]
frames = ["placeholder.sprite"]
frame_ms = 500

[animations.holding_coffee]
frames = ["placeholder.sprite"]
frame_ms = 500

[animations.back_couch]
frames = ["placeholder.sprite"]
frame_ms = 500

[animations.foo]
frames = ["placeholder.sprite"]
frame_ms = 500
"##;
    fs::write(dir.join("pack.toml"), pack_toml).unwrap();
}

#[test]
fn validate_pack_with_errors_bails() {
    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("broken");
    write_broken_pack(&dest);

    let err = validate_pack(&dest).unwrap_err();
    assert!(
        err.to_string().contains("validation failed"),
        "expected a validation-failed bail, got: {err}"
    );
}
