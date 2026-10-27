import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { FileChip } from "../src/components/file-chip.js";
import { FilePreview } from "../src/features/files/file-preview.js";
import type { components } from "../src/api/schema.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});
type FsRead = components["schemas"]["FsRead"];
const path = "/home/me/报告/月度汇总（终版）.xlsx";

function draw(node: React.ReactNode, file = path, data?: Partial<FsRead>, error = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  if (data) client.setQueryData(mq(LOCAL, "fs-read", file), { path: file, sizeBytes: 123, mtime: null, binary: false, truncated: false, content: "", ...data });
  if (error) {
    client.getQueryCache().build(client, { queryKey: mq(LOCAL, "fs-read", file) }).setState({ status: "error", error: new Error("not found") });
  }
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

test("file chips have a filename preview button and a separate ticket-aware download link", () => {
  const html = draw(<FileChip name="月度汇总（终版）.xlsx" path={path} />);
  assert.match(html, /<button[^>]*aria-label="view 月度汇总（终版）\.xlsx"/);
  assert.match(html, /aria-haspopup="dialog"/);
  assert.match(html, /<\/button><a /, "download is not nested inside the preview trigger");
  assert.ok(html.includes(`href="/api/fs/download?path=${encodeURIComponent(path)}"`));
  assert.match(html, /download="月度汇总（终版）\.xlsx"/);
  assert.match(html, /download 月度汇总（终版）\.xlsx/);
  assert.doesNotMatch(html, /<pre|<section|<img/, "closed chips do not mount or fetch previews");
});

test("the shared preview renders text inertly and keeps the download action", () => {
  const html = draw(<FilePreview file="/home/me/HANDOFF.md" />, "/home/me/HANDOFF.md", { content: "# Ready\n<script>alert(1)</script>" });
  assert.match(html, /<pre/);
  assert.match(html, /# Ready/);
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<script>|back to file list/);
  assert.match(html, /title="download"/);
});

test("unsupported binary files show their filename and keep download available", () => {
  const html = draw(<FilePreview file={path} />, path, { binary: true, content: "not displayable" });
  assert.match(html, /月度汇总（终版）\.xlsx/);
  assert.match(html, /binary file · use download/);
  assert.match(html, /title="download"/);
  assert.doesNotMatch(html, /<pre|not displayable/);
});

test("raster images reuse the authenticated image renderer", () => {
  const image = "/home/me/截图.png";
  const html = draw(<FilePreview file={image} />, image);
  assert.match(html, /<img/);
  assert.match(html, /inline=true/);
  assert.doesNotMatch(html, /<pre|cannot read file/);
});

test("the file-list adapter retains its back button and truncated-file warning", () => {
  const html = draw(<FilePreview file={path} onClose={() => {}} />, path, { content: "partial", truncated: true });
  assert.match(html, /back to file list/);
  assert.match(html, /preview truncated · download the complete file/);
});

test("read errors retain the filename, retry and download instead of a blank modal", () => {
  const html = draw(<FilePreview file={path} />, path, undefined, true);
  assert.match(html, /role="alert"/);
  assert.match(html, /cannot read file/);
  assert.match(html, /retry/);
  assert.match(html, /title="download"/);
  assert.match(html, /月度汇总（终版）\.xlsx/);
});
