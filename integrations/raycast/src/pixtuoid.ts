import { getPreferenceValues } from "@raycast/api";
import { execFile, spawn } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const pExecFile = promisify(execFile);

/** A row of `pixtuoid sources --json` — the stable wire contract pinned by the
 *  binary's `source_status_json_shape` test. Mirror changes there here. */
export interface SourceStatus {
  id: string;
  display_name: string;
  connected: boolean;
  cli_present: boolean;
  health: string | null;
}

/** A row of `pixtuoid connect|disconnect <id> --json` (`run_change`).
 *  `outcome` ∈ `"connected" | "disconnected" | "no_op" | "failed: <msg>"`. */
export interface OutcomeRow {
  id: string;
  outcome: string;
}

/** Thrown when the pixtuoid executable can't be located — the UI distinguishes
 *  this (offer the preference / install docs) from a runtime error. */
export class BinaryNotFoundError extends Error {
  constructor() {
    super("pixtuoid executable not found");
    this.name = "BinaryNotFoundError";
  }
}

interface Preferences {
  binaryPath?: string;
}

function expandTilde(p: string): string {
  if (p === "~") return homedir();
  if (p.startsWith("~/")) return join(homedir(), p.slice(2));
  return p;
}

function isExecutable(p: string): boolean {
  try {
    accessSync(p, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

let cached: string | undefined;

/**
 * Resolve the pixtuoid binary. Raycast runs extensions in a Node subprocess
 * with a MINIMAL PATH (no Homebrew / Cargo / npm-global dirs), so a bare
 * `pixtuoid` lookup fails for most installs — we resolve an absolute path:
 *   1. the `binaryPath` preference (validated), else
 *   2. the user's LOGIN SHELL `command -v pixtuoid` (their real PATH), else
 *   3. the common install locations.
 * Cached per command run (a process is one command in Raycast).
 */
export async function resolveBinary(): Promise<string> {
  if (cached) return cached;

  const { binaryPath } = getPreferenceValues<Preferences>();
  if (binaryPath && binaryPath.trim()) {
    const p = expandTilde(binaryPath.trim());
    if (isExecutable(p)) return (cached = p);
    throw new BinaryNotFoundError();
  }

  const shell = process.env.SHELL || "/bin/zsh";
  try {
    const { stdout } = await pExecFile(shell, ["-lc", "command -v pixtuoid"], {
      timeout: 5000,
    });
    const p = stdout.trim();
    if (p && isExecutable(p)) return (cached = p);
  } catch {
    // Login-shell resolution failed (e.g. non-interactive guard) — fall through.
  }

  const candidates = [
    "/opt/homebrew/bin/pixtuoid",
    "/usr/local/bin/pixtuoid",
    join(homedir(), ".cargo", "bin", "pixtuoid"),
    join(homedir(), ".local", "bin", "pixtuoid"),
  ];
  for (const c of candidates) {
    if (isExecutable(c)) return (cached = c);
  }

  throw new BinaryNotFoundError();
}

/** Run pixtuoid with `args` (no shell — args are passed as an array, so a
 *  source id can never be interpreted as a shell token). Returns stdout. */
async function runPixtuoid(args: string[]): Promise<string> {
  const bin = await resolveBinary();
  const { stdout } = await pExecFile(bin, args, { timeout: 20000 });
  return stdout;
}

export async function getSources(): Promise<SourceStatus[]> {
  const out = await runPixtuoid(["sources", "--json"]);
  return JSON.parse(out) as SourceStatus[];
}

/** Toggle one source: a connected source disconnects, otherwise it connects.
 *  Returns the single `OutcomeRow` the CLI emits for the id. */
export async function toggleSource(id: string, connected: boolean): Promise<OutcomeRow> {
  const cmd = connected ? "disconnect" : "connect";
  const out = await runPixtuoid([cmd, id, "--json"]);
  const rows = JSON.parse(out) as OutcomeRow[];
  return rows[0];
}

/** Spawn `pixtuoid floating` DETACHED so the desktop window outlives Raycast
 *  (which closes as soon as the no-view command returns). */
export async function startFloating(): Promise<void> {
  const bin = await resolveBinary();
  const child = spawn(bin, ["floating"], { detached: true, stdio: "ignore" });
  child.unref();
}
