import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://zerho.dev',
  output: 'static',
  compressHTML: true,
  build: {
    inlineStylesheets: 'auto',
  },
});
