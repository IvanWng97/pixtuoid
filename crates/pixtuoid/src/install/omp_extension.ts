// @pixtuoid-omp-extension — managed by pixtuoid (connect/disconnect omp in pixtuoid's Sources panel: press s).
//
// omp's transcripts stay pixtuoid's durable authority; this bridge forwards
// ONLY what they can never carry — pre-persist presence, empty-session
// shutdown, and the tool-approval wait — into the `pixtuoid-hook` shim.
// It NEVER blocks omp: it registers NO gating handlers (omp's `tool_call`
// gate fails CLOSED on a throw/timeout), every handler is try/catch'd, and
// nothing is awaited — the shim self-bounds at 200ms and always exits 0.
//
// Privacy allowlist: event type, session file/id, cwd, tool name, tool call
// id, approval state/reason, and process.pid. Never prompts, messages, tool
// arguments, tool results, or model output.
//
// HOOK_PATH is baked in at install time (a JSON-encoded absolute path). Safe
// to delete — disconnecting omp in pixtuoid's Sources panel replaces this
// with a removed-marker stub. Already-running omp sessions load extensions at
// startup, so they must restart to pick this up.
const HOOK_PATH: string = "{{HOOK_PATH_JSON}}"

// Observe-only lifecycle + approval events; the decoder
// (`pixtuoid-core/src/source/omp.rs`) claims exactly this set.
const FORWARD = new Set<string>([
  "session_start",
  "session_switch",
  "session_branch",
  "session_shutdown",
  "tool_approval_requested",
  "tool_approval_resolved",
])

export default function (pi: any) {
  const forward = (name: string, ev: any, ctx: any) => {
    try {
      const payload: Record<string, unknown> = { type: name }
      try {
        payload.sessionFile = ctx?.sessionManager?.getSessionFile?.()
      } catch {}
      try {
        payload.sessionId = ctx?.sessionManager?.getSessionId?.()
      } catch {}
      if (typeof ctx?.cwd === "string") payload.cwd = ctx.cwd
      if (typeof ev?.previousSessionFile === "string") payload.previousSessionFile = ev.previousSessionFile
      if (typeof ev?.toolCallId === "string") payload.toolCallId = ev.toolCallId
      if (typeof ev?.toolName === "string") payload.toolName = ev.toolName
      if (typeof ev?.reason === "string") payload.reason = ev.reason
      if (typeof ev?.approved === "boolean") payload.approved = ev.approved
      // The omp process pid (extensions run in-process); the daemon's
      // HookPidWatch can end every bound sprite when it dies.
      payload._pid = typeof process !== "undefined" ? process.pid : undefined
      // Buffer stdin (no writable stream, no EPIPE window) and NOTHING
      // awaited: `session_shutdown` runs during process exit, and a slow
      // shim must never hold omp's 30s handler budget.
      Bun.spawn([HOOK_PATH, "--source", "omp"], {
        stdin: new TextEncoder().encode(JSON.stringify(payload)),
        stdout: "ignore",
        stderr: "ignore",
      })
    } catch {
      // Best-effort: a broken shim must never surface in omp.
    }
  }
  for (const name of FORWARD) {
    // Per-event registration is try/catch'd so one unknown event name on an
    // older omp cannot take the whole bridge down.
    try {
      pi.on(name, (ev: any, ctx: any) => {
        forward(name, ev, ctx)
      })
    } catch {}
  }
}
