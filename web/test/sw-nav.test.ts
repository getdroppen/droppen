import assert from "node:assert/strict";
import test from "node:test";
import { internalNavPath } from "../src/lib/sw-nav.js";

test("internalNavPath accepts a same-origin path from a nav message", () => {
  assert.equal(internalNavPath({ type: "nav", url: "/sessions/claude/abc" }), "/sessions/claude/abc");
  assert.equal(internalNavPath({ type: "nav", url: "/" }), "/");
});

test("internalNavPath rejects anything that would leave the origin", () => {
  // An absolute URL to another origin, a scheme-relative host, and a dangerous
  // scheme — none of these is an internal path.
  assert.equal(internalNavPath({ type: "nav", url: "https://evil.example/steal" }), null);
  assert.equal(internalNavPath({ type: "nav", url: "//evil.example/steal" }), null);
  assert.equal(internalNavPath({ type: "nav", url: "javascript:alert(1)" }), null);
});

test("internalNavPath rejects the wrong message shape", () => {
  assert.equal(internalNavPath({ type: "other", url: "/ok" }), null);
  assert.equal(internalNavPath({ type: "nav", url: 42 }), null);
  assert.equal(internalNavPath({ type: "nav" }), null);
  assert.equal(internalNavPath(null), null);
  assert.equal(internalNavPath("nope"), null);
});

test("internalNavPath rejects what a URL parser would take off the origin: a backslash, a tab or line break", () => {
  // The notification's URL comes from a push payload; `public/sw.js` applies the same rule to `openWindow`.
  for (const url of ["/\\evil.example/x", "/\t/evil.example/x", "/\n/evil.example", "/%0a", "/sessions\u0000"]) {
    const got = internalNavPath({ type: "nav", url });
    assert.ok(got === null || !/[\\\t\n\u0000]/.test(got), JSON.stringify(url));
  }
  assert.equal(internalNavPath({ type: "nav", url: "/\\evil.example/x" }), null);
  assert.equal(internalNavPath({ type: "nav", url: "/\t/evil.example/x" }), null);
  assert.equal(internalNavPath({ type: "nav", url: "/sessions/claude/abc?x=1#top" }), "/sessions/claude/abc?x=1#top", "a query and a fragment are fine");
});
