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
     * a few spells, and heads out ~70s later. No-op before the first `step`
     * (no clock yet), while `MAX_LIVE_HIRES` hires are already alive
     * (click-spam can't crowd out the cast), and when the canvas-sized
     * office has no free desk to seat one. Never throws.
     */
    hire() {
        wasm.office_hire(this.__wbg_ptr);
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
