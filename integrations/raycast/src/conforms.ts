import Ajv2020, { type AnySchema } from "ajv/dist/2020.js";

/** A committed `contract/*.schema.json` (draft 2020-12). */
export type Schema = AnySchema;

// A Tolerant Reader: `removeAdditional: true` drops only the properties an
// `additionalProperties: false` forbids (ajv docs/options.md), so a newer CLI's
// added field is ignored rather than failing an installed copy.
const ajv = new Ajv2020({ removeAdditional: true });

/** Whether `value` is an instance of `schema`. A schema ajv can't compile, such
 *  as one with an unresolvable `$ref`, fails closed. */
export function conforms(value: unknown, schema: Schema): boolean {
  try {
    return ajv.validate(schema, value) === true;
  } catch {
    return false;
  }
}
