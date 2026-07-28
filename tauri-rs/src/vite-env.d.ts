/// <reference types="vite/client" />

/**
 * App version, injected at build time from `src-tauri/tauri.conf.json` — the
 * same file Tauri stamps into the bundles, so the number in the UI is always
 * the number that shipped. See `define` in `vite.config.ts`.
 */
declare const __APP_VERSION__: string;
