// Executable contract test for the bundled OpenClaw plugin
// (`crates/pixtuoid/src/install/openclaw_plugin.js`) — run by `just npm-check`.
//
// The Rust side can only grep the template as a STRING; this drives the RENDERED
// module the way OpenClaw's plugin loader does (default export → `register(api)` →
// `api.on(hook, handler)`) and asserts the three contracts that break a user's
// gateway or leak their data if they regress:
//
//   1. NEVER BLOCK (pixtuoid invariant #5) — `before_agent_run` is an awaited,
//      fail-closed decision gate: it must return EXACTLY `{ outcome: "pass" }`,
//      synchronously, and must not throw even when the shim is unspawnable.
//   2. PRIVACY — the forwarded payload is the allowlist + identity and NOTHING
//      else, even when the event carries prompts / messages / file paths.
//   3. IDENTITY — every envelope carries `gatewayPort`, and the port observed on
//      a hook (the real bound port) outranks the registration-time fallback.
//
// The shim is a REAL recorder executable, so the spawn path itself is exercised
// rather than stubbed — that path is what must never block the gateway.

import assert from "node:assert/strict";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const PLUGIN_SOURCE = join(REPO_ROOT, "crates", "pixtuoid", "src", "install", "openclaw_plugin.js");
// Must match `HOOK_PLACEHOLDER` in `crates/pixtuoid/src/install/openclaw.rs` —
// the QUOTED token, so the template stays valid JS before rendering.
const HOOK_PLACEHOLDER = '"{{HOOK_PATH_JSON}}"';
// The six hooks pixtuoid depends on; must match `OPENCLAW_EVENTS` (Rust side
// pins template ⇔ const already, this pins the REGISTERED set at runtime).
const EXPECTED_HOOKS = [
  "gateway_start",
  "gateway_stop",
  "session_start",
  "session_end",
  "before_agent_run",
  "agent_end",
];
// OpenClaw's own default gateway port — the plugin's last-resort fallback.
const DEFAULT_GATEWAY_PORT = 18789;

const POSIX = process.platform !== "win32";

/// Render the template into a temp dir and import it. `shim: "recorder"` bakes a
/// real executable that appends each payload to a file; `shim: "missing"` bakes a
/// path that does not exist (the unspawnable case).
async function renderPlugin(t, { shim = "recorder" } = {}) {
  const root = await mkdtemp(join(tmpdir(), "pixtuoid-openclaw-plugin-"));
  t.after(() => rm(root, { recursive: true, force: true }));

  const source = await readFile(PLUGIN_SOURCE, "utf8");
  assert.equal(
    source.split(HOOK_PLACEHOLDER).length,
    2,
    "the hook placeholder must occur EXACTLY once (the bake replaces one token)",
  );

  const outFile = join(root, "payloads.ndjson");
  // The recorder also logs its own argv, so the spawn CONTRACT (the `--source`
  // attribution flag the shim needs) is pinned at runtime and not just by the Rust
  // side's grep of the template text — which cannot see the argv ARRAY or its value.
  const argvFile = join(root, "argv.txt");
  const hookPath = join(root, shim === "missing" ? "absent-hook" : "recorder-hook");
  if (shim === "recorder") {
    await writeFile(
      hookPath,
      `#!/bin/sh\nprintf '%s\\n' "$*" >> ${JSON.stringify(argvFile)}\ncat >> ${JSON.stringify(outFile)}\n`,
    );
    await chmod(hookPath, 0o755);
  }

  const entry = join(root, "index.js");
  await writeFile(entry, source.replace(HOOK_PLACEHOLDER, JSON.stringify(hookPath)));
  // Cache-bust: each case needs the module's own `gatewayPort` state fresh.
  const module = await import(`${pathToFileURL(entry)}?v=${cacheBust()}`);
  return { argvFile, hookPath, module, outFile, plugin: module.default };
}

let bust = 0;
function cacheBust() {
  bust += 1;
  return bust;
}

/// `register` the plugin against a fake api, returning the handler map.
function register(plugin, { config = {} } = {}) {
  const handlers = new Map();
  plugin.register({
    config,
    on(name, handler) {
      handlers.set(name, handler);
    },
  });
  return handlers;
}

