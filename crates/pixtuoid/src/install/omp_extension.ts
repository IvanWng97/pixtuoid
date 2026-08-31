// @pixtuoid-omp-extension — managed by pixtuoid (connect/disconnect omp in pixtuoid's Sources panel: press s).
//
// omp's transcripts stay pixtuoid's durable authority; this bridge forwards
// ONLY what they can never carry — pre-persist presence, empty-session
// shutdown, and the tool-approval wait — into the `pixtuoid-hook` shim.
// It NEVER blocks omp: it registers NO gating handlers (omp's `tool_call`
// gate fails CLOSED on a throw/timeout), and every handler is try/catch'd.
// The ONE awaited thing is the shim itself — self-bounded at 200ms, always exit 0 —
// because Bun.spawn has no detached mode and `session_shutdown` fires during
// process exit: an un-awaited child loses the race and the empty-session
// shutdown never arrives (capture-verified on omp 18.0.11).
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
  const forward = async (name: string, ev: any, ctx: any) => {
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
      // Buffer stdin (no writable stream, no EPIPE window). Awaiting the
      // 200ms-watchdogged shim keeps the exiting process alive just long
      // enough for `session_shutdown` to land, and keeps an event burst from
      // orphaning a pile of spawns (the opencode-plugin posture).
      const proc = Bun.spawn([HOOK_PATH, "--source", "omp"], {
        stdin: new TextEncoder().encode(JSON.stringify(payload)),
        stdout: "ignore",
        stderr: "ignore",
      })
      await proc.exited
    } catch {
      // Best-effort: a broken shim must never surface in omp.
    }
  }
  for (const name of FORWARD) {
    // Per-event registration is try/catch'd so one unknown event name on an
    // older omp cannot take the whole bridge down.
    try {
      pi.on(name, (ev: any, ctx: any) => forward(name, ev, ctx))
    } catch {}
  }
}
