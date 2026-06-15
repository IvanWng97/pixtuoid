// @pixtuoid-openclaw-plugin
//
// Forwards OpenClaw gateway daemon-presence signals to pixtuoid's `pixtuoid-hook`
// shim, which relays them to the running pixtuoid office (the wandering "Molty"
// gateway mascot).
//
// PRIVACY (load-bearing): build the shim payload from an explicit ALLOWLIST of
// timing/id fields ONLY — NEVER message content, prompts, or file paths. The
// `allowConversationAccess` grant only un-gates `before_agent_run`/`agent_end`
// firing; it does NOT sanitize the payload — this allowlist is the sanitizer.
//
// NEVER BLOCK THE GATEWAY (pixtuoid invariant #5): `before_agent_run` is an
// AWAITED decision hook, so the shim is spawned DETACHED + unref'd and the
// handler returns immediately (fail-open: any error = allow, never derived from
// the spawn). Every handler is try/catch'd and returns undefined.

import { spawn } from "node:child_process";

const HOOK_PATH = {{HOOK_PATH_JSON}};

// The ONLY fields forwarded. `messages` / `prompt` / `sessionFile` / `systemPrompt`
// are deliberately ABSENT — the daemon fixture needs the run pairing key + ids,
// never content.
const ALLOW = ["runId", "sessionId", "sessionKey", "reason", "messageCount"];

function forward(type, ev, ctx) {
  try {
    const payload = { type };
    for (const k of ALLOW) {
      // Pull from ctx first (where ids live), else the event — but NEVER spread
      // the whole event (which carries messages/prompt).
      const v = ctx && ctx[k] !== undefined ? ctx[k] : ev && ev[k];
      if (v !== undefined) payload[k] = v;
    }
    // The gateway pid arms pixtuoid's instant abrupt-down (ExitWatch).
    if (type === "gateway_start") payload._pid = process.pid;

    const proc = spawn(HOOK_PATH, ["--source", "openclaw"], {
      stdio: ["pipe", "ignore", "ignore"],
      detached: true,
    });
    proc.on("error", () => {});
    proc.stdin.on("error", () => {});
    proc.stdin.write(JSON.stringify(payload) + "\n");
    proc.stdin.end();
    proc.unref(); // detached — the awaited hook never waits on it
  } catch (_) {
    // never throw — a thrown error in an awaited decision hook is fail-closed
  }
}

const HOOKS = [
  "gateway_start",
  "gateway_stop",
  "session_start",
  "session_end",
  "before_agent_run",
  "agent_end",
];

export default {
  id: "pixtuoid",
  name: "Pixtuoid",
  register(api) {
    for (const h of HOOKS) {
      try {
        api.on(h, (ev, ctx) => {
          forward(h, ev, ctx);
          // Return nothing → pass/allow. NEVER derived from the detached spawn.
          return undefined;
        });
      } catch (_) {
        /* unknown hook name on this OpenClaw version — skip, never throw */
      }
    }
  },
};
