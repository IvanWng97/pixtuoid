// Contract test for `crates/pixtuoid/src/install/dsh_plugin.mjs`. The Rust side
// can only grep the template as a STRING; this drives the RENDERED module the way
// dsh's cordis loader does, against a REAL recorder executable, so the order the
// shims reach the socket in is exercised, not stubbed.

import assert from "node:assert/strict";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const PLUGIN_SOURCE = join(REPO_ROOT, "crates", "pixtuoid", "src", "install", "dsh_plugin.mjs");
// Must match `HOOK_PLACEHOLDER` in `crates/pixtuoid/src/install/dsh.rs`.
const HOOK_PLACEHOLDER = '"{{HOOK_PATH_JSON}}"';

const POSIX = process.platform !== "win32";

// A recorder that holds each call open, so two shims spawned together overlap,
// and logs when each starts and ends beside the payload it received.
async function renderPlugin(t) {
  const root = await mkdtemp(join(tmpdir(), "pixtuoid-dsh-plugin-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = await readFile(PLUGIN_SOURCE, "utf8");
  assert.equal(source.split(HOOK_PLACEHOLDER).length, 2, "one hook placeholder");

  const outFile = join(root, "payloads.ndjson");
  const traceFile = join(root, "trace.txt");
  const hookPath = join(root, "recorder-hook");
  await writeFile(
    hookPath,
    `#!/bin/sh\necho begin >> ${JSON.stringify(traceFile)}\nsleep 0.1\n` +
      `cat >> ${JSON.stringify(outFile)}\necho end >> ${JSON.stringify(traceFile)}\n`,
  );
  await chmod(hookPath, 0o755);

  const entry = join(root, "pixtuoid-dsh.mjs");
  await writeFile(entry, source.replace(HOOK_PLACEHOLDER, JSON.stringify(hookPath)));
  const module = await import(pathToFileURL(entry).href);
  return { module, outFile, traceFile };
}

// The cordis surface the plugin touches: `on`, `effect`, and the injected agents.
function apply(module) {
  const handlers = new Map();
  module.apply({
    on(name, handler) {
      handlers.set(name, handler);
    },
    effect() {},
    agents: { list: () => [] },
  });
  return handlers;
}

async function recorded(outFile, n) {
  const deadline = Date.now() + 10_000;
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

test("events reach the shim in emission order, one shim at a time", { skip: !POSIX }, async (t) => {
  const { module, outFile, traceFile } = await renderPlugin(t);
  const handlers = apply(module);
  const header = { id: "s1", cwd: "/w" };
  const session = { header };
  const event = (type, data) => handlers.get("session/event")(session, { type, data });

  // Synchronous and void: `agent/created` is serial in dsh, so a returned
  // promise would hold the agent's creation.
  assert.equal(handlers.get("agent/created")({ agent: { session } }), undefined);
  event("approval/asked", { id: "a1", callId: "c1", toolName: "bash", reason: "wider" });
  event("approval/decided", { id: "a1", outcome: "unavailable" });
  event("tool/result", { message: { toolCallId: "c1" } });

  const types = (await recorded(outFile, 4)).map((p) => p.type);
  assert.deepEqual(types, ["session_start", "approval_asked", "approval_decided", "tool_result"]);
  const trace = (await readFile(traceFile, "utf8")).trim().split("\n");
  assert.deepEqual(trace, ["begin", "end", "begin", "end", "begin", "end", "begin", "end"]);
});
