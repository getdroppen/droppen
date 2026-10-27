// Pictures, downloads and GitHub images choosing their way to the machine.
//
// Local mode (the direct transport): exactly today's URLs, tickets and all.
// Cloud mode (a tunneled transport): nothing on the page points at its own
// origin — which is the hub — for a file: pictures are drawn from `blob:` URLs
// of bytes fetched over the channel's blob lane, a placeholder until they are
// in and the reason when they cannot be; a download link points nowhere and a
// click fetches and hands over a `blob:` URL with a proper name; GitHub images
// go the same lane. And the list of downloads under way says how far along
// each is, or why it failed.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { DownloadLink, HostImage, hostImageRoute, startDownload } from "../src/components/host-file.js";
import { LightboxImage } from "../src/components/image-lightbox.js";
import { Markdown } from "../src/components/markdown.js";
import { blobErrorText, transfersNow, TransferList } from "../src/components/blob-transfers.js";
import { githubThroughChannel, MediaFetchError } from "../src/components/source-image.js";
import { FilePreview } from "../src/features/files/file-preview.js";
import { directTransport, type Transport } from "../src/lib/transport/index.js";
import { homeMachine, openMachine, transportFor } from "../src/lib/machines/registry.js";
import { BlobError, hostBlobKey, hostBlobs } from "../src/lib/transport/blob.js";
import { offlineTransport } from "../src/lib/transport/offline.js";
import { fakeMachine, json } from "./fake-machine.js";
import { LOCAL } from "../src/lib/machines/key.js";

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} } },
  interpolation: { escapeValue: false },
});
const t = i18n.t.bind(i18n);

function useTransport(context: TestContext, next: Transport): void {
  openMachine(homeMachine(), next);
  context.after(() => {
    openMachine(homeMachine(), directTransport);
    hostBlobs.clear();
  });
}

function draw(node: React.ReactNode): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

const PICTURE = "/home/me/截图.png";

test("local mode: a host picture and a download link are today's URLs on the page's origin", () => {
  const html = draw(
    <>
      <HostImage path={PICTURE} alt="shot" />
      <DownloadLink path="/home/me/r.pdf" name="r.pdf">
        get
      </DownloadLink>
    </>,
  );
  assert.match(html, /<img src="\/api\/fs\/download\?path=%2Fhome%2Fme%2F%E6%88%AA%E5%9B%BE\.png&amp;inline=true"/);
  assert.match(html, /<a href="\/api\/fs\/download\?path=%2Fhome%2Fme%2Fr\.pdf" download="r\.pdf"/);
});

test("cloud mode: no file is linked to on the page's origin; a picture waits for its bytes", (context) => {
  useTransport(context, fakeMachine(() => json(200, {})).transport);
  const html = draw(
    <>
      <HostImage path={PICTURE} alt="shot" />
      <DownloadLink path="/home/me/r.pdf" name="r.pdf">
        get
      </DownloadLink>
      <LightboxImage source={{ kind: "host", path: PICTURE }} alt="thumb" />
      <Markdown text={`![chart](/home/me/chart.png) and [the report](/home/me/report.pdf)`} />
      <FilePreview file={PICTURE} />
    </>,
  );
  assert.doesNotMatch(html, /\/api\/fs\/download/, "nothing points at the hub for a file");
  assert.doesNotMatch(html, /ticket=/);
  assert.match(html, /aria-busy="true"/, "the existing placeholder while the bytes come");
  assert.match(html, /<a href="#" data-tunneled=""/);
});

test("cloud mode: a picture whose bytes are in is drawn from a blob: URL", (context) => {
  useTransport(context, fakeMachine(() => json(200, {})).transport);
  hostBlobs.put(hostBlobKey(LOCAL, hostImageRoute(PICTURE)), new Blob(["png"], { type: "image/png" }));
  const html = draw(<HostImage path={PICTURE} alt="shot" className="thumb" />);
  assert.match(html, /<img src="blob:[^"]+" alt="shot" class="thumb"/);
});

