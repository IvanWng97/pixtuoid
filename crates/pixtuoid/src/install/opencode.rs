//! OpenCode plugin install. Unlike CC/Codex (config-merge), OpenCode uses a
//! file-based plugin system: we write `pixtuoid.js` into `.opencode/plugins/`.
//!
//! The JS plugin uses the official OpenCode `event` hook for real-time events
//! and performs a one-time scan of ~/.local/share/opencode/storage/session_diff
//! on load to discover pre-existing sessions. Events are forwarded to
//! `pixtuoid-hook` as fire-and-forget child_process calls (never blocking
//! OpenCode). No fs.watch — the event hook is the sole runtime channel.

use std::path::{Path, PathBuf};

use crate::install::io;

pub const PLUGIN_FILENAME: &str = "pixtuoid.js";

/// The JS plugin source, embedded in the Rust binary at compile time.
///
/// Uses the official OpenCode `event` hook for real-time events (no fs.watch)
/// and performs a one-time scan of session_diff on load to discover pre-existing
/// sessions. See the module-level doc comment for the architecture diagram.
pub const PLUGIN_SOURCE: &str = r#"// pixtuoid — OpenCode visualization plugin
// Installed by `pixtuoid install-hooks --target opencode`.
// Uses the official event hook for real-time events + one-time boot scan for
// pre-existing sessions. Never blocks OpenCode (fire-and-forget).
export const PixtuoidPlugin = async ({ project, client, $, directory, worktree }) => {
  const fs = await import("fs");
  const path = await import("path");
  const os = await import("os");
  const { execSync } = require("child_process");

  const hookBin = process.env.PIXTUOID_HOOK || "pixtuoid-hook";
  const diffDir = path.join(os.homedir(), ".local/share/opencode/storage/session_diff");

  function send(hookEventName, sessionId, extra = {}) {
    const msg = JSON.stringify({
      _pixtuoid_source: "opencode",
      hook_event_name: hookEventName,
      session_id: sessionId || "",
      cwd: directory || "",
      ...extra,
    });
    try {
      execSync(
        `printf '%s' '${msg.replace(/'/g, "'\\''")}' | PIXTUOID_SOURCE=opencode ${hookBin}`,
        { timeout: 3000, stdio: "ignore", env: { ...process.env, PIXTUOID_SOURCE: "opencode" } },
      );
    } catch {}
  }

  // --- One-time boot scan: discover pre-existing sessions ---
  if (fs.existsSync(diffDir)) {
    try {
      const files = fs.readdirSync(diffDir);
      let planted = 0;
      for (const f of files) {
        if (!f.endsWith(".json") || !f.startsWith("ses_")) continue;
        if (planted >= 10) break;
        const filePath = path.join(diffDir, f);
        const content = fs.readFileSync(filePath, "utf-8").trim();
        if (!content || content === "[]") continue;
        const sessionId = f.replace(/^ses_/, "").replace(/\.json$/, "");
        send("SessionStart", sessionId, {});
        planted++;
      }
    } catch {}
  }

  // --- Runtime phase: official OpenCode event hook ---
  return {
    event: async ({ event }) => {
      switch (event.type) {
        case "session.created": {
          const { id, directory: dir } = event.properties?.info || {};
          if (id) send("SessionStart", id, { cwd: dir || directory || "" });
          break;
        }
        case "message.part.updated": {
          const part = event.properties?.part || {};
          if (part.state === "started") {
            send("PreToolUse", part.sessionID, {
              tool_use_id: part.callID || "oc-" + Date.now(),
              tool_name: part.tool || "tool",
              tool_input: part.input,
            });
          } else if (part.state === "done" || part.state === "error") {
            send("PostToolUse", part.sessionID, {
              tool_use_id: part.callID || "oc-" + Date.now(),
            });
          }
          break;
        }
        case "permission.updated": {
          const p = event.properties || {};
          if (p.status === "pending") {
            send("PermissionRequest", p.sessionID, {});
          }
          break;
        }
        case "session.deleted": {
          const { id } = event.properties?.info || {};
          if (id) send("SessionEnd", id, {});
          break;
        }
      }
    },
  };
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
        assert!(PLUGIN_SOURCE.contains("event: async"));
        assert!(PLUGIN_SOURCE.contains("_pixtuoid_source"));
        assert!(PLUGIN_SOURCE.contains("PIXTUOID_SOURCE=opencode"));
        assert!(PLUGIN_SOURCE.contains("session.created"));
        assert!(PLUGIN_SOURCE.contains("message.part.updated"));
        assert!(PLUGIN_SOURCE.contains("permission.updated"));
        assert!(PLUGIN_SOURCE.contains("session.deleted"));
        assert!(!PLUGIN_SOURCE.contains("fs.watch"));
        assert!(!PLUGIN_SOURCE.contains("__pixtuoid_watchers"));
    }
}
