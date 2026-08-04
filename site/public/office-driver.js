// The shared wasm-`Office` driver for OfficeBackdrop and Showcase's VIBING channel.
// Both are `is:inline` scripts (CSP-hashed), which the bundler doesn't process, so
// they can't static-import a shared `src/` module — they dynamic-`import()` this
// public ES module at boot instead.
'use strict';

const WASM_INIT_RETRIES = 2; // extra attempts after the first
const WASM_INIT_BACKOFF_MS = 400;

// The ONE memoized wasm-glue init both live-office consumers share. The generated
// glue's `__wbg_init` memoizes only the RESOLVED instance, not an in-flight promise,
// so two boots racing before either resolves would instantiate two wasm instances
// and stomp the module-global `wasm` — whichever Office built against the loser then
// reads the wrong linear memory forever. The bounded retry MUST stay INSIDE this one
// shared promise: a per-consumer retry would reintroduce that race.
export function sharedWasm(wasmJsUrl) {
  if (window.__pixWasm) return window.__pixWasm;
  function attempt(left) {
    return import(wasmJsUrl)
      .then(function (m) {
        return m.default().then(function () {
          return m;
        });
      })
      .catch(function (err) {
        if (left <= 0) throw err;
        return new Promise(function (res) {
          setTimeout(res, WASM_INIT_BACKOFF_MS);
        }).then(function () {
          return attempt(left - 1);
        });
      });
  }
  window.__pixWasm = attempt(WASM_INIT_RETRIES);
  return window.__pixWasm;
}

// A fresh view over the office's RGBA frame buffer. Re-read ptr AND len on EVERY
// frame — a resize / `memory.grow` reallocates and invalidates any retained view.
export function officeFrameView(office, wasm) {
  return new Uint8ClampedArray(wasm.memory.buffer, office.frame_ptr(), office.frame_len());
}
