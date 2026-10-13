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

async function renderPlugin(t) {
  const root = await mkdtemp(join(tmpdir(), "pixtuoid-dsh-plugin-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = await readFile(PLUGIN_SOURCE, "utf8");
  assert.equal(source.split(HOOK_PLACEHOLDER).length, 2, "one hook placeholder");

  const outFile = join(root, "payloads.ndjson");
  const traceFile = join(root, "trace.txt");
  const hookPath = join(root, "recorder-hook");
  // Holds each call open, so two shims spawned together overlap in the trace.
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

// The first `n` lines of `file` once it holds them: the shims write after the
// listeners return.
async function lines(file, n) {
  const deadline = Date.now() + 10_000;
  for (;;) {
    const text = await readFile(file, "utf8").catch(() => "");
    const got = text.split("\n").filter((l) => l.trim().length > 0);
    if (got.length >= n) return got.slice(0, n);
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${n} line(s) of ${file}; got ${got.length}`);
    }
    await new Promise((r) => setTimeout(r, 25));
  }
}

const header = { id: "s1", cwd: "/w" };
const session = { header };

test("events reach the shim in emission order, one shim at a time", { skip: !POSIX }, async (t) => {
  const { module, outFile, traceFile } = await renderPlugin(t);
  const handlers = apply(module);
  const event = (type, data) => handlers.get("session/event")(session, { type, data });

  handlers.get("agent/created")({ agent: { session } });
  event("approval/asked", { id: "a1", callId: "c1", toolName: "bash", reason: "wider" });
  event("approval/decided", { id: "a1", outcome: "unavailable" });
  event("tool/result", { message: { toolCallId: "c1" } });

  const types = (await lines(outFile, 4)).map((l) => JSON.parse(l).type);
  assert.deepEqual(types, ["session_start", "approval_asked", "approval_decided", "tool_result"]);
  const trace = await lines(traceFile, 8);
  assert.deepEqual(trace, ["begin", "end", "begin", "end", "begin", "end", "begin", "end"]);
});

test("agent/created's listener returns nothing", async (t) => {
  const { module } = await renderPlugin(t);
  const handlers = apply(module);
  assert.equal(
    handlers.get("agent/created")({ agent: { session } }),
    undefined,
    "agent/created's listener must return nothing",
  );
});

test("forwards only the allowlist, never content", { skip: !POSIX }, async (t) => {
  const { module, outFile } = await renderPlugin(t);
  const handlers = apply(module);
  const event = (type, data) => handlers.get("session/event")(session, { type, data });

  handlers.get("agent/created")({
    agent: { session: { header: { ...header, title: "SECRET_TITLE", prompt: "SECRET_PROMPT" } } },
  });
  event("tool/call", { callId: "c1", name: "bash", input: { command: "SECRET_ARGUMENT" } });
  event("approval/asked", {
    id: "a1",
    callId: "c1",
    toolName: "bash",
    reason: "wider access",
    input: { command: "SECRET_ARGUMENT" },
  });
  event("approval/decided", { id: "a1", outcome: "unavailable", note: "SECRET_NOTE" });
  event("tool/result", { message: { toolCallId: "c1", content: [{ text: "SECRET_RESULT" }] } });
  event("assistant/message", {
    content: [{ type: "text", text: "SECRET_OUTPUT" }],
    usage: { inputTokens: 1, outputTokens: 2 },
  });
  event("request/header", {
    header: { config: { model: "m", system: "SECRET_SYSTEM" } },
    messages: ["SECRET_MESSAGE"],
  });

  // Exact payloads: a key nobody planted fails too.
  const at = { sessionId: "s1", cwd: "/w", _pid: process.pid };
  const forwarded = (await lines(outFile, 7)).map((l) => JSON.parse(l));
  assert.deepEqual(forwarded, [
    { type: "session_start", ...at },
    { type: "tool_call", ...at, callId: "c1", toolName: "bash" },
    { type: "approval_asked", ...at, callId: "c1", toolName: "bash", reason: "wider access" },
    { type: "approval_decided", ...at, outcome: "unavailable", callId: "c1", toolName: "bash" },
    { type: "tool_result", ...at, callId: "c1" },
    { type: "usage", ...at, inputTokens: 1, outputTokens: 2 },
    { type: "model", ...at, model: "m" },
  ]);
});
