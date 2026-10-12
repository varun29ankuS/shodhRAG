/**
 * Production build of the long-chat measurement harness (perf/long-chat.html)
 * into perf/dist. The app's own build (index.html) never includes it.
 *   npx vite build --config perf/vite.config.ts
 */
import path from 'path';
import { defineConfig, mergeConfig } from 'vite';
import base from '../vite.config';

export default defineConfig(async env => {
  const resolved = typeof base === 'function' ? await base(env) : await base;
  return mergeConfig(resolved, {
    build: {
      outDir: path.resolve(__dirname, 'dist'),
      emptyOutDir: true,
      rollupOptions: { input: path.resolve(__dirname, 'long-chat.html') },
    },
  });
});
