// Astro 7's 'jsx' default drops the space between adjacent inline elements on
// separate source lines, joining visible text ("pixtuoid v0.11.1" →
// "pixtuoidv0.11.1"). Pin the Astro 6 behavior. Prettier reads it too: the
// formatter must know which whitespace the compiler collapses, or a reformat
// changes what renders.
export const COMPRESS_HTML = true;
