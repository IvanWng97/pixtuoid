/** The JSON Schema keywords the committed `contract/*.schema.json` use. */
export interface Schema {
  type?: string | string[];
  enum?: unknown[];
  $ref?: string;
  properties?: Record<string, Schema>;
  required?: string[];
  additionalProperties?: boolean;
  $defs?: Record<string, Schema>;
}

function typeOf(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (Number.isInteger(value)) return "integer";
  return typeof value;
}

/** Whether `value` is an instance of `schema`, resolving `#/$defs/…` refs
 *  against `root`: the subset of JSON Schema the contract schemas use. A `$ref`
 *  or keyword outside it fails closed. */
export function conforms(value: unknown, schema: Schema, root: Schema = schema): boolean {
  if (schema.$ref !== undefined) {
    const def = /^#\/\$defs\/(.+)$/.exec(schema.$ref)?.[1];
    const target = def === undefined ? undefined : root.$defs?.[def];
    return target !== undefined && conforms(value, target, root);
  }
  if (schema.type !== undefined) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    const got = typeOf(value);
    if (!types.some((t) => t === got || (t === "number" && got === "integer"))) return false;
  }
  if (schema.enum !== undefined && !schema.enum.includes(value)) return false;
  if (schema.properties !== undefined || schema.required !== undefined) {
    if (typeOf(value) !== "object") return false;
    const record = value as Record<string, unknown>;
    const properties = schema.properties ?? {};
    if (!(schema.required ?? []).every((key) => key in record)) return false;
    for (const [key, field] of Object.entries(record)) {
      const fieldSchema = properties[key];
      if (fieldSchema === undefined) {
        if (schema.additionalProperties === false) return false;
      } else if (!conforms(field, fieldSchema, root)) {
        return false;
      }
    }
  }
  return true;
}
