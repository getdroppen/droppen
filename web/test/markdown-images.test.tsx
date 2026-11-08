// Pictures in an agent's markdown.
//
// The bug: an agent that opened a pull request against a private repository
// pushed its screenshots to that repository and wrote
// `![…](https://github.com/<owner>/<repo>/blob/pr-assets/…png?raw=true)`. The
// markdown renderer had no `img` override, so the browser fetched that URL
// itself, without a GitHub login, got a 404 and drew a broken-image glyph.
//
// What is pinned here: which of the three sources a `src` is drawn from (the
// host's file route, the server's `gh` proxy, or the web directly), that the
// proxy's answer is what the `<img>` draws, that a picture that cannot be drawn
// becomes a card with its alt text and link, that a picture from the web is a
// card naming its host until the reader asks for it (and none of its `<img>`
// before), and what the full-size overlay offers. `react-dom/server` rather
// than a DOM, like the other render tests: there is no click to simulate, so
// the overlay is rendered on its own and the reader's request is made with
// `requestImage`, which is what the card's button calls
// (`markdown-images-dom.test.tsx` clicks it).
//
// The requests are the page's for as long as it is open, so each test that
// makes one uses a URL of its own.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { Markdown } from "../src/components/markdown.js";
import { LightboxOverlay, requestImage } from "../src/components/image-lightbox.js";
import { githubImageKey, MediaFetchError } from "../src/components/source-image.js";
import { imageFileName, imageSource, isGithubFileUrl } from "../src/lib/image-source.js";
import { localFilePath } from "../src/lib/fs-url.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const PR_IMAGE = "https://github.com/octo/example/blob/pr-assets/pr-47/sidebar-mac-light.png?raw=true";

function draw(node: React.ReactNode, seed?: (client: QueryClient) => void): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  seed?.(client);
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

/* ── where a src is drawn from ──────────────────────────────────────────── */

test("the PR screenshots an agent pushed go through the gh proxy", () => {
  for (const url of [
    PR_IMAGE,
    "https://github.com/octo/example/blob/pr-assets/pr-47/sidebar-mac-dark.png?raw=true",
    "https://github.com/o/r/blob/main/a.png",
    "https://github.com/o/r/blob/feature/x/docs/a.png?raw=1",
    "https://github.com/o/r/raw/main/a.png",
    "https://raw.githubusercontent.com/o/r/main/a.png",
    "https://raw.githubusercontent.com/o/r/main/a.png?token=GHSAT0AAA",
  ]) {
    assert.deepEqual(imageSource(url), { kind: "github", url }, url);
  }
});

test("other web images load directly, including GitHub URLs the proxy does not take", () => {
  for (const url of [
    "https://img.shields.io/badge/license-MIT-111111",
    "https://github.com/user-attachments/assets/6d0b4c1e-0000-4000-8000-000000000000",
    "https://github.com/o/r/blob/main/a.png?x=1",
    "https://github.com.evil.com/o/r/blob/main/a.png",
    "https://github.com:8443/o/r/blob/main/a.png",
    "http://github.com/o/r/blob/main/a.png",
    "https://example.com/a.png",
  ]) {
    assert.deepEqual(imageSource(url), { kind: "direct", url }, url);
    assert.equal(isGithubFileUrl(url), false, url);
  }
});

test("host paths, in every spelling, use the host file route", () => {
  assert.deepEqual(imageSource("/Users/me/shot.png"), { kind: "host", path: "/Users/me/shot.png" });
  assert.deepEqual(imageSource("~/shot.png"), { kind: "host", path: "~/shot.png" });
  assert.deepEqual(imageSource("file:///Users/me/%E6%88%AA%E5%9B%BE.png"), { kind: "host", path: "/Users/me/截图.png" });
  assert.deepEqual(imageSource("file://localhost/tmp/a.png"), { kind: "host", path: "/tmp/a.png" });
  assert.equal(localFilePath("file://elsewhere/tmp/a.png"), null, "another machine's file is not ours");
});

test("nothing tries to draw a src with no meaning here", () => {
  for (const src of [undefined, "", "docs/a.png", "./a.png", "data:image/png;base64,AAAA", "//cdn.example.com/a.png", "/api/settings"]) {
    assert.equal(imageSource(src), null, String(src));
  }
});

test("a saved picture is named after its file", () => {
  assert.equal(imageFileName({ kind: "github", url: PR_IMAGE }), "sidebar-mac-light.png");
  assert.equal(imageFileName({ kind: "github", url: "https://github.com/o/r/blob/main/%E6%88%AA%E5%9B%BE.png" }), "截图.png");
  assert.equal(imageFileName({ kind: "host", path: "/tmp/a b.png" }), "a b.png");
});

