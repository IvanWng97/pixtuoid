import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';

// `base` is relative to the site/ project root.
export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: 'CONFIGURATION.md', base: '../docs' }),
  }),
  // separate collections, not one multi-pattern glob, so each page grabs its
  // single entry without an order/`.find` dependency
  architecture: defineCollection({
    loader: glob({ pattern: 'ARCHITECTURE.md', base: '../docs' }),
  }),
  contributing: defineCollection({
    loader: glob({ pattern: 'CONTRIBUTING.md', base: '../docs' }),
  }),
  parallelDelivery: defineCollection({
    loader: glob({ pattern: 'PARALLEL-DELIVERY.md', base: '../docs' }),
  }),
};
