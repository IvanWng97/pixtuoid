---
name: beautify-decoration
version: 1.0.0
description: "Iterate on the visual identity of a top-down pixel-art decoration (sprite + layout integration) in pixtuoid. Use when redesigning an existing decoration (pantry, lounge, meeting room, cubicle decor) or adding a new one. Captures the rebuild trap, the visual-verification loop, resolution constraints, sprite-format pitfalls, and the layout-integration checklist that we learned the hard way during the pantry beautify session."
metadata:
  scope: "pixtuoid repo only"
---

# beautify-decoration (v1)

A repo-specific iteration loop for visually redesigning a decoration in `pixtuoid`. Follow this when the user says "beautify X" or "make Y look better" — it short-circuits several rebuild traps and visual-design dead ends that aren't obvious from the codebase alone.

## When to use

- Redesigning an existing decoration sprite (pantry, lounge, meeting, cubicle decor)
- Adding a new fixture (pendant lamp, water cooler, chalkboard, etc.)
- User says "items look too small / don't read like X / blend together"
- After making sprite edits and "I don't see any change"

## The visual-iteration loop

```
1. Edit sprite OR layout
   ↓
2. cargo build --release --example snapshot
   ↓
3. ./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
   ↓
4. .venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3 -q <quadrant>
   (or skip the quadrant guessing: snapshot --crop-furniture pantry|couch|vending|
   printer|meeting|sofa|desk OR --crop-agent <label> renders a 40x24-cell window
   already centered on the target — no Python step)
   ↓
5. Read the cropped PNG → self-critique → back to step 1
   ↓
6. When happy, send to user with SendUserFile and short caption
   ↓
7. cargo build --release --workspace    ← rebuild the LIVE binary too
   ↓
8. Commit with iteration history (which designs were tried, why rejected)
```

The user is the final judge of "does it look like a fridge / coffee machine / etc." — but you should self-critique before sending. Three iterations of self-critique before bothering the user.

**Step 7 is mandatory.** `cargo build --release --example snapshot` does NOT rebuild the main binary. Users testing with `./target/release/pixtuoid run` won't see sprite changes until the workspace is rebuilt. Forgetting this step is how "I changed the sprite but nothing happened in the live TUI" bugs get filed.

**Step 8 is mandatory.** Commit messages for sprite changes must include the iteration count and a one-line rationale for each rejected attempt. Future editors need to know which alternatives were explored — otherwise they'll re-try the same dead-end designs (the seated_sleeping sprite went through 4 iterations before reading correctly at scale).

## Sharp edges (the things that wasted time during the pantry session)

### 1. The rebuild trap

- `cargo build --release --workspace` **does not** rebuild examples. Use `cargo build --release --example snapshot` when iterating on `examples/snapshot`.
- `include_str!` in `crates/pixtuoid-scene/src/embedded_pack.rs` bakes sprite files at compile time. A `build.rs` exists at `crates/pixtuoid-scene/build.rs` that emits `rerun-if-changed` for every `.sprite` and `pack.toml` — so a sprite edit DOES trigger a rebuild now. If you added a new asset and edits still aren't being picked up, check that build.rs is matching its extension.
- If unsure, verify with: `strings target/release/examples/snapshot | grep "<some unique string from your sprite>"`.

### 2. Snapshot defaults hide the large sprite variants

`examples/snapshot` defaults to 192×80 cells → buffer 192×160. Several layouts (pantry, corridor appliances) have conditional variants based on room dimensions. Corridor items (vending machine, printer) only appear when `walkway_h ≥ 9–10`. **Use the default `--cols 192 --rows 80` to see everything.**

Pantry-specific threshold: `pantry_room.width >= 36` triggers the 32×10 sprite; below that, the 20×8 `pantry_small.sprite` is used. Threshold lives in `crates/pixtuoid-scene/src/layout/compute.rs`.

### 3. Visual-inspection helper

The full PNG is too big to grok at a glance and too small at thumbnail. Crop the relevant quadrant with PIL:

```python
from PIL import Image
img = Image.open('/tmp/snap.png')
w, h = img.size
# Pantry is bottom-left quadrant; adjust ratios for other zones:
#   meeting:  (0, 0, 0.30*w, 0.45*h)
#   pantry:   (0, 0.49*h, 0.30*w, h)
#   cubicle:  (0.30*w, 0, w, 0.55*h)
#   lounge:   pre-2026 retired; merged into cubicle band
crop = img.crop((0, int(h*0.49), int(w*0.30), h))
crop = crop.resize((crop.width*2, crop.height*2), Image.NEAREST)
crop.save('/tmp/crop.png')
```

Then `Read` the cropped PNG — you (Claude) can see PNG content via the Read tool.

PIL is available system-wide (installed via `pip3 install --user --break-system-packages Pillow`). If a fresh environment misses it, install once.

### 4. Resolution budget

- Each sprite pixel ≈ half a terminal cell (half-block compression).
- Subzones smaller than **~5 display cells wide** blur into pixel noise — users can't read them.
- Sub-pixel detail (a 1-cell handle, a 1-cell stripe) is invisible. Iterate on **silhouette + color identity**, not pixel polish.
- A 32×10 sprite has only ~16 display cells of width. Three zones of ~5 cells each is the practical max for legibility. Drop items; don't shrink them.

### 5. Identity mistakes that look identical to each other

Symptoms of weak identity:

- **Transparent body (`.`)**: the wall color shows through, weakening the silhouette. Use a solid fill color for appliances.
- **All-dark appliances**: a row of `M`-bodied items reads as "row of dark boxes." Give each appliance a distinct base color (e.g., `w` white fridge against `M` dark coffee machine + `M` dark microwave with `q` glass).
- **Symmetric H-frame on a white box** → reads as washing machine, not fridge. Use asymmetric handles (single-side handle, or center-French-door pair).
- **Cyan + blue dispenser dots next to each other** → reads as cyan-cyan because `b` is dark and gets dim. Space them out or use `c` + `r`.

