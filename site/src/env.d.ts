// Injected at build time from the workspace Cargo.toml (see astro.config.mjs).
declare const __PIXTUOID_VERSION__: string;

// The page's cross-component runtime contracts (producers/consumers documented
// in README.md "Cross-component seams"; existence pinned by tests/e2e). All
// optional: each consumer guards, and reduced-motion / pre-boot states leave
// some unset.
interface Window {
  /** THE site clock boundary (7/19) — defined in Base.astro's head boot. */
  __pixNight?: () => boolean;
  /** Per-frame dimmer opacity — written by OfficeBackdrop's controller. */
  __pixLights?: number;
  /** Hire a coworker into the live office — set once the wasm office boots. */
  __pixHire?: () => void;
  /** THE theme registry + fallback, seeded parse-first in Base.astro's head. */
  __pixTheme?: {
    KEY: string;
    VALID: readonly string[];
    BG: Record<string, string>;
    ok: (_v: string) => boolean;
    fallback: () => string;
  };
  /** Key-shortcut guards (Base.astro): the typing-surface check + the WCAG
   * 2.1.4 focus gate for the bare single-char shortcuts (digits 1–6, t). */
  __pixKeys?: {
    typing: (_e: Event) => boolean;
    shortcutContext: () => boolean;
  };
}
