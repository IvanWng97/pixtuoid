// @pixtuoid-opencode-plugin — managed by pixtuoid (connect/disconnect opencode in pixtuoid's Sources panel: press s).
//
// opencode has no config-level shell hook, so pixtuoid integrates as a tiny plugin
// (auto-discovered from `<config>/plugins/*.ts`) that pipes the session lifecycle /
// tool / permission events into the `pixtuoid-hook` shim on stdin. It NEVER blocks
// or breaks opencode (the shim is best-effort, bounded by its own write-timeout
// watchdog, `transport::WRITE_TIMEOUT`, and every error here is swallowed —
// invariant #5).
//
// HOOK_PATH is baked in at install time (a JSON-encoded absolute path). Safe to
// delete — disconnecting opencode in pixtuoid's Sources panel replaces this
// with a removed-marker stub.
import { spawn } from "node:child_process"

const HOOK_PATH: string = "{{HOOK_PATH_JSON}}"

// Only forward what pixtuoid maps; the decoder (`source/opencode.rs`) ignores
// anything else anyway.
const FORWARD = new Set<string>([
  "session.created",
  "session.deleted",
  "session.execution.started",
  "session.execution.succeeded",
  "session.execution.failed",
  "session.execution.interrupted",
  "session.tool.input.started",
  "session.tool.success",
  "session.tool.failed",
  "permission.asked",
])

// Copied field by field, so no prompt, tool input or output leaves opencode.
const allowlisted = (data: any) => ({
  sessionID: data?.sessionID,
  parentID: data?.parentID,
  location: data?.location && { directory: data.location.directory },
  model: data?.model && { id: data.model.id },
  id: data?.id,
  name: data?.name,
  action: data?.action,
  source: data?.source && { id: data.source.id },
})

// Resolves once the shim exits or fails to start; never rejects.
const send = (payload: string) =>
  new Promise<void>((resolve) => {
    try {
      // No `detached`: on Windows it gives each shim its own console window.
      const child = spawn(HOOK_PATH, ["--source", "opencode"], {
        stdio: ["pipe", "ignore", "ignore"],
        windowsHide: true,
      })
      child.on("error", () => resolve())
      child.on("close", () => resolve())
      child.stdin?.on("error", () => {})
      child.stdin?.end(payload)
    } catch {
      resolve()
    }
  })

// A session's record, or nothing when it cannot be read: `ctx.session.get`'s
// endpoint answers `{ data: PublicSessionInfo }`
// (`packages/protocol/src/groups/session.ts:127-130`, `:287-290`), whose lone
// `data` the promise adapter unwraps (`packages/plugin/src/promise/adapter.ts:186-197`),
// so it returns the `Session.Info` (`packages/schema/src/session.ts:31-59`). A
// fork's row keeps its source in `fork_session_id` with `parent_id` null
// (`packages/core/src/session/projector.ts:160-161`), read back as `fork`, not
// `parentID` (`packages/core/src/session/info.ts:21-28`), so a fork stays a root.
const record = async (ctx: any, sessionID: unknown) => {
  try {
    return (await ctx.session.get({ sessionID })) ?? {}
  } catch {
    return {}
  }
}

export default {
  id: "pixtuoid",
  setup(ctx: any) {
    const controller = new AbortController()
    // Not awaited: setup returns at once, and the loop's own awaits only pace
    // this subscription, whose queue is unbounded.
    void (async () => {
      try {
        for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
          if (!FORWARD.has(event?.type)) continue
          // A turn's start carries only `sessionID`; the session's record adds
          // what `session.created` did, for a session swept since.
          const data =
            event.type === "session.execution.started"
              ? { ...(await record(ctx, event.data?.sessionID)), ...event.data }
              : event.data
          await send(
            JSON.stringify({
              type: event.type,
              location: event.location && { directory: event.location.directory },
              data: allowlisted(data),
              // The opencode server's pid; the daemon's HookPidWatch ends every
              // sprite bound to it when it dies.
              _pid: process.pid,
            }),
          )
        }
      } catch {
        // Best-effort: a failed stream must never surface in opencode.
      }
    })()
    return () => controller.abort()
  },
}
