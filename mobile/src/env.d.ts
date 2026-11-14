/// <reference types="vite/client" />

/** The desktop's colour tokens as the phone's stylesheet (tokens.ts, at build time). */
declare const __SHODH_TOKENS__: string;

/**
 * Whether this is a debug build (the Tauri CLI's TAURI_ENV_DEBUG): only then
 * is the scripted computer of CI's screenshots (lib/demo.ts) in the app.
 */
declare const __SHODH_DEMO__: boolean;
