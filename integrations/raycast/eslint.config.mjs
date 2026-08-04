import { defineConfig } from "eslint/config";
import raycastConfig from "@raycast/eslint-config";

// The ignores are all GENERATED: `raycast-env.d.ts` + `dist/` come from `ray build`,
// the two `src/contract*.ts` from `npm run gen:contract`.
export default defineConfig([
  { ignores: ["raycast-env.d.ts", "dist/**", "src/contract.ts", "src/contract-outcome.ts"] },
  ...raycastConfig,
  {
    // A chord Raycast reserves is silently swallowed, so the Action it decorates is
    // unreachable — and no other gate here can see it (`tsc` types the shortcut fine;
    // `ray build`/`ray lint` need the macOS app). Upstream ships this at WARN and
    // `eslint .` exits 0 on warnings, which is how a dead `⌘,` binding shipped.
    // Scoped to this one rule rather than a blanket `--max-warnings 0`: upstream's
    // style rules are advice a routine version bump could turn into a surprise red.
    rules: { "@raycast/no-reserved-shortcut": "error" },
  },
]);
