// @pixtuoid-openclaw-plugin
//
// Forwards OpenClaw gateway daemon-presence signals to pixtuoid's `pixtuoid-hook`
// shim, which relays them to the running pixtuoid office (the wandering lobster
// gateway mascot).
//
// PRIVACY (load-bearing): build the shim payload from an explicit ALLOWLIST of
// timing/id fields ONLY — NEVER message content, prompts, or file paths. The
// `allowConversationAccess` grant only un-gates `before_agent_run`/`agent_end`
// firing; it does NOT sanitize the payload — this allowlist is the sanitizer.
//
// NEVER BLOCK THE GATEWAY (pixtuoid invariant #5). `before_agent_run` is an
// AWAITED, FAIL-CLOSED decision gate on the user's own turn (upstream registers
// it `fail-closed`, and a throw or a >15s hang discards their prompt), so three
// rules hold here:
//   1. the shim is spawned DETACHED + unref'd and NOTHING is awaited;
//   2. every handler is try/catch'd and can never throw;
//   3. the decision hook returns an EXPLICIT `{ outcome: "pass" }`.
// Rule 3 is deliberate. `undefined` also passes today (upstream filters it out
// before its merge policy runs, and `void` is in the declared handler type), but
// that merge policy still CONTAINS a written `undefined → block` arm which only
// the filter keeps unreachable. An explicit pass is the shape the contract is
// designed around — "only pass and block outcomes are supported" — so it cannot
// be inverted by that one guard changing. Any EXTRA key is rejected as a
// malformed decision and fails closed: keep this object exactly one key.
//
// GATEWAY IDENTITY (`gatewayPort`): OpenClaw supports several isolated gateways
// per host, each on its own base port, so the port is the runtime identity
// pixtuoid keys a mascot on. `gateway_start` hands us the REAL bound port
// (`event.port`, mirrored on the gateway hook ctx) and that is the only
// authoritative source — a `--port` override stays a local inside OpenClaw's
// gateway CLI and reaches NEITHER `api.config.gateway.port` NOR
// `OPENCLAW_GATEWAY_PORT`. So the port is adopted from any hook that carries one
// and remembered; the registration-time resolution below (upstream's own
// env → config → default order) is only the fallback for a plugin hot reload,
// which re-runs `register` WITHOUT replaying `gateway_start`. Accepted residual:
// a hot reload inside a `--port`-overridden session falls back to the default, so
// pixtuoid renders a second short-lived mascot that its presence TTL sweeps away.
//
// Deliberately NO `import { resolveGatewayPort } from "openclaw/plugin-sdk/core"`:
// this file is dropped into OpenClaw's state dir, whose path chain contains no
// `node_modules/openclaw` for a globally installed CLI, so a bare-specifier
// import would fail module resolution and take the WHOLE plugin down (no
// lobster). The resolution order is mirrored instead — see DEFAULT_GATEWAY_PORT.

import { spawn } from "node:child_process";

const HOOK_PATH = "{{HOOK_PATH_JSON}}";

// The ONLY fields forwarded. `messages` / `prompt` / `sessionFile` / `systemPrompt`
// are deliberately ABSENT — the daemon fixture needs the run pairing key + ids,
// never content. `success` is the agent_end run pass/fail BOOLEAN (#317: false =
// the model backend broke → the lobster renders Degraded); the `error` STRING that
// rides alongside it is deliberately NOT forwarded (it can embed content).
const ALLOW = ["runId", "sessionId", "sessionKey", "reason", "messageCount", "success"];

// OpenClaw's own default gateway port (its `DEFAULT_GATEWAY_PORT`, config/paths).
// Un-importable from here (see the module note), so it is named ONCE and carried
// by pixtuoid's upstream-drift watch instead of copied inline.
const DEFAULT_GATEWAY_PORT = 18789;

// The gateway's resolved port — the mascot's identity. Seeded at registration
// from the same env → config → default order OpenClaw's own `resolveGatewayPort`
// uses, then UPGRADED in place the first time a hook hands us the real bound port.
let gatewayPort = DEFAULT_GATEWAY_PORT;

function validPort(n) {
  return Number.isInteger(n) && n > 0 && n <= 65535;
}

function resolvePort(config) {
  const fromEnv = Number.parseInt(process.env.OPENCLAW_GATEWAY_PORT ?? "", 10);
  if (validPort(fromEnv)) return fromEnv;
  const fromConfig = config && config.gateway && config.gateway.port;
  if (validPort(fromConfig)) return fromConfig;
  return DEFAULT_GATEWAY_PORT;
}

// Adopt the authoritative port whenever a hook carries one (`gateway_start`'s
// event + the gateway hook ctx). Within one process the bound port never changes,
// so this only ever corrects the registration-time fallback.
function notePort(ev, ctx) {
  const observed = [ev && ev.port, ctx && ctx.port];
  for (const p of observed) {
    if (validPort(p)) gatewayPort = p;
  }
}

function forward(type, ev, ctx) {
  try {
    notePort(ev, ctx);
    const payload = { type };
    for (const k of ALLOW) {
      // Pull from ctx first (where ids live), else the event — but NEVER spread
      // the whole event (which carries messages/prompt).
      const v = ctx && ctx[k] !== undefined ? ctx[k] : ev && ev[k];
      if (v !== undefined) payload[k] = v;
    }
    // WHICH gateway sent this. pixtuoid keys one mascot per port, so a host
    // running two gateways renders two independent lobsters instead of one
    // collapsed presence where either gateway's stop takes the other down.
    payload.gatewayPort = gatewayPort;
    // pixtuoid arms its instant abrupt-down (ExitWatch) on the gateway pid. Stamp
    // it on EVERY event (not just gateway_start) so a MID-ATTACH or reconnect —
    // where pixtuoid never observed gateway_start — can still adopt the live pid
    // (#318). The plugin runs IN the gateway process, so process.pid is the
    // gateway's pid for every hook.
    payload._pid = process.pid;

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

// The ONE awaited DECISION hook among them (see the never-block note above).
const DECISION_HOOK = "before_agent_run";

// A FRESH object per decision, never one shared module-level literal: the gateway
// receives this value and any key it (or a future merge policy) stamps onto it
// would otherwise persist into every LATER turn's decision — and an extra key is
// rejected as malformed, i.e. fail-closed forever after one mutation.
function pass() {
  return { outcome: "pass" };
}

export default {
  id: "pixtuoid",
  name: "Pixtuoid",
  register(api) {
    gatewayPort = resolvePort(api && api.config);
    for (const h of HOOKS) {
      try {
        api.on(h, (ev, ctx) => {
          forward(h, ev, ctx);
          // The decision hook passes EXPLICITLY; the observers are void hooks.
          // NEVER derived from the detached spawn.
          return h === DECISION_HOOK ? pass() : undefined;
        });
      } catch (_) {
        /* unknown hook name on this OpenClaw version — skip, never throw */
      }
    }
  },
};