/// Poll for `n` recorded payloads — the shim is spawned DETACHED, so delivery is
/// asynchronous by design (that is exactly why the gateway is never blocked).
async function recorded(outFile, n) {
  const deadline = Date.now() + 5_000;
  for (;;) {
    const text = await readFile(outFile, "utf8").catch(() => "");
    const lines = text.split("\n").filter((l) => l.trim().length > 0);
    if (lines.length >= n) return lines.map((l) => JSON.parse(l));
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${n} payload(s); got ${lines.length}`);
    }
    await new Promise((r) => setTimeout(r, 25));
  }
}

test("registers exactly the six presence hooks", async (t) => {
  const { plugin } = await renderPlugin(t);
  assert.deepEqual([...register(plugin).keys()], EXPECTED_HOOKS);
});

test("the decision hook passes EXPLICITLY and the observers are void", async (t) => {
  const { plugin } = await renderPlugin(t);
  const handlers = register(plugin);
  // Exactly one key: an extra key is a malformed decision upstream and FAILS CLOSED.
  const decision = handlers.get("before_agent_run")({ runId: "r" }, {});
  assert.deepEqual(decision, { outcome: "pass" });
  assert.deepEqual(Object.keys(decision), ["outcome"]);
  // A FRESH object per decision, not one shared module-level literal. The gateway
  // receives this value, so under a shared literal any key a consumer stamps onto
  // it persists into EVERY later turn — and an extra key is exactly what fails
  // closed. Mutating turn 1's result and re-checking turn 2 is what distinguishes
  // the two implementations; the shape assertions above only ever see turn 1.
  decision.reason = "a consumer stamped this";
  const next = handlers.get("before_agent_run")({ runId: "r2" }, {});
  assert.deepEqual(Object.keys(next), ["outcome"], "each decision must be a fresh object");
  for (const hook of EXPECTED_HOOKS.filter((h) => h !== "before_agent_run")) {
    assert.equal(handlers.get(hook)({}, {}), undefined, `${hook} must be a void observer`);
  }
});

test("an unspawnable shim still passes the gate and never throws", async (t) => {
  // The worst realistic case: the baked shim path no longer exists (a moved
  // binary). Every handler must stay a synchronous, non-throwing no-op — a throw
  // here would discard the user's prompt.
  const { plugin } = await renderPlugin(t, { shim: "missing" });
  const handlers = register(plugin);
  assert.deepEqual(handlers.get("before_agent_run")({ runId: "r" }, {}), { outcome: "pass" });
  for (const hook of EXPECTED_HOOKS) {
    assert.doesNotThrow(() => handlers.get(hook)({ runId: "r" }, { sessionId: "s" }));
  }
});

test("forwards ONLY the allowlist plus identity, never content", { skip: !POSIX }, async (t) => {
  const { module, outFile, plugin } = await renderPlugin(t);
  assert.equal(typeof module.default.register, "function", "loader needs a callable register");
  const handlers = register(plugin, { config: { gateway: { port: 19789 } } });

  handlers.get("before_agent_run")(
    {
      messages: [{ content: "SECRET_MESSAGE" }],
      prompt: "SECRET_PROMPT",
      runId: "event-run",
      sessionFile: "/SECRET/PATH",
      systemPrompt: "SECRET_SYSTEM",
    },
    { runId: "context-run", sessionId: "context-session" },
  );
  handlers.get("agent_end")(
    {
      error: "SECRET_ERROR",
      messages: [{ content: "SECRET_MESSAGE" }],
      runId: "event-run",
      sessionId: "event-session",
      success: false,
    },
    {},
  );

  // Look the payloads up BY TYPE: the shims are detached, so their appends race —
  // arrival order is deliberately not part of the contract.
  const lines = await recorded(outFile, 2);
  const byType = new Map(lines.map((l) => [l.type, l]));
  const run = byType.get("before_agent_run");
  const end = byType.get("agent_end");
  assert.deepEqual(run, {
    type: "before_agent_run",
    // ctx wins over the event for ids.
    runId: "context-run",
    sessionId: "context-session",
    gatewayPort: 19789,
    _pid: process.pid,
  });
  assert.deepEqual(end, {
    type: "agent_end",
    runId: "event-run",
    sessionId: "event-session",
    success: false,
    // The error's PRESENCE rides along as a bare boolean — that is what separates a
    // provider outage from a user abort, both of which upstream reports as
    // `success: false`. The string itself stays behind (asserted below), which is
    // the whole point of forwarding a discriminator instead of the message.
    errored: true,
    gatewayPort: 19789,
    _pid: process.pid,
  });
  const serialized = JSON.stringify([run, end]);
  assert.ok(!serialized.includes("SECRET"), `no content may leak: ${serialized}`);
});

test("the port observed on a hook outranks the config fallback", { skip: !POSIX }, async (t) => {
  // `--port` reaches neither `api.config.gateway.port` nor the environment, so
  // `gateway_start`'s `event.port` is the ONLY authoritative source; once seen it
  // must own every later envelope too (a session event carries no port).
  const { outFile, plugin } = await renderPlugin(t);
  const handlers = register(plugin, { config: { gateway: { port: 18789 } } });
  handlers.get("gateway_start")({ port: 19790 }, { port: 19790 });
  handlers.get("session_start")({ sessionId: "s1" }, { sessionId: "s1" });
  const byType = new Map((await recorded(outFile, 2)).map((l) => [l.type, l]));
  assert.equal(byType.get("gateway_start").gatewayPort, 19790, "the bound port wins");
  assert.equal(
    byType.get("session_start").gatewayPort,
    19790,
    "and is remembered for port-less hooks",
  );
});

test("port resolution falls back env > config > default", { skip: !POSIX }, async (t) => {
  const original = process.env.OPENCLAW_GATEWAY_PORT;
  t.after(() => {
    if (original === undefined) delete process.env.OPENCLAW_GATEWAY_PORT;
    else process.env.OPENCLAW_GATEWAY_PORT = original;
  });

  // No env, no config → OpenClaw's documented default.
  delete process.env.OPENCLAW_GATEWAY_PORT;
  {
    const { outFile, plugin } = await renderPlugin(t);
    register(plugin).get("session_start")({ sessionId: "s" }, {});
    const [only] = await recorded(outFile, 1);
    assert.equal(only.gatewayPort, DEFAULT_GATEWAY_PORT);
  }
  // Config only.
  {
    const { outFile, plugin } = await renderPlugin(t);
    register(plugin, { config: { gateway: { port: 20001 } } }).get("session_start")(
      { sessionId: "s" },
      {},
    );
    const [only] = await recorded(outFile, 1);
    assert.equal(only.gatewayPort, 20001);
  }
  // Env wins over config (upstream's own precedence).
  process.env.OPENCLAW_GATEWAY_PORT = "20002";
  {
    const { outFile, plugin } = await renderPlugin(t);
    register(plugin, { config: { gateway: { port: 20001 } } }).get("session_start")(
      { sessionId: "s" },
      {},
    );
    const [only] = await recorded(outFile, 1);
    assert.equal(only.gatewayPort, 20002);
  }
  // A junk env value falls through to the config rather than stamping NaN.
  process.env.OPENCLAW_GATEWAY_PORT = "not-a-port";
  {
    const { outFile, plugin } = await renderPlugin(t);
    register(plugin, { config: { gateway: { port: 20003 } } }).get("session_start")(
      { sessionId: "s" },
      {},
    );
    const [only] = await recorded(outFile, 1);
    assert.equal(only.gatewayPort, 20003);
  }
});

// Verified against the SHIPPED bundle (openclaw 2026.7.1,
// `dist/paths-*.js::parseGatewayPortEnvValue` → `parseTcpPort`): a bare
// `Number.parseInt` agrees with upstream on plain digits and diverges on every
// other form — it stops at the first non-digit, so `127.0.0.1:18902` became port
// `127`. That port IS the mascot's identity, so the divergence keyed the lobster
// to a gateway nobody was running. `CONFIG_PORT` is distinct from every expected
// value so "fell through to config" can never be mistaken for a parse.
const CONFIG_PORT = 20100;
for (const [raw, want, why] of [
  ["18902", 18902, "bare digits"],
  ["  18902  ", 18902, "trimmed"],
  ["127.0.0.1:18902", 18902, "host:port — the parseInt regression (was 127)"],
  ["[::1]:18902", 18902, "bracketed IPv6"],
  ["18902abc", CONFIG_PORT, "trailing garbage is not a port (was 18902)"],
  ["a:b:18902", CONFIG_PORT, "two colons is not host:port"],
  ["70000", CONFIG_PORT, "above the TCP max"],
  ["0", CONFIG_PORT, "not positive"],
  ["", CONFIG_PORT, "empty"],
]) {
  test(`env port form: ${JSON.stringify(raw)} ⇒ ${want} (${why})`, { skip: !POSIX }, async (t) => {
    const original = process.env.OPENCLAW_GATEWAY_PORT;
    t.after(() => {
      if (original === undefined) delete process.env.OPENCLAW_GATEWAY_PORT;
      else process.env.OPENCLAW_GATEWAY_PORT = original;
    });
    process.env.OPENCLAW_GATEWAY_PORT = raw;
    const { outFile, plugin } = await renderPlugin(t);
    register(plugin, { config: { gateway: { port: CONFIG_PORT } } }).get("session_start")(
      { sessionId: "s" },
      {},
    );
    const [only] = await recorded(outFile, 1);
    assert.equal(only.gatewayPort, want);
  });
}

test("agent_end forwards the errored discriminator, never the error string", { skip: !POSIX }, async (t) => {
  // Upstream builds `success` as `!aborted && !promptError`, so success:false alone
  // cannot tell a user CANCELLING a turn from a provider outage — and Degraded is
  // sticky. Only a prompt error carries `error`, so its PRESENCE is the signal. The
  // string itself can embed prompt content and must never leave the gateway.
  const { outFile, plugin } = await renderPlugin(t);
  const handlers = register(plugin, { config: { gateway: { port: 18789 } } });
  handlers.get("agent_end")(
    { runId: "r1", sessionId: "s1", success: false, error: "Provider 500: upstream down" },
    {},
  );
  handlers.get("agent_end")({ runId: "r2", sessionId: "s1", success: false }, {});
  handlers.get("agent_end")({ runId: "r3", sessionId: "s1", success: true }, {});
  const rows = await recorded(outFile, 3);
  const byRun = new Map(rows.map((r) => [r.runId, r]));

  assert.equal(byRun.get("r1").errored, true, "a prompt error is a real failure");
  assert.equal(byRun.get("r2").errored, false, "an abort carries no error → not degraded");
  assert.equal(
    byRun.get("r3").errored,
    undefined,
    "success:true needs no discriminator at all",
  );
  for (const row of rows) {
    assert.equal(row.error, undefined, "the error STRING must never be forwarded");
    assert.ok(
      !JSON.stringify(row).includes("upstream down"),
      `no error text may leak: ${JSON.stringify(row)}`,
    );
  }
});

test("an out-of-range observed port is refused, keeping the resolved one", { skip: !POSIX }, async (t) => {
  // Defence in depth at the producer: pixtuoid's decoder REJECTS an envelope whose
  // port is unusable, so stamping a bogus value would silently drop the mascot.
  const { outFile, plugin } = await renderPlugin(t);
  const handlers = register(plugin, { config: { gateway: { port: 18999 } } });
  for (const bad of [0, -1, 65_536, 1.5, "19001", null]) {
    handlers.get("gateway_start")({ port: bad }, {});
  }
  const lines = await recorded(outFile, 6);
  for (const line of lines) {
    assert.equal(line.gatewayPort, 18999, `bogus port ${line.gatewayPort} was adopted`);
  }
});

test("the shim is spawned with the source-attribution flag", { skip: !POSIX }, async (t) => {
  // `--source openclaw` is how the shim stamps `_pixtuoid_source` — its ONLY
  // attribution channel, and the reason a payload is demuxed to the daemon lane at
  // all. The Rust side can only grep the template for the literal, so the VALUE is
  // pinned here, against the argv the spawned process actually received.
  const { argvFile, module, outFile, plugin } = await renderPlugin(t);
  assert.equal(module.default.id, "pixtuoid", "the export id must match the manifest id");
  register(plugin).get("gateway_stop")({ reason: "shutdown" }, {});
  const [stop] = await recorded(outFile, 1);
  assert.equal(stop.type, "gateway_stop");
  assert.equal(stop.reason, "shutdown");
  const argv = (await readFile(argvFile, "utf8")).trim().split("\n");
  assert.deepEqual(argv, ["--source openclaw"], "the shim must be told which source it speaks for");
});
