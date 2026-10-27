/**
 * A plugin as this instance sees it: the server's own `PluginStatus`
 * (`docs/plugins/runtime.md` §4), read from the generated OpenAPI schema so
 * the page and the server cannot drift. `GET /api/plugins`, the feed's
 * `settings` and `plugins` frames and Settings › Plugins all use this shape.
 */
import type { components } from "@/api/schema";

export type PluginStatus = components["schemas"]["PluginStatus"];
export type PluginNotice = components["schemas"]["PluginNotice"];
