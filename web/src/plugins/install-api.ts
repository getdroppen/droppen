/**
 * The two plugin-package calls Settings › Plugins makes
 * (`docs/plugins/runtime.md` §4): the tarball's bytes as the body of
 * `POST /api/plugins`, and `DELETE /api/plugins/{id}`.
 *
 * They live here, over the machine's typed client, because the generated
 * schema learns the two routes' raw-body shape later: a narrow local cast
 * keeps the rest of the host typed, and this is the one file to change when
 * `web/src/api/schema.d.ts` knows them.
 */
import type { ApiClient } from "@/api/client";
import type { PluginStatus } from "./types";

/** The small part of the generated client these two routes need. */
interface PluginPackageRoutes {
  POST(
    path: "/api/plugins",
    init: {
      body: Blob;
      /** openapi-fetch JSON-encodes every body but `FormData`; the tarball goes as it is. */
      bodySerializer: (body: unknown) => BodyInit;
      params: { header: { "content-type": string } };
    },
  ): Promise<{ data?: unknown; error?: unknown; response: Response }>;
  DELETE(
    path: "/api/plugins/{id}",
    init: { params: { path: { id: string } } },
  ): Promise<{ data?: unknown; error?: unknown; response: Response }>;
}

/** The server's `{ error, message }` body, or the status line when there is none. */
export function pluginFailure({ error, response }: { error?: unknown; response: Response }): string {
  const body = (error && typeof error === "object" ? error : {}) as { error?: unknown; message?: unknown };
  if (typeof body.message === "string" && body.message) return body.message;
  if (typeof body.error === "string" && body.error) return body.error;
  return `HTTP ${response.status}`;
}

/**
 * Install (or upgrade) the plugin in `file`: its bytes, untouched, as the
 * server's tarball. Answers the installed plugin's status.
 */
export async function installPlugin(api: ApiClient, file: Blob): Promise<PluginStatus> {
  const res = await (api as unknown as PluginPackageRoutes).POST("/api/plugins", {
    body: file,
    bodySerializer: (body) => body as BodyInit,
    // `pnpm pack` writes gzip; a browser that knows nothing of `.tgz` says octet-stream.
    params: { header: { "content-type": "application/gzip" } },
  });
  if (!res.response.ok || !res.data) throw new Error(pluginFailure(res));
  return res.data as PluginStatus;
}

/** Remove the installed package; its documents and on/off decision stay. */
export async function uninstallPlugin(api: ApiClient, id: string): Promise<void> {
  const res = await (api as unknown as PluginPackageRoutes).DELETE("/api/plugins/{id}", { params: { path: { id } } });
  if (!res.response.ok) throw new Error(pluginFailure(res));
}
