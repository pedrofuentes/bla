/**
 * Release gate for command mode. Keep this mirrored with
 * `src-tauri/src/lib.rs::COMMAND_MODE_ENABLED`; each language reads only its
 * own constant so there are no scattered runtime booleans.
 */
export const COMMAND_MODE_ENABLED = false;
