// @pixtuoid-opencode-plugin — managed by `pixtuoid install-hooks --target opencode`.
//
// opencode has no config-level shell hook, so pixtuoid integrates as a tiny
// opencode plugin (auto-discovered from `<config>/plugins/*.ts`). This pipes the
// session lifecycle / tool / permission events pixtuoid maps into the
// `pixtuoid-hook` shim on stdin; the shim forwards them over the pixtuoid
// daemon's socket. It NEVER blocks or breaks opencode (the shim is best-effort,
// self-bounds at 200ms, and every error here is swallowed — invariant #5).
//
// HOOK_PATH is baked in at install time (a JSON-encoded absolute path). Safe to
// delete — `pixtuoid uninstall-hooks --target opencode` replaces this with a
// removed-marker stub.
const HOOK_PATH: string = {{HOOK_PATH_JSON}}

// Only forward what pixtuoid maps. `message.part.updated` fires per token, so we
// gate it to tool parts — AND only on a tool-state TRANSITION (see below),
// instead of flooding the socket. The decoder (`source/opencode.rs`) ignores
// anything else anyway.
const FORWARD = new Set<string>([
  "session.created",
  "session.deleted",
  "permission.asked",
  "permission.v2.asked",
])

// opencode re-publishes a tool part with status STILL `running` on EVERY output
// chunk (streaming `bash`/`task` output). Forwarding each would spawn a shim per
// chunk and inflate the tooltip's tool-call count, so we forward a tool part only
// when its status CHANGES (the pending→running→completed edges). The key is
// `sessionID:callID`, NOT callID alone: this Map is module-level (one plugin
// process serves a parent AND its task-spawned child sessions concurrently), so
// callID alone could collide across sessions and drop the wrong one. Bounded by
// the live tool-call count; entries are freed on a terminal status.
const lastToolStatus = new Map<string, string>()

export const PixtuoidOpencode = async () => ({
  event: async ({ event }: { event: { type: string; properties: any } }) => {
    try {
      const t = event?.type
      let keep = FORWARD.has(t)
      if (t === "message.part.updated" && event?.properties?.part?.type === "tool") {
        const part = event.properties.part
        const callID: unknown = part?.callID
        const status: unknown = part?.state?.status
        const sessionID: unknown = event.properties?.sessionID
        if (typeof callID === "string" && typeof status === "string") {
          const key = `${typeof sessionID === "string" ? sessionID : "?"}:${callID}`
          if (lastToolStatus.get(key) === status) return // same status re-published (streaming) — drop
          lastToolStatus.set(key, status)
          if (status === "completed" || status === "error") lastToolStatus.delete(key)
        }
        keep = true
      }
      if (!keep) return
      const payload = JSON.stringify({
        type: t,
        properties: event.properties,
        // The opencode process pid (the in-process worker shares the CLI's pid);
        // the daemon's HookPidWatch ends every bound sprite when it dies.
        _pid: typeof process !== "undefined" ? process.pid : undefined,
      })
      const proc = Bun.spawn([HOOK_PATH, "--source", "opencode"], {
        stdin: new TextEncoder().encode(payload),
        stdout: "ignore",
        stderr: "ignore",
      })
      // opencode does NOT await this hook (`void hook.event(...)`), so awaiting
      // the (200ms-bounded, always-exit-0) shim here can't stall opencode — it
      // just keeps a burst of events from orphaning a pile of spawns.
      await proc.exited
    } catch {
      // Best-effort: a broken shim must never surface in opencode.
    }
  },
})

export default PixtuoidOpencode
