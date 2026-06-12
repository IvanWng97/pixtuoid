#!/usr/bin/env python3
"""Extract per-agent cost/role metrics from a Workflow transcript dir.

The review pipeline's economics live in the workflow journals
(`<session>/subagents/workflows/wf_*/agent-*.jsonl`): every assistant message
carries an Anthropic `usage` block. This collector turns one workflow run into
a metrics JSON so reviews become COMPARABLE across time — the before/after
evidence for the knowledge-base experiments (REVIEW-LEDGER.md), not vibes.

Usage:
    review-metrics.py <wf-dir> [--label NAME] [--json OUT.json]

Role classification is keyword-based on each agent's first user prompt —
crude but stable for our review workflows (finder/dedup/verify naming is
part of the prompts we send).
"""

import argparse
import json
import sys
from pathlib import Path

# (role, keywords matched against the lowercased first user prompt)
ROLE_KEYWORDS = [
    ("verifier", ("adversarial", "verify", "skeptic", "refute", "verdict", "judge")),
    ("dedup", ("dedup", "deduplicate", "merge the findings", "distinct findings")),
    ("finder", ("find bugs", "finder", "review the", "audit", "subsystem", "lens")),
    ("implementer", ("implement", "fix the", "apply the", "build ")),
]


def first_user_prompt(path: Path) -> str:
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            msg = rec.get("message")
            if not isinstance(msg, dict):
                continue
            if msg.get("role") == "user":
                content = msg.get("content")
                if isinstance(content, str):
                    return content
                if isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict) and block.get("type") == "text":
                            return block.get("text", "")
    return ""


def classify(prompt: str) -> str:
    p = prompt.lower()
    for role, keys in ROLE_KEYWORDS:
        if any(k in p for k in keys):
            return role
    return "other"


def agent_metrics(path: Path) -> dict:
    out_tok = in_uncached = cache_read = cache_write = calls = 0
    first_ts = last_ts = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts = rec.get("timestamp")
            if ts:
                first_ts = first_ts or ts
                last_ts = ts
            msg = rec.get("message")
            usage = msg.get("usage") if isinstance(msg, dict) else None
            if isinstance(usage, dict):
                calls += 1
                out_tok += usage.get("output_tokens") or 0
                in_uncached += usage.get("input_tokens") or 0
                cache_read += usage.get("cache_read_input_tokens") or 0
                cache_write += usage.get("cache_creation_input_tokens") or 0
    prompt = first_user_prompt(path)
    return {
        "agent": path.stem,
        "role": classify(prompt),
        "prompt_head": prompt[:160].replace("\n", " "),
        "api_calls": calls,
        "output_tokens": out_tok,
        "input_tokens_uncached": in_uncached,
        "cache_read_tokens": cache_read,
        "cache_write_tokens": cache_write,
        "first_ts": first_ts,
        "last_ts": last_ts,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("wf_dir", type=Path)
    ap.add_argument("--label", default=None)
    ap.add_argument("--json", type=Path, default=None)
    args = ap.parse_args()

    agents = sorted(args.wf_dir.glob("agent-*.jsonl"))
    if not agents:
        sys.exit(f"no agent-*.jsonl under {args.wf_dir}")

    rows = [agent_metrics(p) for p in agents]
    by_role: dict[str, dict] = {}
    for r in rows:
        b = by_role.setdefault(r["role"], {"agents": 0, "output_tokens": 0, "cache_write_tokens": 0})
        b["agents"] += 1
        b["output_tokens"] += r["output_tokens"]
        b["cache_write_tokens"] += r["cache_write_tokens"]

    summary = {
        "label": args.label or args.wf_dir.name,
        "wf_dir": str(args.wf_dir),
        "agents": len(rows),
        "output_tokens": sum(r["output_tokens"] for r in rows),
        "input_tokens_uncached": sum(r["input_tokens_uncached"] for r in rows),
        "cache_read_tokens": sum(r["cache_read_tokens"] for r in rows),
        "cache_write_tokens": sum(r["cache_write_tokens"] for r in rows),
        "by_role": by_role,
        "agents_detail": rows,
    }
    if args.json:
        args.json.write_text(json.dumps(summary, indent=1))

    print(f"{summary['label']}: {summary['agents']} agents, "
          f"out={summary['output_tokens']:,} cacheW={summary['cache_write_tokens']:,} "
          f"cacheR={summary['cache_read_tokens']:,}")
    for role, b in sorted(by_role.items(), key=lambda kv: -kv[1]["output_tokens"]):
        print(f"  {role:<12} {b['agents']:>3} agents  out={b['output_tokens']:>10,}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
