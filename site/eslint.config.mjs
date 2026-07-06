// Flat config: JS + TypeScript + Astro, Prettier-compatible.
// Accessibility is enforced at runtime against the built pages by Lighthouse CI
// (lighthouserc.json), not statically here — eslint-plugin-jsx-a11y was dropped
// because its latest release (6.10.2) caps at eslint 9 and blocked eslint 10.
import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import astro from 'eslint-plugin-astro';
import prettier from 'eslint-config-prettier';

export default [
  // public/wasm/ is wasm-bindgen GENERATED glue (committed by `just gen-wasm`,
  // like the demo media in public/demos/) — never hand-linted; a regen would
  // fight the linter every build.
  { ignores: ['dist/', '.astro/', 'node_modules/', 'public/wasm/'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...astro.configs.recommended,
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        __PIXTUOID_VERSION__: 'readonly',
        __GH_STARS__: 'readonly',
      },
    },
    rules: {
      // inline client scripts intentionally swallow storage/JSON errors
      'no-empty': ['error', { allowEmptyCatch: true }],
      'no-unused-vars': ['error', { caughtErrors: 'none', argsIgnorePattern: '^_' }],
      '@typescript-eslint/no-unused-vars': [
        'error',
        { caughtErrors: 'none', argsIgnorePattern: '^_' },
      ],
    },
  },
  prettier,
];