### 6. Sprite-format pitfalls

- Every row in a `.sprite` file must have **exactly** the same number of space-separated cells. Off-by-one is the most common bug.
- Verify with: `awk '/^@/{next}/^#/{next}NF{print NR": "NF}' crates/pixtuoid-scene/sprites/default/foo.sprite` — all NF values must match.
- Or visualize packed rows: `awk '/^@/{next}/^#/{next}NF{for(i=1;i<=NF;i++)printf "%s",$i;print " ["NF"]"}' foo.sprite`.
- Palette keys must be unique RGB (the per-agent recolor pass substitutes by RGB equality — see `embedded_pack.rs` header comment).
- Reuse existing palette keys when possible; new keys go in `crates/pixtuoid-scene/sprites/default/pack.toml` `[palette]` section.

### 7. Layout integration checklist

When a sprite **changes size**:

1. Update the decoration's footprint in the `furniture_def(Furniture)` geometry table in `crates/pixtuoid-scene/src/layout/decor.rs` — the single source of truth for footprint + visual, read by `mask::build_walkable_mask` (waypoints via `approach::obstacle_footprint`), `approach.rs`, and the z-sort. Do NOT hardcode a `(w, h)` at the mask stamp site; it would diverge from the table that `stand_point`/approach and render-centering read.
2. A non-waypoint obstacle (plant, wall decor, pod decor) is likewise stamped from its `FurnitureDef` row via `furniture_def(kind.furniture()).footprint`, not an inline literal — so the same table edit covers it.
3. Run `cargo test -p pixtuoid-scene` — the `walkable_mask_is_fully_connected_across_buffer_sizes` test (lives in `layout/tests.rs`, moved with the layout cluster) catches mask/sprite mismatches by trying multiple buffer sizes and asserting BFS reach from the door.
4. If the connectivity test fails on the smallest buffer (96×70), the sprite is too big for that pantry. Add a `_small` variant + conditional pick (see `PantryRoom::counter_size` / `SceneLayout::pantry_counter_size()` for the pattern).
5. Update animation list in `crates/pixtuoid-scene/sprites/default/pack.toml` and `embedded_pack.rs` to include both `foo.sprite` and `foo_small.sprite` if you added a variant.

### 8. Live binary uses different binary than snapshot

`./target/release/pixtuoid run` uses the main binary. `examples/snapshot` uses its own binary. **Both** need `cargo build --release --example snapshot` (or `cargo build --release --workspace --example snapshot`) when iterating on snapshot — and `cargo build --release` is fine for the live TUI binary.

## Self-critique checklist — MANDATORY before every SendUserFile

You **must** run this checklist explicitly before each `SendUserFile` in a beautify loop. State the result of each row in the message (✅/⚠️/❌). Fix any ❌ before sending; if you ship a ⚠️, call it out so the user knows the trade-off.

| Check | What it means |
|---|---|
| Stranger-ID | If a stranger saw this with no context, would they identify each new element as the intended thing? Name each element explicitly. |
| Visually differs | Diff is noticeable, not a sub-pixel tweak. If hash-identical to last attempt, you didn't actually rebuild. |
| Subzone width | Each new sub-element ≥ 5 **display** cells wide (horizontal cells = buffer px; vertical cells = buffer px / 2 due to half-block). |
| Color distinctness | New elements use colors distinct from immediate neighbours. |
| `cargo test` | Connectivity test passes (`cargo test --workspace --features pixtuoid-core/test-renderer`). |
| `--debug-walkable` | Rendered the overlay and visually checked no narrow / isolated walkable pockets near the new element. |

Skipping this checklist defeats the point of the skill — the whole reason it exists is that past sessions shipped invisible / unverified changes.

## Workflow when adding a NEW decoration

1. Sketch the design as a list of cells per row (count exactly).
2. Pick a palette: reuse `pack.toml` keys; only add new ones if necessary.
3. Write the `.sprite` file; verify row widths with the awk command above.
4. Add the include_str! line to `embedded_pack.rs`.
5. Add the `[animations.foo]` block to `pack.toml`.
6. Decide where it lives in the layout — add a `Point` placement in `SceneLayout::compute`.
7. Add the obstacle footprint to `build_walkable_mask` (or a waypoint kind if it's interactive).
8. Add a `DrawableKind::Foo` variant + `paint_drawable` arm if z-sorting matters.
9. Run `cargo test -p pixtuoid-scene` — the layout/walkable-connectivity and painter tests this checklist relies on live there since the scene split (`-p pixtuoid-core` no longer runs any of them).
10. Snapshot + iterate.

## Recap of the pantry session (case study)

What we did: replaced the 20×8 pantry counter with a 32×10 design through 8 iterations:

- **v1–v3**: Too crowded, 6 zones × 3 cells each = unreadable.
- **v4**: Simplified to 3 zones (fridge / coffee / microwave-snacks) at 8/10/10 cells.
- **v5–v6**: Tried adding detail (handle pairs, dividers). User said "no difference between v5/v6" — too subtle to read at scale.
- **v7**: Discovered `cargo build --workspace` was not rebuilding the snapshot example, so v6 was never actually rendered. Fixed by adding build.rs.
- **v8**: Color-coded for identity — solid WHITE fridge vs. dark coffee + dark microwave. Strong silhouette differentiation. (Honest self-critique: still looks washing-machine-y due to H-frame.)

Lessons: **silhouette + color over detail**, **always rebuild the example explicitly**, **bump cols to 192 for the large variant**.
