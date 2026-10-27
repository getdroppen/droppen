/**
 * The two plugin-package calls Settings › Plugins makes
 * (`docs/plugins/runtime.md` §4): the tarball's bytes as the body of
 * `POST /api/plugins`, and `DELETE /api/plugins/{id}`.
 *
 * The routes and the status they answer are the generated schema's; only the
 * install body needs help. The OpenAPI body is a binary, which the generator
 * types as `string`, while the page holds the file as a `Blob` — so the Blob
 * is handed to the client as it is and the client's default JSON serializer
 * is bypassed instead of stringifying it.
 */
import type { ApiClient } from "@/api/client";
import type { PluginStatus } from "./types";

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
  const res = await api.POST("/api/plugins", {
    // The generated binary type is `string`; the request body remains the Blob
    // (see the module comment), and gzip is one of the two types the server takes.
    body: file as unknown as string,
    bodySerializer: (body) => body,
    headers: { "content-type": "application/gzip" },
  });
  if (!res.response.ok || !res.data) throw new Error(pluginFailure(res));
  return res.data;
}

/** Remove the installed package; its documents and on/off decision stay. */
export async function uninstallPlugin(api: ApiClient, id: string): Promise<void> {
  const res = await api.DELETE("/api/plugins/{id}", { params: { path: { id } } });
  if (!res.response.ok) throw new Error(pluginFailure(res));
}
