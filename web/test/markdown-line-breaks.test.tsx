// A person's message keeps the lines they typed.
//
// The bug: a message written over several lines (Shift+Enter in the composer,
// or typed in the CLI) was drawn as one run-on paragraph, because the
// transcript drew it as markdown and markdown draws a single newline as a
// space. What is pinned here: with `lineBreaks` every newline inside a
// paragraph is a `<br>`, a blank line still starts a new paragraph, and code
// keeps its newlines as they are, with no `<br>` inside it; an agent's reply
// stays markdown as written; and the transcript turns `lineBreaks` on for the
// user's turns only.
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
import { ItemBlock } from "../src/features/sessions/transcript-rows.js";
import { transcriptItem as item } from "./session-fixtures.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } },
});

function draw(node: React.ReactNode): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  // A break is drawn as `<br>` and the newline after it, which is only whitespace.
  return html.replace(/<br\/>\n/g, "<br/>");
}

test("a typed message keeps its lines: each newline is a line break, in one paragraph", () => {
  const html = draw(<Markdown text={"check the build\nthen run the tests\r\nand tell me"} lineBreaks />);
  assert.match(html, /<p[^>]*>check the build<br\/>then run the tests<br\/>and tell me<\/p>/);
});

test("a blank line still starts a new paragraph, not a pair of breaks", () => {
  const html = draw(<Markdown text={"first\n\nsecond"} lineBreaks />);
  assert.match(html, /<p[^>]*>first<\/p>\s*<p[^>]*>second<\/p>/);
  assert.doesNotMatch(html, /<br\/>/);
});

test("code keeps its own newlines: no break inside a fence or inline code", () => {
  const html = draw(<Markdown text={"see:\n```\nfn main() {\n}\n```\nand `a` here"} lineBreaks />);
  assert.match(html, /<code[^>]*>fn main\(\) \{\n\}\n<\/code>/);
  assert.equal((html.match(/<br\/>/g) ?? []).length, 0, html);
});

test("lists and links keep their markdown, with the typed break inside the item", () => {
  const html = draw(<Markdown text={"- one\n- two\n\n[the docs](https://example.com)\nplease"} lineBreaks />);
  assert.match(html, /<li[^>]*>one<\/li>/);
  assert.match(html, /<a [^>]*href="https:\/\/example.com"[^>]*>the docs<\/a><br\/>please/);
});

test("an agent's reply is markdown as written: a single newline stays soft", () => {
  const html = draw(<Markdown text={"one line\nwrapped by the model"} />);
  assert.match(html, /<p[^>]*>one line\nwrapped by the model<\/p>/);
  assert.doesNotMatch(html, /<br\/>/);
});

test("the transcript keeps the user's lines and leaves the agent's markdown alone", () => {
  const typed = "fix the login page\nkeep the old route working";
  const user = draw(<ItemBlock item={item({ role: "user", text: typed })} />);
  assert.match(user, /fix the login page<br\/>keep the old route working/);
  const agent = draw(<ItemBlock item={item({ role: "assistant", text: typed })} />);
  assert.doesNotMatch(agent, /<br\/>/);
  assert.match(agent, /fix the login page\nkeep the old route working/);
});
