import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://zerho.xyz',
  output: 'static',
  compressHTML: true,
  build: {
    inlineStylesheets: 'auto',
  },
});