/* ── what the markdown draws ────────────────────────────────────────────── */

test("a GitHub image draws the proxy's bytes, not the URL the browser cannot open", () => {
  const html = draw(<Markdown text={`![侧边栏 macOS 浅色 修改前后](${PR_IMAGE})`} />, (client) =>
    client.setQueryData(githubImageKey(LOCAL, PR_IMAGE), new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: "image/png" })),
  );
  assert.match(html, /<button[^>]*title="侧边栏 macOS 浅色 修改前后"[^>]*cursor-zoom-in/);
  assert.match(html, /<img src="blob:[^"]+" alt="侧边栏 macOS 浅色 修改前后" class="max-h-60 /);
  assert.doesNotMatch(html, /<img src="https:\/\/github\.com/);
  assert.match(html, /<span class="block max-w-full truncate text-2xs text-muted">侧边栏 macOS 浅色 修改前后<\/span>/, "alt as caption");
  assert.doesNotMatch(html, /data-image-on-request|load image/, "the proxy's bytes are the host's to fetch, so drawn on sight");
});

test("a GitHub image still on its way holds its place", () => {
  const html = draw(<Markdown text={`![shot](${PR_IMAGE})`} />);
  assert.match(html, /<span role="img" aria-label="shot" aria-busy="true"/);
  assert.doesNotMatch(html, /<img/);
});

test("a proxy refusal becomes a card with the reason and the link, not a broken glyph", () => {
  const html = draw(<Markdown text={`![shot](${PR_IMAGE})`} />, (client) =>
    client
      .getQueryCache()
      .build(client, { queryKey: githubImageKey(LOCAL, PR_IMAGE) })
      .setState({ status: "error", error: new MediaFetchError("gh is not logged in on the server (run `gh auth login`)", 503) }),
  );
  assert.match(html, /data-image-fallback/);
  assert.match(html, />shot</);
  assert.match(html, /gh is not logged in on the server/);
  assert.ok(html.includes(`href="${PR_IMAGE.replace(/&/g, "&amp;")}"`), html);
  assert.match(html, /target="_blank"/);
  assert.match(html, />open link</);
  assert.doesNotMatch(html, /<img|<button/);
});

test("a web image is not fetched until the reader asks: a card names its host instead", () => {
  const url = "https://images.example.com:8443/chart.png?q=report";
  const html = draw(<Markdown text={`before ![quarterly chart](${url}) after`} />);
  assert.doesNotMatch(html, /<img/, "nothing on the page fetches it");
  assert.match(html, /<p[^>]*>before <span data-image-on-request="" title="https:\/\/images\.example\.com:8443\/chart\.png\?q=report"/, "phrasing content in the paragraph");
  assert.match(html, />quarterly chart</);
  assert.match(html, /<span class="[^"]*font-mono text-faint">images\.example\.com:8443<\/span>/, "the host, port included");
  assert.match(html, /<button type="button"[^>]*>load image<\/button>/);
  assert.match(html, /<a href="https:\/\/images\.example\.com:8443\/chart\.png\?q=report" target="_blank" rel="noreferrer"[^>]*>open link<\/a>/);
  assert.doesNotMatch(html, /cursor-zoom-in/, "no thumbnail to open yet");
});

test("a web image with no alt text is called an image on its card", () => {
  const html = draw(<Markdown text="![](https://images.example.com/untitled.png)" />);
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /<span class="min-w-0 truncate">image<\/span>/);
});

test("once asked for, a web image loads lazily and without a referrer, and stays asked for when drawn again", () => {
  const url = "https://img.shields.io/badge/license-MIT-111111";
  const text = `![License](${url})`;
  assert.doesNotMatch(draw(<Markdown text={text} />), /<img/);
  requestImage(url);
  // Each draw is a fresh mount: a row the virtualised transcript scrolled
  // out and back in, or the same picture in another message.
  for (const html of [draw(<Markdown text={text} />), draw(<Markdown text={`again: ${text}`} />)]) {
    assert.match(html, /<img src="https:\/\/img\.shields\.io\/badge\/license-MIT-111111"[^>]*loading="lazy"[^>]*referrerPolicy="no-referrer"/i);
    assert.match(html, /<button type="button" title="License"[^>]*cursor-zoom-in/, "the thumbnail that opens full size, as before");
    assert.doesNotMatch(html, /data-image-on-request|load image/);
  }
  const other = draw(<Markdown text="![License](https://img.shields.io/badge/license-MIT-222222)" />);
  assert.doesNotMatch(other, /<img/, "the request is for that URL, not its host");
  assert.match(other, />load image</);
});

