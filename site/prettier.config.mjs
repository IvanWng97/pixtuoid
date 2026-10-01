// @ts-check
import { COMPRESS_HTML } from './config/compress-html.mjs';

/** @type {import('prettier').Config} */
export default {
  printWidth: 100,
  singleQuote: true,
  semi: true,
  trailingComma: 'es5',
  plugins: ['prettier-plugin-astro'],
  astroCompressHTML: COMPRESS_HTML,
  overrides: [
    {
      files: '*.astro',
      options: { parser: 'astro' },
    },
  ],
};
