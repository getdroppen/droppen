import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { AppErrorBoundary } from "../src/components/error-boundary.js";

test("AppErrorBoundary renders children when no error is thrown and derives error state cleanly", () => {
  const html = renderToStaticMarkup(
    <AppErrorBoundary>
      <div>healthy</div>
    </AppErrorBoundary>,
  );
  assert.match(html, /healthy/);
  const next = AppErrorBoundary.getDerivedStateFromError(new Error("boom"));
  assert.equal(next.error?.message, "boom");
});
