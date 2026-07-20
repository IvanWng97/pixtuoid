/* @ts-self-types="./pixtuoid_web.d.ts" */

/**
 * A live office rendered to a reusable RGBA buffer across frames. Owns a
 * `FloorSession` (the scene-owned painter session: per-floor render caches +
 * persistent office coffee/chitchat + the dual eviction) so keeping ONE
 * handle alive across `step` calls is what keeps motion/pose continuous
 * (no walk-flash) — same contract as `OfficeRenderer`.
 */
export class Office {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        OfficeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_office_free(ptr, 0);
    }
    /**
     * Stage a handoff for the worker's spawn-time track (`night` + 10-min
     * `epoch` block). A stale epoch at click time self-heals through the
     * normal chunked swap. Overwrites any prior stage.
     * @param {boolean} night
     * @param {number} epoch
     */
    audio_adopt_begin(night, epoch) {
        wasm.office_audio_adopt_begin(this.__wbg_ptr, night, epoch);
    }
    /**
     * Promote a COMPLETE handoff to the live driver. `true` = the ♩ click is
     * now upload-only. Refuses a torn handoff, and refuses to stomp a driver
     * a click already warmed (first ready wins).
     * @returns {boolean}
     */
    audio_adopt_finish() {
        const ret = wasm.office_audio_adopt_finish(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Copy in loop stem `idx` (`LoopStem::ALL` order: beds sequentially,
     * rain last). Same `false` contract as `audio_adopt_oneshot`.
     * @param {number} idx
     * @param {Float32Array} samples
     * @returns {boolean}
     */
    audio_adopt_loop(idx, samples) {
        const ptr0 = passArrayF32ToWasm0(samples, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.office_audio_adopt_loop(this.__wbg_ptr, idx, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Copy in one one-shot buffer (the worker's pool-discovery order).
     * `false` = refused (no stage / bad pool / overflow) — JS abandons the
     * handoff and the click-time warmup takes over.
     * @param {number} pool
     * @param {Float32Array} samples
     * @returns {boolean}
     */
    audio_adopt_oneshot(pool, samples) {
        const ptr0 = passArrayF32ToWasm0(samples, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.office_audio_adopt_oneshot(this.__wbg_ptr, pool, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Create the audio engine for the CURRENT day/night + weather (from the
     * last `step`'s clock). Idempotent — a second call is ignored, so JS can
     * call it freely on the ♩ click. Costs nothing until `audio_warmup_step`
     * synthesizes the beds.
     */
    audio_begin() {
        wasm.office_audio_begin(this.__wbg_ptr);
    }
    /**
     * @param {number} idx
     * @returns {number}
     */
    audio_loop_len(idx) {
        const ret = wasm.office_audio_loop_len(this.__wbg_ptr, idx);
        return ret >>> 0;
    }
    /**
     * Zero-copy pointer/length into the looping bed samples for stem `idx`
     * (0=Pad … 5=Rain). RE-READ after warmup completes AND whenever a tick
     * reports `swapped` (a track swap / any `memory.grow` moves the data).
     * @param {number} idx
     * @returns {number}
     */
    audio_loop_ptr(idx) {
        const ret = wasm.office_audio_loop_ptr(this.__wbg_ptr, idx);
        return ret >>> 0;
    }
    /**
     * @param {number} pool
     * @param {number} idx
     * @returns {number}
     */
    audio_oneshot_len(pool, idx) {
        const ret = wasm.office_audio_oneshot_len(this.__wbg_ptr, pool, idx);
        return ret >>> 0;
    }
    /**
     * Zero-copy pointer/length into a one-shot buffer: `pool` is the wire index
     * (0=keystroke, 1=raindrop, 2=door chime, 3=printer, 4=vending), `idx` the
     * pool slot (keystrokes/drops are pools; the appliance cues are single).
     * Uploaded once after warmup.
     * @param {number} pool
     * @param {number} idx
     * @returns {number}
     */
    audio_oneshot_ptr(pool, idx) {
        const ret = wasm.office_audio_oneshot_ptr(this.__wbg_ptr, pool, idx);
        return ret >>> 0;
    }
    /**
     * The engine's sample rate (Hz) — JS builds its `AudioBuffer`s at this rate
     * (the browser resamples to the AudioContext rate).
     * @returns {number}
     */
    audio_sample_rate() {
        const ret = wasm.office_audio_sample_rate(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Advance the audio one tick at `now_ms` (the site's pause-shifted clock,
     * same as `step`) and return the JS glue commands as JSON:
     * `{"gains":[g0..g5],"plays":[[poolWire,idx,gain],…],"swapped":bool}`.
     * `gains` are the 6 loop-stem target amplitudes (JS ramps each GainNode);
     * `plays` are one-shots to spawn; `swapped` = re-read the loop buffers.
     * Empty-ish before the beds are ready. No serde (tiny hand-built payload,
     * like `overlay_json`).
     * @param {number} now_ms
     * @returns {string}
     */
    audio_tick(now_ms) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.office_audio_tick(this.__wbg_ptr, now_ms);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Build ONE synthesis piece; returns pieces REMAINING (0 = ready to
     * upload buffers + tick). JS loops it off `setTimeout(0)` so the multi-
     * second synthesis never blocks the main thread in one shot. 0 if audio
     * hasn't begun.
     * @returns {number}
     */
    audio_warmup_step() {
        const ret = wasm.office_audio_warmup_step(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Byte length of the RGBA frame (`w*h*4`).
     * @returns {number}
     */
    frame_len() {
        const ret = wasm.office_frame_len(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Pointer to the RGBA frame in wasm linear memory (`w*h*4` bytes).
     *
     * CONTRACT: re-read this (and rebuild any `Uint8ClampedArray` view) after
     * EVERY `step` — a canvas resize reallocates the staging buffer (the
     * pointer moves), and any wasm `memory.grow` invalidates existing JS
     * views into linear memory even when the pointer value is unchanged.
     * @returns {number}
     */
    frame_ptr() {
        const ret = wasm.office_frame_ptr(this.__wbg_ptr);
        return ret >>> 0;
    }
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
     * @returns {boolean}
     */
    hire() {
        const ret = wasm.office_hire(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Whether the office's sky shows the SUN at hour-of-day `hour` (0..24). The
     * site's VIBING sky-slider reads this to draw its thumb as a sun by day /
     * moon by night, so the control can't drift from the office it previews —
     * it delegates to the engine's ONE day/night boundary (`SUN_RISE_H`/
     * `SUN_SET_H`, `pixtuoid_scene`'s `sky::hour_is_day`). Pure in `hour`; the
     * `&self` receiver keeps it a JS method on the office handle JS already holds.
     * @param {number} hour
     * @returns {boolean}
     */
    is_day(hour) {
        const ret = wasm.office_is_day(this.__wbg_ptr, hour);
        return ret !== 0;
    }
    /**
     * Build an office seeded with `seed` (drives the layout variant). Errors
     * only if the compile-time-embedded sprite pack fails to parse (a build
     * bug), surfaced to JS as an exception.
     * @param {number} seed
     */
    constructor(seed) {
        const ret = wasm.office_new(seed);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        OfficeFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
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
     * @returns {string}
     */
    overlay_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.office_overlay_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Recolor the whole office to a theme by name (`"normal"|"cyberpunk"|
     * "dracula"|"tokyo-night"|"catppuccin"|"gruvbox"`). Unknown name = no-op.
     * Flushes the recolor cache so agent sprites repaint on the next frame; the
     * env recolors on its own (painted fresh each frame from `self.theme`).
     * @param {string} name
     */
    set_theme(name) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.office_set_theme(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Force the office's weather (`"clear"|"rain"|"storm"|"snow"|"fog"|
     * "overcast"|"windy"|"smog"`), or `None` to follow the clock-based cycle.
     * Applied each `step` (see the force_weather invariant) so two Offices sharing
     * the one wasm module never fight over the thread-local override.
     * @param {string | null} [name]
     */
    set_weather(name) {
        var ptr0 = isLikeNone(name) ? 0 : passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        wasm.office_set_weather(this.__wbg_ptr, ptr0, len0);
    }
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
     * @param {number} now_ms
     * @param {number} w
     * @param {number} h
     */
    step(now_ms, w, h) {
        wasm.office_step(this.__wbg_ptr, now_ms, w, h);
    }
}
if (Symbol.dispose) Office.prototype[Symbol.dispose] = Office.prototype.free;

/**
 * The worker-side synthesizer (#705): the audio-prewarm Web Worker
 * instantiates its OWN wasm module (memories can't be shared), pumps
 * [`SynthTake::step`] to 0 — blocking is fine off the main thread — then
 * copies each buffer out through the ptr/len getters (the driver's read
 * contract) and transfers them to the main thread for `Office::audio_adopt_*`.
 */
export class SynthTake {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SynthTakeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_synthtake_free(ptr, 0);
    }
    /**
     * @returns {number}
     */
    epoch() {
        const ret = wasm.synthtake_epoch(this.__wbg_ptr);
        return ret;
    }
    /**
     * The loop-stem count — the worker's copy-out bound, read from the ONE
     * authority (`LoopStem::ALL`) instead of a JS-side literal. Loops aren't
     * self-terminating like the one-shot pools, so JS can't discover it.
     * @returns {number}
     */
    loop_count() {
        const ret = wasm.synthtake_loop_count(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @param {number} idx
     * @returns {number}
     */
    loop_len(idx) {
        const ret = wasm.synthtake_loop_len(this.__wbg_ptr, idx);
        return ret >>> 0;
    }
    /**
     * Zero-copy reads, same contract as the `Office::audio_*` getters — the
     * worker copies (`Float32Array.slice`) before its next wasm call.
     * @param {number} idx
     * @returns {number}
     */
    loop_ptr(idx) {
        const ret = wasm.synthtake_loop_ptr(this.__wbg_ptr, idx);
        return ret >>> 0;
    }
    /**
     * `now_ms` = UNIX-epoch milliseconds (the `Office::step` contract) —
     * selects the same day/night + weather track the office would at that
     * instant (procedural weather; a `weather_override` mismatch on the main
     * office self-heals through the normal swap).
     * @param {number} now_ms
     */
    constructor(now_ms) {
        const ret = wasm.synthtake_new(now_ms);
        this.__wbg_ptr = ret;
        SynthTakeFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * The selected track, split for the adopt wire (`audio_adopt_begin`).
     * @returns {boolean}
     */
    night() {
        const ret = wasm.synthtake_night(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @param {number} pool
     * @param {number} idx
     * @returns {number}
     */
    oneshot_len(pool, idx) {
        const ret = wasm.synthtake_oneshot_len(this.__wbg_ptr, pool, idx);
        return ret >>> 0;
    }
    /**
     * @param {number} pool
     * @param {number} idx
     * @returns {number}
     */
    oneshot_ptr(pool, idx) {
        const ret = wasm.synthtake_oneshot_ptr(this.__wbg_ptr, pool, idx);
        return ret >>> 0;
    }
    /**
     * Build ONE synthesis piece; pieces remaining (0 = done).
     * @returns {number}
     */
    step() {
        const ret = wasm.synthtake_step(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) SynthTake.prototype[Symbol.dispose] = SynthTake.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_92b29b0548f8b746: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg___wbindgen_throw_344f42d3211c4765: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_getTimezoneOffset_dc9862c79e5a81a3: function(arg0) {
            const ret = arg0.getTimezoneOffset();
            return ret;
        },
        __wbg_new_cc984128914cfc6f: function(arg0) {
            const ret = new Date(arg0);
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./pixtuoid_web_bg.js": import0,
    };
}

const OfficeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_office_free(ptr, 1));
const SynthTakeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_synthtake_free(ptr, 1));

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedFloat32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('pixtuoid_web_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
