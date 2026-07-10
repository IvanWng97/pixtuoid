# Bundled faces (binary-side AA text)

- `JetBrainsMono-Regular.ttf` — the primary face for every AA text surface
  (floating badges/board, snapshot cell text + `--proof`). License: `OFL.txt`.
- `PixtuoidSymbols.ttf` — the symbol FALLBACK face: JetBrains Mono has no
  glyph for parts of the office vocabulary (`★ ◐ ⬢ ▮ ▯ ⏱ ↳`), so
  `aa_text` falls back here per-character. It is a block-level subset of
  **JuliaMono-Regular v0.059** (U+2190–21FF, U+2300–23FF, U+25A0–25FF,
  U+2600–26FF, U+2700–27BF, U+2B00–2B5F), renamed because the OFL grant
  reserves the name "JuliaMono" (modified versions must not use it).
  License: `OFL-JuliaMono.txt`. Regenerate with fontTools:
  `subset.Subsetter` over those ranges, then rename ONLY the font-name
  records (name IDs 1/3/4/6/16/17 → "Pixtuoid Symbols", the PostScript
  name space-free) — the copyright (ID 0) and license records stay
  VERBATIM, as the OFL requires.
