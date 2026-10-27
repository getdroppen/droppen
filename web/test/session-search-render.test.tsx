// What the in-session search bar says: the counter, the hits in a snippet, and
// the three notices — none, too many, and a jump that is paging history in.
//
// `react-dom/server` with no DOM, like `transcript-rows.test.tsx`; the bar is
// driven by a hand-built controller, since everything it decides is in
// `session-search.ts` and tested there.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import type { SearchMatch } from "../src/features/sessions/session-search.js";
import {
  SessionSearchBar,
  Snippet,
  type SessionSearch,
} from "../src/features/sessions/session-search-view.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

function match(index: number): SearchMatch {
  return { index, role: "user", kind: "text", field: "text", snippet: `item ${index}`, ranges: [[0, 4]], timestamp: null };
}

function controller(over: Partial<SessionSearch>): SessionSearch {
  const noop = () => {};
  return {
    open: true,
    openBar: noop,
    close: noop,
    inputRef: { current: null },
    text: "item",
    setText: noop,
    commitText: noop,
    searching: false,
    error: null,
    result: null,
    position: -1,
    enter: noop,
    step: noop,
    jumpTo: noop,
    paging: null,
    cancelJump: noop,
    jumpFailed: false,
    ...over,
  };
}

function result(count: number, total: number, truncated = false) {
  const ordered = Array.from({ length: count }, (_, i) => match(1000 - i));
  return { query: "item", terms: ["item"], ordered, total, truncated, atTotal: 1000 };
}

test("hits in a snippet are marked, and only the hits", () => {
  const html = draw(<Snippet text="…fix the parser, then the parser tests" ranges={[[9, 15], [26, 32]]} />);
  assert.equal(
    html,
    '<span>…fix the </span><mark class="rounded-sm bg-warn/30 text-fg">parser</mark><span>, then the </span>' +
      '<mark class="rounded-sm bg-warn/30 text-fg">parser</mark><span> tests</span>',
  );
});

test("a closed bar draws nothing", () => {
  assert.equal(draw(<SessionSearchBar search={controller({ open: false })} />), "");
});

test("the counter says which match of how many, newest first", () => {
  const html = draw(<SessionSearchBar search={controller({ result: result(3, 3), position: 0 })} />);
  assert.match(html, />1\/3</);
  assert.match(html, /data-session-search-input/, "the box a second Cmd+F recognises");
  assert.doesNotMatch(html, /no matches|too many/);
});

test("no matches says so, and the steppers are off", () => {
  const html = draw(<SessionSearchBar search={controller({ result: result(0, 0), position: -1 })} />);
  assert.match(html, />0\/0</);
  assert.match(html, /no matches/);
  assert.match(html, /aria-label="older match \(Enter\)" disabled=""/);
});

test("more matches than were listed says how many are shown", () => {
  const listed = draw(<SessionSearchBar search={controller({ result: result(200, 1234), position: 4 })} />);
  assert.match(listed, />5\/1,234</);
  assert.match(listed, /too many matches, only the first 200 are shown/);

  const capped = draw(<SessionSearchBar search={controller({ result: result(200, 10000, true), position: 0 })} />);
  assert.match(capped, />1\/10,000\+</, "past the server's cap the count is a floor");
});

test("a jump paging history in says so and can be cancelled", () => {
  const html = draw(
    <SessionSearchBar search={controller({ result: result(3, 3), position: 2, paging: { loaded: 2, total: 5 } })} />,
  );
  assert.match(html, /loading earlier messages…/);
  assert.match(html, />2\/5</);
  assert.match(html, />cancel</);
});

test("a failed search and an unreachable match are errors", () => {
  assert.match(draw(<SessionSearchBar search={controller({ error: "boom" })} />), /search failed: boom/);
  assert.match(
    draw(<SessionSearchBar search={controller({ result: result(3, 3), position: 0, jumpFailed: true })} />),
    /could not reach this match/,
  );
});
