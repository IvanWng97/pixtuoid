import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';

// Single-source the Configuration page from the repo's docs/CONFIGURATION.md —
// no copy, no drift. `base` is relative to the site/ project root.
export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: 'CONFIGURATION.md', base: '../docs' }),
  }),
};
