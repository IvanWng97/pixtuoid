/* tslint:disable */
/* eslint-disable */

/**
 * A live office rendered to a reusable RGBA buffer across frames. Owns a
 * `FloorSession` (the scene-owned painter session: per-floor render caches +
 * persistent office coffee/chitchat + the dual eviction) so keeping ONE
 * handle alive across `step` calls is what keeps motion/pose continuous
 * (no walk-flash) — same contract as `OfficeRenderer`.
 */
export class Office {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Stage a handoff for the worker's spawn-time track (`night` + 10-min
     * `epoch` block). A stale epoch at click time self-heals through the
     * normal chunked swap. Overwrites any prior stage.
     */
    audio_adopt_begin(night: boolean, epoch: number): void;
    /**
     * Promote a COMPLETE handoff to the live driver. `true` = the ♩ click is
     * now upload-only. Refuses a torn handoff, and refuses to stomp a driver
     * a click already warmed (first ready wins).
     */
    audio_adopt_finish(): boolean;
    /**
     * Copy in loop stem `idx` (`LoopStem::ALL` order: beds sequentially,
     * rain last). Same `false` contract as `audio_adopt_oneshot`.
     */
    audio_adopt_loop(idx: number, samples: Float32Array): boolean;
    /**
     * Copy in one one-shot buffer (the worker's pool-discovery order).
     * `false` = refused (no stage / bad pool / overflow) — JS abandons the
     * handoff and the click-time warmup takes over.
     */
    audio_adopt_oneshot(pool: number, samples: Float32Array): boolean;
    /**
     * Create the audio engine for the CURRENT day/night + weather (from the
     * last `step`'s clock). Idempotent — a second call is ignored, so JS can
     * call it freely on the ♩ click. Costs nothing until `audio_warmup_step`
     * synthesizes the beds.
     */
    audio_begin(): void;
    audio_loop_len(idx: number): number;
    /**
     * Zero-copy pointer/length into the looping bed samples for stem `idx`
     * (0=Pad … 5=Rain). RE-READ after warmup completes AND whenever a tick
     * reports `swapped` (a track swap / any `memory.grow` moves the data).
     */
    audio_loop_ptr(idx: number): number;
    audio_oneshot_len(pool: number, idx: number): number;
    /**
     * Zero-copy pointer/length into a one-shot buffer: `pool` is the wire index
     * (0=keystroke, 1=raindrop, 2=door chime, 3=printer, 4=vending), `idx` the
     * pool slot (keystrokes/drops are pools; the appliance cues are single).
     * Uploaded once after warmup.
     */
    audio_oneshot_ptr(pool: number, idx: number): number;
    /**
     * The engine's sample rate (Hz) — JS builds its `AudioBuffer`s at this rate
     * (the browser resamples to the AudioContext rate).
     */
    audio_sample_rate(): number;
    /**
     * Advance the audio one tick at `now_ms` (the site's pause-shifted clock,
     * same as `step`) and return the JS glue commands as JSON:
     * `{"gains":[g0..g5],"plays":[[poolWire,idx,gain],…],"swapped":bool}`.
     * `gains` are the 6 loop-stem target amplitudes (JS ramps each GainNode);
     * `plays` are one-shots to spawn; `swapped` = re-read the loop buffers.
     * Empty-ish before the beds are ready. No serde (tiny hand-built payload,
     * like `overlay_json`).
     */
    audio_tick(now_ms: number): string;
    /**
     * Build ONE synthesis piece; returns pieces REMAINING (0 = ready to
     * upload buffers + tick). JS loops it off `setTimeout(0)` so the multi-
     * second synthesis never blocks the main thread in one shot. 0 if audio
     * hasn't begun.
     */
    audio_warmup_step(): number;
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
     * a few spells, and heads out ~70s later. Returns whether the hire was
     * admitted (`true`) or refused (`false`) — refused before the first `step`
     * (no clock yet), while `MAX_LIVE` hires are already alive (click-spam
     * can't crowd out the cast), and when the canvas-sized office has no free
     * desk to seat one. The caller (the site's install-copy chain) answers its
     * receipt event from this return, not a JS-side mirror of the cap. Never
     * throws.
     */
    hire(): boolean;
    /**
     * Whether the office's sky shows the SUN at hour-of-day `hour` (0..24). The
     * site's VIBING sky-slider reads this to draw its thumb as a sun by day /
     * moon by night, so the control can't drift from the office it previews —
     * it delegates to the engine's ONE day/night boundary (`SUN_RISE_H`/
     * `SUN_SET_H`, `pixtuoid_scene`'s `sky::hour_is_day`). Pure in `hour`; the
     * `&self` receiver keeps it a JS method on the office handle JS already holds.
     */
    is_day(hour: number): boolean;
    /**
     * Build an office seeded with `seed` (drives the layout variant). Errors
     * only if the compile-time-embedded sprite pack fails to parse (a build
     * bug), surfaced to JS as an exception.
     */
    constructor(seed: number);
    /**
     * Export the current frame's name-badge labels + neon wall-board TEXT as a
     * small JSON string for the site's DOM overlay (`OfficeBackdrop.astro`).
     *
     * The wasm office renders at a SMALL buffer that CSS upscales with
     * `image-rendering: pixelated`, so anti-aliased text CANNOT be baked into the
     * pixels (it would nearest-neighbor blow up blocky). Instead the site lays
     * crisp Monaspace Neon DOM spans over the canvas from this model. Coordinates
     * are OFFICE-BUFFER px (a label's `x` is the sprite CENTER, `y` its head-top;
     * the board `rect` is the neon-panel interior) — the site scales them to the
     * CSS-displayed canvas. Colors are RESOLVED against the CURRENT theme, so a
     * `set_theme` reflects with no extra call. Call right after `step` (it reads
     * the step's clock). No serde — the payload is tiny and hand-built (escaped);
     * the site wraps `JSON.parse` in try/catch so a bad frame degrades to no overlay.
     */
    overlay_json(): string;
    /**
     * Recolor the whole office to a theme by name (`"normal"|"cyberpunk"|
     * "dracula"|"tokyo-night"|"catppuccin"|"gruvbox"`). Unknown name = no-op.
     * Flushes the recolor cache so agent sprites repaint on the next frame; the
     * env recolors on its own (painted fresh each frame from `self.theme`).
     */
    set_theme(name: string): void;
    /**
     * Force the office's weather (`"clear"|"rain"|"storm"|"snow"|"fog"|
     * "overcast"|"windy"|"smog"`), or `None` to follow the clock-based cycle.
     * Applied each `step` (see the force_weather invariant) so two Offices sharing
     * the one wasm module never fight over the thread-local override.
     */
    set_weather(name?: string | null): void;
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

/**
 * The worker-side synthesizer (#705): the audio-prewarm Web Worker
 * instantiates its OWN wasm module (memories can't be shared), pumps
 * [`SynthTake::step`] to 0 — blocking is fine off the main thread — then
 * copies each buffer out through the ptr/len getters (the driver's read
 * contract) and transfers them to the main thread for `Office::audio_adopt_*`.
 */
export class SynthTake {
    free(): void;
    [Symbol.dispose](): void;
    epoch(): number;
    /**
     * The loop-stem count — the worker's copy-out bound, read from the ONE
     * authority (`LoopStem::ALL`) instead of a JS-side literal. Loops aren't
     * self-terminating like the one-shot pools, so JS can't discover it.
     */
    loop_count(): number;
    loop_len(idx: number): number;
    /**
     * Zero-copy reads, same contract as the `Office::audio_*` getters — the
     * worker copies (`Float32Array.slice`) before its next wasm call.
     */
    loop_ptr(idx: number): number;
    /**
     * `now_ms` = UNIX-epoch milliseconds (the `Office::step` contract) —
     * selects the same day/night + weather track the office would at that
     * instant (procedural weather; a `weather_override` mismatch on the main
     * office self-heals through the normal swap).
     */
    constructor(now_ms: number);
    /**
     * The selected track, split for the adopt wire (`audio_adopt_begin`).
     */
    night(): boolean;
    oneshot_len(pool: number, idx: number): number;
    oneshot_ptr(pool: number, idx: number): number;
    /**
     * Build ONE synthesis piece; pieces remaining (0 = done).
     */
    step(): number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_office_free: (a: number, b: number) => void;
    readonly __wbg_synthtake_free: (a: number, b: number) => void;
    readonly office_audio_adopt_begin: (a: number, b: number, c: number) => void;
    readonly office_audio_adopt_finish: (a: number) => number;
    readonly office_audio_adopt_loop: (a: number, b: number, c: number, d: number) => number;
    readonly office_audio_adopt_oneshot: (a: number, b: number, c: number, d: number) => number;
    readonly office_audio_begin: (a: number) => void;
    readonly office_audio_loop_len: (a: number, b: number) => number;
    readonly office_audio_loop_ptr: (a: number, b: number) => number;
    readonly office_audio_oneshot_len: (a: number, b: number, c: number) => number;
    readonly office_audio_oneshot_ptr: (a: number, b: number, c: number) => number;
    readonly office_audio_sample_rate: (a: number) => number;
    readonly office_audio_tick: (a: number, b: number) => [number, number];
    readonly office_audio_warmup_step: (a: number) => number;
    readonly office_frame_len: (a: number) => number;
    readonly office_frame_ptr: (a: number) => number;
    readonly office_hire: (a: number) => number;
    readonly office_is_day: (a: number, b: number) => number;
    readonly office_new: (a: number) => [number, number, number];
    readonly office_overlay_json: (a: number) => [number, number];
    readonly office_set_theme: (a: number, b: number, c: number) => void;
    readonly office_set_weather: (a: number, b: number, c: number) => void;
    readonly office_step: (a: number, b: number, c: number, d: number) => void;
    readonly synthtake_epoch: (a: number) => number;
    readonly synthtake_loop_count: (a: number) => number;
    readonly synthtake_loop_len: (a: number, b: number) => number;
    readonly synthtake_loop_ptr: (a: number, b: number) => number;
    readonly synthtake_new: (a: number) => number;
    readonly synthtake_night: (a: number) => number;
    readonly synthtake_oneshot_len: (a: number, b: number, c: number) => number;
    readonly synthtake_oneshot_ptr: (a: number, b: number, c: number) => number;
    readonly synthtake_step: (a: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
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
