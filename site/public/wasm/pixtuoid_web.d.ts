/* tslint:disable */
/* eslint-disable */

/**
 * A live office rendered to a reusable RGBA buffer across frames. Owns the
 * per-floor render caches (`FloorCtx`) + the persistent office state
 * (coffee/chitchat) so keeping ONE handle alive across `step` calls is what
 * keeps motion/pose continuous (no walk-flash) — same contract as
 * `OfficeRenderer`.
 */
export class Office {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Byte length of the RGBA frame (`w*h*4`).
     */
    frame_len(): number;
    /**
     * Pointer to the RGBA frame in wasm linear memory (`w*h*4` bytes).
     *
     * CONTRACT: re-read this (and rebuild any `Uint8ClampedArray` view) after
     * EVERY `step` — a canvas resize reallocates the staging buffer (the
     * pointer moves), and any wasm `memory.grow` invalidates existing JS
     * views into linear memory even when the pointer value is unchanged.
     */
    frame_ptr(): number;
    /**
     * Hire one more agent (#434): the site's install section calls this on a
     * Copy click, and a new coworker walks into the background office, works
     * a few spells, and heads out ~70s later. No-op before the first `step`
     * (no clock yet) and while `MAX_LIVE_HIRES` hires are already alive
     * (click-spam can't crowd out the cast). Never throws.
     */
    hire(): void;
    /**
     * Build an office seeded with `seed` (drives the layout variant). Errors
     * only if the compile-time-embedded sprite pack fails to parse (a build
     * bug), surfaced to JS as an exception.
     */
    constructor(seed: number);
    /**
     * Advance to `now_ms` and render at `w`×`h` pixels into the RGBA staging
     * buffer.
     *
     * CONTRACT: `now_ms` must be UNIX-epoch milliseconds — `Date.now()`, NOT
     * `performance.now()` and NOT a `requestAnimationFrame` timestamp (both
     * are ms-since-page-load: motion still animates, but the office's
     * day/night cycle and wall clock decode `now` as calendar time, so a
     * page-relative clock pins the scene at 1970 — permanently 00:00,
     * defeating the browser-timezone support entirely).
     */
    step(now_ms: number, w: number, h: number): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_office_free: (a: number, b: number) => void;
    readonly office_frame_len: (a: number) => number;
    readonly office_frame_ptr: (a: number) => number;
    readonly office_hire: (a: number) => void;
    readonly office_new: (a: number) => [number, number, number];
    readonly office_step: (a: number, b: number, c: number, d: number) => void;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