test("cloud mode: a download fetches over the blob lane and hands the browser a named blob: URL", async (context) => {
  const machine = fakeMachine(() => ({
    status: 200,
    headers: [
      ["content-type", "application/pdf"],
      ["content-disposition", `attachment; filename="weekly report.pdf"`],
    ],
    chunks: ["%PDF-", "1.7"],
  }));
  useTransport(context, machine.transport);
  const opened: [string, string | undefined][] = [];
  await startDownload(LOCAL, "/home/me/r.pdf", {}, (url, name) => opened.push([url, name]));
  assert.equal(machine.exchanges[0]!.kind, "blob");
  assert.equal(machine.exchanges[0]!.meta.path, "/api/fs/download?path=%2Fhome%2Fme%2Fr.pdf");
  assert.equal(opened.length, 1);
  assert.match(opened[0]![0], /^blob:/);
  assert.equal(opened[0]![1], "weekly report.pdf", "the machine's name for it, since a blob: URL has none");
  assert.equal(transfersNow().length, 0, "nothing left under way");

  // A picture already drawn is not fetched again to be saved.
  hostBlobs.put(hostBlobKey(LOCAL, hostImageRoute(PICTURE)), new Blob(["png"]));
  await startDownload(LOCAL, PICTURE, { inline: true }, (url, name) => opened.push([url, name]));
  assert.equal(machine.exchanges.length, 1);
  assert.equal(opened[1]![1], "截图.png");
});

test("cloud mode, machine offline: a download says why it failed", async (context) => {
  const offline = offlineTransport(null, {
    user: null,
    messages: { readOnly: "", noCopy: "", unavailable: "the device is offline — this needs it to be online" },
  });
  useTransport(context, offline);
  const opened: string[] = [];
  await startDownload(LOCAL, "/home/me/r.pdf", { name: "r.pdf" }, (url) => opened.push(url));
  assert.equal(opened.length, 0, "nothing handed to the browser");
  const failed = transfersNow().find((x) => x.name === "r.pdf");
  assert.ok(failed && failed.error instanceof BlobError);
  assert.equal(blobErrorText(failed.error, t), "the device is offline — this needs it to be online");
  const html = draw(<TransferList transfers={[failed]} />);
  assert.match(html, /role="alert"/);
  assert.match(html, /could not download: the device is offline — this needs it to be online/);
});

test("the transfer list shows how far along a download is", () => {
  const html = draw(
    <TransferList
      transfers={[
        { id: 1, name: "big.iso", received: 50, total: 200 },
        { id: 2, name: "stream.log", received: 3 * 1024 * 1024, total: null },
      ]}
    />,
  );
  assert.match(html, /big\.iso/);
  assert.match(html, /25%/);
  assert.match(html, /3\.0 MB/, "bytes so far when the size is not known");
  assert.match(html, /width:25%/);
});

test("a download from a machine not in view says which machine it comes from", () => {
  const labelOf = (machine: string | undefined) => (machine && machine !== "m-here" ? (machine === "m-there" ? "studio" : machine) : null);
  const html = draw(
    <TransferList
      labelOf={labelOf}
      transfers={[
        { id: 1, name: "here.zip", machine: "m-here", received: 1, total: 2 },
        { id: 2, name: "there.zip", machine: "m-there", received: 1, total: 2 },
      ]}
    />,
  );
  assert.equal((html.match(/data-transfer-machine/g) ?? []).length, 1, "only the other machine's download is labelled");
  assert.match(html, /there\.zip<\/span><span data-transfer-machine="" class="[^"]*" title="studio">studio</);
});

test("cloud mode: a GitHub picture goes over the blob lane, and its failure keeps the channel's reason", async (context) => {
  const machine = fakeMachine((e) => (e.meta.path.includes("missing") ? json(404, { error: "not found on GitHub" }) : { status: 200, headers: [["content-type", "image/png"]], chunks: ["png"] }));
  useTransport(context, machine.transport);
  const url = "https://github.com/o/r/blob/main/a.png";
  const blob = await githubThroughChannel(url, new AbortController().signal, transportFor(homeMachine()));
  assert.equal(await blob.text(), "png");
  assert.equal(machine.exchanges[0]!.kind, "blob");
  assert.equal(machine.exchanges[0]!.meta.path, `/api/media/github?url=${encodeURIComponent(url)}`);
  await assert.rejects(githubThroughChannel("https://github.com/o/r/blob/main/missing.png", new AbortController().signal, transportFor(homeMachine())), (error: unknown) => {
    assert.ok(error instanceof MediaFetchError);
    assert.equal(error.status, 404, "the status the retry rule reads");
    assert.equal(error.message, "not found on GitHub");
    return true;
  });
});

test("blob errors in words: too large, unreachable, or the machine's own sentence", () => {
  assert.equal(blobErrorText(new BlobError("tooLarge", "x", null, 25 * 1024 * 1024), t), "larger than 25 MB: too large to move through the hub");
  assert.equal(blobErrorText(new BlobError("network", "pipe closed"), t), "the device could not be reached");
  assert.equal(blobErrorText(new BlobError("status", "permission denied", 403), t), "permission denied");
});
