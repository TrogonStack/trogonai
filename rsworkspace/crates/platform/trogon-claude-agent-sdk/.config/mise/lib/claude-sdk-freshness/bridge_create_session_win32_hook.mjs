// Preloaded with `--import` for Windows scenarios: routes the bridge's `path` imports to the win32 flavor, the way a
// Windows node would load them, because overriding `process.platform` alone leaves `node:path` posix on other hosts.
import { register } from 'node:module';

register('./bridge_create_session_win32_loader.mjs', import.meta.url);
