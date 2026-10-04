import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { conforms } from "../src/conforms.ts";

const schema = (name) => JSON.parse(readFileSync(new URL(`../contract/${name}.schema.json`, import.meta.url), "utf8"));
const sourceStatus = schema("source-status");
const outcomeRow = schema("outcome-row");

test("a source row conforms, its optional health null or absent", () => {
  const row = { id: "codex", display_name: "Codex", connected: true, cli_present: false };
  assert.ok(conforms(row, sourceStatus));
  assert.ok(conforms({ ...row, health: null }, sourceStatus));
  assert.ok(conforms({ ...row, health: "decode drift" }, sourceStatus));
});

test("a source row missing a field, mistyped, or carrying an unknown one does not", () => {
  const row = { id: "codex", display_name: "Codex", connected: true, cli_present: false };
  const missing = { ...row };
  delete missing.display_name;
  assert.ok(!conforms(missing, sourceStatus));
  assert.ok(!conforms({ ...row, connected: "yes" }, sourceStatus));
  assert.ok(!conforms({ ...row, health: 3 }, sourceStatus));
  assert.ok(!conforms({ ...row, extra: 1 }, sourceStatus));
  assert.ok(!conforms(null, sourceStatus));
  assert.ok(!conforms([row], sourceStatus));
});

test("an outcome row's token is one the schema's enum names", () => {
  assert.ok(conforms({ id: "codex", outcome: "failed", message: "denied" }, outcomeRow));
  assert.ok(conforms({ id: "codex", outcome: "connected" }, outcomeRow));
  assert.ok(!conforms({ id: "codex", outcome: "exploded" }, outcomeRow));
  assert.ok(!conforms({ id: "codex" }, outcomeRow));
});

test("a ref outside #/$defs fails closed", () => {
  assert.ok(!conforms("x", { $ref: "https://example.com/s.json" }));
  assert.ok(!conforms("x", { $ref: "#/$defs/Missing" }));
});
