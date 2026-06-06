//! OpenCode plugin install. Unlike CC/Codex (config-merge), OpenCode uses a
//! file-based plugin system: we write `pixtuoid.js` into `.opencode/plugins/`.
//!
//! The JS plugin scans ~/.local/share/opencode/storage/session_diff on load
//! and uses fs.watch for real-time changes, forwarding structured events to
//! `pixtuoid-hook` as fire-and-forget child_process calls (never blocking
//! OpenCode). The event bus (return { event: ... }) is unused in practice.

use std::path::{Path, PathBuf};

use crate::install::io;

pub const PLUGIN_FILENAME: &str = "pixtuoid.js";

/// The JS plugin source, embedded in the Rust binary at compile time.
///
/// Strategy: OpenCode's event bus doesn't fire events to plugins in practice,
/// so we actively scan the session_diff directory on load and watch it with
/// fs.watch for real-time changes instead of relying on the event bus.
pub const PLUGIN_SOURCE: &str = r#"// pixtuoid — OpenCode visualization plugin
// Installed by `pixtuoid install-hooks --target opencode`.
// Scans session_diff on load, watches for changes, and forwards structured
// events to pixtuoid-hook. Never blocks OpenCode (fire-and-forget).
export const PixtuoidPlugin = async ({ project, client, $, directory, worktree }) => {
  const fs = await import("fs");
  const path = await import("path");
  const os = await import("os");
  const { execSync } = require("child_process");

  const hookBin = process.env.PIXTUOID_HOOK || "pixtuoid-hook";
  const diffDir = path.join(os.homedir(), ".local/share/opencode/storage/session_diff");

  function sendEvent(hookEventName, sessionId, extra) {
    const msg = JSON.stringify({
      _pixtuoid_source: "opencode",
      hook_event_name: hookEventName,
      session_id: sessionId || "",
      cwd: directory || "",
      ...extra,
    });
    // Fire-and-forget: errors silently caught, 3s timeout prevents blocking.
    try {
      execSync(
        `printf '%s' '${msg.replace(/'/g, "'\\''")}' | PIXTUOID_SOURCE=opencode ${hookBin}`,
        { timeout: 3000, stdio: "ignore", env: { ...process.env, PIXTUOID_SOURCE: "opencode" } },
      );
    } catch {}
  }

  // --- Initial scan of existing sessions ---
  if (fs.existsSync(diffDir)) {
    try {
      const files = fs.readdirSync(diffDir);
      let planted = 0;
      for (const f of files) {
        if (!f.endsWith(".json") || !f.startsWith("ses_")) continue;
        if (planted >= 10) break; // cap at 10 to avoid flood on plugin boot
        const filePath = path.join(diffDir, f);
        const content = fs.readFileSync(filePath, "utf-8").trim();
        if (!content || content === "[]") continue;
        const sessionId = f.replace(/^ses_/, "").replace(/\.json$/, "");
        sendEvent("SessionStart", sessionId, {});
        planted++;
      }
    } catch {}
  }

  // --- fs.watch for real-time changes ---
  let watcher = null;
  if (fs.existsSync(diffDir)) {
    try {
      watcher = fs.watch(diffDir, (eventType, filename) => {
        if (!filename || !filename.endsWith(".json") || !filename.startsWith("ses_")) return;
        const sessionId = filename.replace(/^ses_/, "").replace(/\.json$/, "");
        if (eventType === "rename") {
          sendEvent("SessionStart", sessionId, {});
        } else {
          // change = activity within the session
          sendEvent("PreToolUse", sessionId, {
            tool_use_id: "oc-" + sessionId + "-" + Date.now(),
            tool_name: "Edit:file",
          });
          sendEvent("PostToolUse", sessionId, {
            tool_use_id: "oc-" + sessionId + "-" + Date.now(),
          });
        }
      });
    } catch {}
  }

  // Keep watcher alive by attaching to globalThis (GC prevention).
  if (watcher) globalThis.__pixtuoid_watchers ||= [];
  if (watcher) globalThis.__pixtuoid_watchers.push(watcher);

  return {};
};
"#;

/// Default global plugin file path: ~/.config/opencode/plugins/pixtuoid.js
///
/// Used as the `Target::default_config_path` so the Target-compatible
/// `io::read_config` treats it as a regular file (readable text) and
/// `is_present` (file-exists check) works correctly.
pub fn default_plugin_dir() -> PathBuf {
    io::home_relative(".config/opencode/plugins/pixtuoid.js")
}

/// Project-local plugin directory: <project_root>/.opencode/plugins/
pub fn project_plugin_dir(project_root: &Path) -> PathBuf {
    project_root.join(".opencode").join("plugins")
}

/// Write the pixtuoid JS plugin to `plugin_path` (the full file path).
/// Returns `true` if the file was written (new/changed), `false` if it already
/// exists with identical content.
pub fn install_plugin(plugin_path: &Path) -> Result<bool, std::io::Error> {
    if plugin_path.exists() {
        let existing = std::fs::read_to_string(plugin_path).unwrap_or_default();
        if existing == PLUGIN_SOURCE {
            return Ok(false); // unchanged
        }
    }
    if let Some(parent) = plugin_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(plugin_path, PLUGIN_SOURCE)?;
    Ok(true) // written
}

/// Remove the pixtuoid JS plugin at `plugin_path`.
/// Returns `true` if the file was removed, `false` if it didn't exist.
pub fn uninstall_plugin(plugin_path: &Path) -> Result<bool, std::io::Error> {
    if plugin_path.exists() {
        std::fs::remove_file(plugin_path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Parent directory of the plugin file (used for project-level installs).
pub fn plugin_parent_dir(plugin_path: &Path) -> &Path {
    plugin_path.parent().unwrap_or(Path::new(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall_plugin() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugin_path = dir.path().join(PLUGIN_FILENAME);

        // First install writes the file.
        assert!(install_plugin(&plugin_path).unwrap());
        assert!(plugin_path.exists());
        let content = std::fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content, PLUGIN_SOURCE);

        // Second install is a no-op (content identical).
        assert!(!install_plugin(&plugin_path).unwrap());

        // Uninstall removes it.
        assert!(uninstall_plugin(&plugin_path).unwrap());
        assert!(!plugin_path.exists());

        // Second uninstall is a no-op.
        assert!(!uninstall_plugin(&plugin_path).unwrap());
    }

    #[test]
    fn default_plugin_dir_is_under_home() {
        let dir = default_plugin_dir();
        // The path points to the FILE (pixtuoid.js), not just the directory.
        let path_str = dir.to_string_lossy();
        assert!(path_str.contains(".config/opencode/plugins"));
        assert!(path_str.ends_with("pixtuoid.js"));
    }

    #[test]
    fn project_plugin_dir_is_under_dot_opencode() {
        let dir = project_plugin_dir(Path::new("/tmp/my-project"));
        assert_eq!(dir, PathBuf::from("/tmp/my-project/.opencode/plugins"));
    }

    #[test]
    fn plugin_source_is_valid_js_syntax() {
        // Basic sanity: the embedded JS contains valid exports.
        assert!(PLUGIN_SOURCE.contains("export const PixtuoidPlugin"));
        assert!(PLUGIN_SOURCE.contains("sendEvent"));
        assert!(PLUGIN_SOURCE.contains("_pixtuoid_source"));
        assert!(PLUGIN_SOURCE.contains("PIXTUOID_SOURCE=opencode"));
    }
}
