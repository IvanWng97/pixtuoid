# Security Policy

## Supported Versions

Fixes land on the latest released version; there are no maintained back-branches.
Upgrade to the newest release (`cargo install pixtuoid`, `brew upgrade`, or
`npm i -g pixtuoid`) to receive security fixes.

| Version        | Supported |
|----------------|-----------|
| latest         | ✅        |
| older releases | ❌        |

## Privacy posture

pixtuoid is **local-only and telemetry-free by design**:

- **No network egress.** The binary makes no outbound network connections — no
  analytics, crash-reporting upload, update check, or "phone home". (The crash
  hook writes a backtrace to a *local* file and only ever offers a pre-filled
  GitHub URL for you to open manually.) The dependency set is audited daily for
  advisories via `cargo-deny` (see `.github/workflows/audit.yml`).
- **Your session data stays on your machine.** pixtuoid reads your agent CLIs'
  transcripts (e.g. `~/.claude/projects`, `~/.codex/sessions`) **read-only**, to
  derive what each sprite is doing; nothing is transmitted anywhere.

## Trust boundaries / attack surface

The components that handle untrusted or privileged input, and how they're bounded:

1. **The hook shim (`pixtuoid-hook`)** is invoked by your agent CLI and forwards a
   single JSON line from stdin to the office over a **Unix domain socket** (a
   per-user runtime path — `$XDG_RUNTIME_DIR/pixtuoid.sock`, else
   `/tmp/pixtuoid-<uid>.sock`; `PIXTUOID_SOCKET` overrides — created `0600`,
   owner-only) or a Windows named pipe. It is a *local IPC* — there is no network
   listener. The shim is hardened to **never block the agent**: it always exits
   `0`, within a hard ~200 ms watchdog bound, on any error. It does not execute or
   shell out to anything in the payload.

2. **The office's socket listener** binds that owner-only socket with a flock'd,
   `O_NOFOLLOW`, atomic-rename bind — only the local user can connect. The CLI's
   plain-text surfaces (the `--headless` summary, the `doctor` report, the
   Sources/install output) **strip control characters** from untrusted wire values
   (`strip_control_chars`) so a crafted transcript can't smuggle escape sequences
   into piped output. The live half-block TUI renders into its own cell grid (agent
   labels derive from project-directory path components, read over the user's own
   `0600` socket and transcripts), not by echoing raw bytes to the terminal.

3. **Hook installation** (when you explicitly *connect* a source) edits the agent
   CLI's own config — e.g. `~/.claude/settings.json` — through a single
   advisory-locked, `fsync` + atomic-rename writer that **preserves the file's
   permissions, follows stow symlinks, and takes a one-time backup** before the
   first change. Installs are idempotent and reversible (*disconnect* removes the
   hook entries via a sentinel). pixtuoid never writes another tool's config
   except on an explicit connect/disconnect.

If you find a way to cross one of these boundaries (e.g. a transcript or hook
payload that escapes the terminal, a non-owner socket connect, an install path
traversal, or any network egress), please report it.

## Reporting a Vulnerability

Report privately via
[GitHub private vulnerability reporting](https://github.com/IvanWng97/pixtuoid/security/advisories/new).

You will receive acknowledgement within 48 hours, and a fix or mitigation plan
within 7 days for confirmed vulnerabilities.

**Do not** open a public issue for security vulnerabilities.
