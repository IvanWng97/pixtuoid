// The shared wasm-`Office` driver for the two live-office hero consumers —
// OfficeBackdrop's page-wide backdrop and Showcase's VIBING channel (#721).
// Both are `is:inline` scripts (CSP-hashed), which the bundler doesn't process,
// so they can't static-import a shared `src/` module — they dynamic-`import()`
// this public ES module at boot instead (precedent: audio-worker.js, #705).
// This single-sources the ONE wasm-init promise + the frame-memory contract
// that were byte-identical copies across the two components.
//
// Both consumers boot the office OFF the render-critical path (OfficeBackdrop
// via load→requestIdleCallback, Showcase via IntersectionObserver in-view), so
// the extra same-origin import hop is not above-fold-critical; a transient
// failure to fetch this module degrades to the poster through the caller's
// existing `.catch`, exactly like a wasm-binary fetch failure.
'use strict';

const WASM_INIT_RETRIES = 2; // extra attempts after the first
const WASM_INIT_BACKOFF_MS = 400;

// The ONE memoized wasm-glue init both live-office consumers share. The
// generated glue's `__wbg_init` memoizes only the RESOLVED instance, not an
// in-flight promise, so two independent boots racing before either resolves
// would instantiate two wasm instances and stomp the module-global `wasm`,
// leaving whichever Office built against the loser reading the wrong linear
// memory forever — so the promise is memoized on `window.__pixWasm` and every
// consumer awaits the same instantiation. The bounded retry (a transient
// ~874KB binary-fetch drop self-heals rather than stranding the office on the
// poster) MUST stay INSIDE this one shared promise: a per-consumer retry would
// reintroduce the double-instantiate race. On final exhaustion the promise
// rejects (the deliberate no-wasm poster fallback); the caller nulls
// `window.__pixWasm` in its own `.catch` so a later-booting consumer can retry
// after a recovered outage (#671). Full rationale: site/CLAUDE.md VIBING entry.
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

// The frame-memory contract: a fresh view over the office's RGBA frame buffer.
// Re-read ptr AND len on EVERY frame — a resize / `memory.grow` reallocates and
// invalidates any retained view (the frame_ptr contract). The caller checks
// `view.length` against its ImageData before blitting (a resize can land
// between this read and the paint).
export function officeFrameView(office, wasm) {
  return new Uint8ClampedArray(wasm.memory.buffer, office.frame_ptr(), office.frame_len());
}
