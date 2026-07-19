// Injected at build time from the workspace Cargo.toml (see astro.config.mjs).
declare const __PIXTUOID_VERSION__: string;
// Build-time GitHub star count ("342"), or null when the API was unreachable
// at build (offline builds must not fail) — consumers omit the count then.
declare const __GH_STARS__: string | null;

// The page's cross-component runtime contracts (producers/consumers documented
// in README.md "Cross-component seams"; existence pinned by tests/e2e). All
// optional: each consumer guards, and reduced-motion / pre-boot states leave
// some unset.
interface Window {
  /** THE site clock boundary (7/19) — defined in Base.astro's head boot. */
  __pixNight?: () => boolean;
  /** Per-frame dimmer opacity — written by OfficeBackdrop's controller. */
  __pixLights?: number;
  /** Hire a coworker into the live office — set once the wasm office boots.
   * Returns whether the engine admitted the hire (the receipt-event signal;
   * see `Office::hire`'s bool contract in pixtuoid-web/src/lib.rs). */
  __pixHire?: () => boolean;
  /** Boot splash lifted (mirrors the one-shot pix:revealed for a late listener);
   * set by Base.astro. Gates OfficeBackdrop's office-reveal roll. */
  __pixRevealed?: boolean;
  /** Office boot RESOLVED (live / failed / unsupported) — set by OfficeBackdrop.
   * The boot splash's Level-2 gate polls it so it lifts straight into the reveal. */
  __pixEngineReady?: boolean;
  /** The ONE shared wasm-init promise, memoized across the two live-office
   * consumers (hero backdrop + Showcase VIBING). Nulled on final retry
   * exhaustion so a later-booting consumer re-attempts (#671). */
  __pixWasm?: Promise<unknown> | null;
  /** THE theme registry + fallback, seeded parse-first in Base.astro's head. */
  __pixTheme?: {
    KEY: string;
    VALID: readonly string[];
    BG: Record<string, string>;
    ok: (_v: string) => boolean;
    fallback: () => string;
  };
  /** Key-shortcut guards (Base.astro): the typing-surface check, shared by
   * every single-char shortcut, and the WCAG 2.1.4 focus gate — which only
   * `t` (decorative retint) rides; digits 1–6 are document-global instead,
   * gated by enabled()/typing() below and the boot-splash guard. */
  __pixKeys?: {
    typing: (_e: Event) => boolean;
    shortcutContext: () => boolean;
    /** WCAG 2.1.4 off-switch for the bare digit shortcuts, persisted in
     * localStorage('pix-keys'); read at each keydown, written by the statusline
     * toggle. */
    enabled: () => boolean;
    setEnabled: (_on: boolean) => void;
  };
}
