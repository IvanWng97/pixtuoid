# `.harness/` — flight recorder + action gate (dogfood v0)

An **evidence-first** guardrail layer for this repo's Claude Code sessions. It is
**not** a security sandbox — a same-uid agent can still bypass it. Its job is to
make a bypass or a *loosened guardrail* **visible**, not impossible.

## What it does

- **Gates** each proposed tool action (`Edit`/`Write`/`Bash`/…) against a
  declarative policy — matched on the **real `tool_input`** (the model can't fake
  which call it's making), so a `block` is legitimate. In `enforce` mode a deny
  returns **exit 2** (blocks, before the permission check); in the **default
  `observe` mode it records the would-be block and returns 0** — a pure
  flight-recorder with zero friction. Flip via `mode` in `policy.toml`.
- **Records** every decision as a **hash-chained** line in `ledger.jsonl` — the
  flight recorder. Tampering (editing/deleting a past line) breaks the chain.
- **Fails loud, never silent.** A gate bug fails **closed** (exit 2); the
  `canary` catches the gate going inert.

## Files

| File | Role |
|---|---|
| `policy.toml` | the rules (machine form of CLAUDE.md's invariants); editing it is itself gated |
| `harness.py` | the engine — `gate` / `replay` / `canary` / `verify` / `selftest` |
| `fixtures.jsonl` | labeled adversarial + allowed actions — the replay/eval corpus |
| `ledger.jsonl` | the hash-chained flight recorder (git-ignored; runtime data) |

## Use

```sh
just harness-check          # selftest + replay + canary + verify (the RED-on-loosen CI gate)
python3 .harness/harness.py selftest
HARNESS_GATE_OFF=1 …        # escape hatch: records a BYPASS line + allows (audited, not silent)
```

Wired as a `PreToolUse` hook in `.claude/settings.json` (session-scoped; takes
effect on the **next** session launched in this worktree).

## Operating rules

**Reach for it when:** value = (how *unsupervised* the agent is) × (how *un-lintable*
the rule is). High-value = autonomous/overnight runs, subagent fan-outs, or a
convention the compiler/clippy can't express. Near-zero value = you hand-reviewing
every diff on a rule `clippy`/`just arch` already catches. → stays `observe` until
autonomy rises.

**Update it when:** (add) a new CLAUDE.md invariant that's cheap-per-action AND
non-lintable; (add) a real incident nothing caught early — encode it as a rule +
fixture ("never again"); (fix) the drift selftest reddens or a false-positive keeps
firing; (fix) the CC hook contract changes; (**delete**) an invariant becomes
compiler/clippy-enforced — a rule whose owner got upgraded is dead weight. Before
adding any rule: *can a stronger layer (compiler > clippy > gate) own it cheaply?*
If yes, put it there, not here.

## Honest limits (read before trusting it)

- **Not a sandbox.** Bypass paths remain: disabling the hook, obfuscated bash
  (heredoc/redirect), and `Agent`-tool subagents whose tool calls don't fire the
  parent hook. Real enforcement of those needs CC's OS sandbox + managed-settings
  tier — **not built here** (out of scope for v0).
- **Ledger is tamper-EVIDENT, not tamper-PROOF** — same-uid, so the model can
  rewrite it; the hash-chain makes that *detectable*, not prevented.
- **Best-effort matchers.** The Bash rule is a string tripwire; `println!` in an
  in-`src` `#[cfg(test)]` module is a known false-positive (use the escape hatch).

## Kill switch

This is a **dogfood experiment**. If two weeks of real use doesn't earn its keep,
delete `.harness/`, revert `.claude/settings.json`, drop the `harness-check`
recipe. No repo, no ceremony.