test("on a page a hub serves, the card offers the link alone: the page's policy refuses the picture", () => {
  // Cloud mode is the hub's `<meta name="droppen-hub">` in the shell (`lib/hub/config.ts`).
  const before = globalThis.document;
  globalThis.document = {
    querySelector: (selector: string) => (selector === 'meta[name="droppen-hub"]' ? { getAttribute: () => "wss://hub.example" } : null),
  } as unknown as Document;
  try {
    const html = draw(<Markdown text="![shot](https://images.example.com/cloud.png)" />);
    assert.match(html, /data-image-on-request/);
    assert.match(html, />images\.example\.com</);
    assert.match(html, /<a href="https:\/\/images\.example\.com\/cloud\.png" target="_blank" rel="noreferrer"[^>]*>open link<\/a>/);
    assert.doesNotMatch(html, /<button|load image|<img/);
  } finally {
    globalThis.document = before;
  }
});

test("host images use the ticketed file route", () => {
  const html = draw(<Markdown text="![plot](/Users/me/plot.png) ![b](file:///Users/me/b.png)" />);
  assert.match(html, /<img src="\/api\/fs\/download\?path=%2FUsers%2Fme%2Fplot\.png&amp;inline=true"/);
  assert.match(html, /<img src="\/api\/fs\/download\?path=%2FUsers%2Fme%2Fb\.png&amp;inline=true"/);
  assert.doesNotMatch(html, /data-image-on-request|load image/, "drawn on sight, as before");
});

test("an image inside a link is a card without a button or an anchor, so the link keeps the click", () => {
  const html = draw(<Markdown text="[![Stars](https://img.shields.io/github/stars/o/r)](https://github.com/o/r)" />);
  assert.match(html, /<a href="https:\/\/github\.com\/o\/r"[^>]*><span data-image-on-request=""/);
  assert.match(html, />Stars</);
  assert.match(html, />img\.shields\.io</);
  assert.doesNotMatch(html, /<img|<button|load image|open link/);
  assert.equal(html.match(/<a /g)?.length, 1, "no anchor inside the link");
});

test("an image inside a link, once asked for elsewhere, is drawn alone", () => {
  const url = "https://img.shields.io/github/forks/o/r";
  requestImage(url);
  const html = draw(<Markdown text={`[![Forks](${url})](https://github.com/o/r)`} />);
  assert.match(html, /<a href="https:\/\/github\.com\/o\/r"[^>]*><img src="https:\/\/img\.shields\.io\/github\/forks\/o\/r"/);
  assert.doesNotMatch(html, /<button/);
});

test("a src nothing can draw is a card without a link", () => {
  const html = draw(<Markdown text="![diagram](docs/arch.png)" />);
  assert.match(html, /data-image-fallback/);
  assert.match(html, />diagram</);
  assert.match(html, /docs\/arch\.png/);
  assert.doesNotMatch(html, /<img|<a /);
});

test("a plain link to an image file stays a link", () => {
  const html = draw(<Markdown text="[the shot](https://example.com/a.png) and https://example.com/b.png" />);
  assert.match(html, /<a href="https:\/\/example\.com\/a\.png"[^>]*>the shot<\/a>/);
  assert.match(html, /<a href="https:\/\/example\.com\/b\.png"/);
  assert.doesNotMatch(html, /<img|<button/);
});

test("a file:// link downloads through the host route instead of being dropped", () => {
  const html = draw(<Markdown text="[notes](file:///Users/me/notes.md)" />);
  assert.match(html, /href="\/api\/fs\/download\?path=%2FUsers%2Fme%2Fnotes\.md"/);
});

/* ── the overlay ────────────────────────────────────────────────────────── */

test("the opened lightbox shows the full picture, its caption, and the way out", () => {
  const html = draw(
    <LightboxOverlay
      alt="shot"
      caption="sidebar, before and after"
      original={PR_IMAGE}
      draw={({ className }) => <img src="blob:x" alt="shot" className={className} />}
      actions={<a href="blob:x" download="sidebar-mac-light.png" title="download" />}
      onClose={() => {}}
    />,
  );
  assert.match(html, /role="dialog" aria-modal="true" aria-label="shot"/);
  assert.match(html, /<img src="blob:x" alt="shot" class="max-h-full max-w-full rounded object-contain"/);
  assert.match(html, />sidebar, before and after</);
  assert.match(html, /<a href="https:\/\/github\.com\/octo\/example[^"]*" target="_blank" rel="noreferrer" title="open original"/);
  assert.match(html, /download="sidebar-mac-light\.png"/);
  assert.match(html, /<button type="button" title="back"/);
});
