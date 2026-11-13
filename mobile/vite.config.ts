import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
import { tokensCss } from './tokens.ts';

const desktopCss = fileURLToPath(new URL('../app/src/index.css', import.meta.url));

// `tauri android dev` serves the frontend to the phone at this host.
const host = process.env.TAURI_DEV_HOST;

// The Tauri CLI says "true" for a debug build (`tauri android build --debug`,
// `tauri android dev`). Only then is CI's scripted computer (src/lib/demo.ts)
// in the app; a release build's JavaScript leaves it out.
const debugBuild = process.env.TAURI_ENV_DEBUG === 'true';

export default defineConfig({
  plugins: [react()],
  define: {
    __SHODH_TOKENS__: JSON.stringify(tokensCss(readFileSync(desktopCss, 'utf8'))),
    __SHODH_DEMO__: JSON.stringify(debugBuild),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
  },
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.{ts,tsx}'],
    setupFiles: ['tests/setup.ts'],
  },
});
