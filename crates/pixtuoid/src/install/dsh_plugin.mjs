// @pixtuoid-dsh-plugin — pixtuoid's cordis plugin for DeepSeek Harness.
// Written by `pixtuoid connect dsh`; edits are overwritten on reconnect.
// Zero @deepseek-ai imports ON PURPOSE: everything arrives through `ctx`, so
// this single file mounts from any absolute path with no package install and
// no second cordis instance.
const HOOK_PATH = "{{HOOK_PATH_JSON}}"

import { spawn } from "node:child_process"

export const name = "pixtuoid"
export const inject = ["agents"]

// Every subscription below is an EMIT listener — never a waterfall/serial —
// so dsh never blocks on us and this plugin structurally cannot stall a tool
// call, a prompt, or shutdown.
const FORWARD = [
  "agent/session-start",
  "agent/disposed",
  "tool/call",
  "tool/result",
  "approval/asked",
  "approval/decided",
  "assistant/message",
  "request/header",
]

export function apply(ctx) {
  const send = (payload) => {
    try {
      payload._pid = typeof process !== "undefined" ? process.pid : undefined
      const proc = spawn(HOOK_PATH, ["--source", "dsh"], {
        stdio: ["pipe", "ignore", "ignore"],
      })
      proc.on("error", () => {})
      proc.stdin.on("error", () => {})
      proc.stdin.end(JSON.stringify(payload) + "\n")
    } catch {
      // A broken shim must never surface into dsh.
    }
  }

  const base = (header) => ({
    sessionId: String(header.id),
    ...(header.cwd ? { cwd: header.cwd } : {}),
    ...(header.parentSession ? { parentSession: String(header.parentSession) } : {}),
  })

  const started = new Set()
  const sendStart = (agent, reason) => {
    try {
      const header = agent.session.header
      if (started.has(String(header.id))) return
      started.add(String(header.id))
      const locate = ctx.get("sessionPersistence")?.locate(header)
      send({
        type: "session_start",
        ...base(header),
        ...(reason ? { reason } : {}),
        ...(locate?.path ? { transcriptPath: locate.path } : {}),
      })
    } catch {}
  }
  const sendEnd = (agent) => {
    try {
      const header = agent.session.header
      if (!started.delete(String(header.id))) return
      send({ type: "session_end", ...base(header) })
    } catch {}
  }

  ctx.on("agent/session-start", ({ agent, source }) => sendStart(agent, source))
  ctx.on("agent/disposed", ({ agent }) => sendEnd(agent))

  // approval/decided carries only {id, outcome}; the asked side owns
  // callId/toolName, so remember the pair until it resolves.
  const asked = new Map()
  ctx.on("session/event", (session, event) => {
    try {
      const header = session.header
      const d = event.data
      switch (event.type) {
        case "tool/call":
          send({
            type: "tool_call",
            ...base(header),
            callId: String(d.callId),
            toolName: d.name,
          })
          break
        case "tool/result": {
          const block = d.message?.content?.[0]
          send({
            type: "tool_result",
            ...base(header),
            ...(block?.toolCallId ? { callId: String(block.toolCallId) } : {}),
            isError: block?.isError === true || d.error !== undefined,
          })
          break
        }
        case "approval/asked":
          asked.set(String(d.id), { callId: d.callId, toolName: d.toolName })
          send({
            type: "approval_asked",
            ...base(header),
            approvalId: String(d.id),
            ...(d.callId ? { callId: String(d.callId) } : {}),
            toolName: d.toolName,
            ...(d.reason ? { reason: d.reason } : {}),
          })
          break
        case "approval/decided": {
          const pair = asked.get(String(d.id))
          asked.delete(String(d.id))
          send({
            type: "approval_decided",
            ...base(header),
            approvalId: String(d.id),
            outcome: d.outcome,
            ...(pair?.callId ? { callId: String(pair.callId) } : {}),
            ...(pair?.toolName ? { toolName: pair.toolName } : {}),
          })
          break
        }
        case "assistant/message":
          if (d.usage) {
            send({
              type: "usage",
              ...base(header),
              inputTokens: d.usage.inputTokens,
              outputTokens: d.usage.outputTokens,
              ...(d.usage.cacheWriteTokens !== undefined
                ? { cacheWriteTokens: d.usage.cacheWriteTokens }
                : {}),
            })
          }
          break
        case "request/header": {
          const config = d.header?.config
          if (config) {
            send({
              type: "model",
              ...base(header),
              provider: config.provider,
              model: config.model,
              ...(config.reasoningEffort ? { reasoningEffort: config.reasoningEffort } : {}),
            })
          }
          break
        }
        default:
      }
    } catch {}
  })

  // The plugin's fiber may unwind BEFORE the agent registry's during tree
  // teardown, so agent/disposed is not guaranteed for every live agent —
  // sweep them here. Best-effort: an unflushed child at process exit is
  // covered by the pid watch on the Rust side.
  ctx.effect(() => () => {
    try {
      for (const agent of ctx.agents.list()) sendEnd(agent)
    } catch {}
  })
}
