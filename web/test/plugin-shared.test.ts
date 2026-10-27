// The shared-module facades (`build/plugin-shared.ts`): one chunk per module
// a plugin leaves bare, each re-exporting the host's own instance under the
// names a plugin imports, and an import map that names them. A mini build
// stands in for the app: its entry is the host, and the facades must hand a
// plugin the very objects the host loaded (`useState`, `jsx`, `definePlugin`,
// `PluginHostContext`, …) — not copies, and not a default-only CommonJS
// interop object with no names.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { build, type Rollup } from "vite";
import { importMap, pluginShared, SHARED_MODULES } from "../build/plugin-shared.js";

const webRoot = resolve(import.meta.dirname, "..");
const ENTRY = "virtual:droppen-plugin-shared-test-entry";
const RESOLVED_ENTRY = "\0" + ENTRY;

/** The host side of the build: the modules the facades must lend out. */
const HOST_ENTRY = `
import { useState, lazy, createContext } from "react";
import { jsx, jsxs, Fragment } from "react/jsx-runtime";
import { createPortal } from "react-dom";
import { definePlugin, PluginHostContext, usePluginHost } from "@droppen/plugin-sdk";
export const host = { useState, lazy, createContext, jsx, jsxs, Fragment, createPortal, definePlugin, PluginHostContext, usePluginHost };
`;

/** A tiny entry module for the build: the host's imports, exported for comparison. */
function hostEntry(): import("vite").Plugin {
  return {
    name: "plugin-shared-test-entry",
    resolveId(source) {
      return source === ENTRY ? RESOLVED_ENTRY : null;
    },
    load(id) {
      return id === RESOLVED_ENTRY ? HOST_ENTRY : null;
    },
  };
}

/** The output as the build's bundle record, as `importMap` sees it in a real build. */
function bundleOf(output: readonly Rollup.OutputChunk[]): Rollup.OutputBundle {
  return Object.fromEntries(output.map((chunk) => [chunk.fileName, chunk]));
}

test("every shared module gets a facade of its own, and the import map names it", async () => {
  const result = await build({
    root: webRoot,
    configFile: false,
    logLevel: "silent",
    plugins: [pluginShared({ webRoot }), hostEntry()],
    build: { write: false, rollupOptions: { input: ENTRY, preserveEntrySignatures: "strict" } },
  });
  assert.ok(result, "a build result");
  const output = (Array.isArray(result) ? result : [result]).flatMap((each) =>
    each.output.filter((file): file is Rollup.OutputChunk => file.type === "chunk"),
  );
  const bundle = bundleOf(output);
  const map = importMap(bundle);
  assert.deepEqual(Object.keys(map.imports), [...SHARED_MODULES]);
  for (const specifier of SHARED_MODULES) {
    const url = map.imports[specifier]!;
    assert.match(url, new RegExp(`^/assets/plugin-shared-${specifier.replace(/^@/, "").replace(/[\\/\\.]/g, "-")}-[\\w-]+\\.js$`), specifier);
    assert.ok(bundle[url.slice(1)], `${url} is in the build`);
  }

  // Load the chunks as the browser would (no import map in Node: the facade
  // URLs are imported directly; their own imports are relative).
  const dir = mkdtempSync(join(tmpdir(), "droppen-shared-"));
  for (const file of output) {
    const path = join(dir, file.fileName);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, file.code);
  }
  writeFileSync(join(dir, "package.json"), '{"type":"module"}\n');
  const load = (fileName: string) => import(pathToFileURL(join(dir, fileName)).href);
  const entry = output.find((chunk) => chunk.isEntry && chunk.facadeModuleId === RESOLVED_ENTRY);
  assert.ok(entry, "the host entry is its own chunk");
  const { host } = (await load(entry.fileName)) as { host: Record<string, unknown> };
  const shared = {} as Record<string, Record<string, unknown>>;
  for (const specifier of SHARED_MODULES) shared[specifier] = await load(map.imports[specifier]!.slice(1));

  const react = shared.react!;
  for (const name of ["useState", "lazy", "createContext"]) {
    assert.equal(typeof react[name], "function", `react exports ${name}`);
    assert.equal(react[name], host[name], `${name} is the host's own`);
  }
  assert.equal(typeof react.default, "object", "a default export for `import React from \"react\"`");
  assert.equal((react.default as Record<string, unknown>).useState, host.useState, "and it is the same React the host loaded");

  const jsxRuntime = shared["react/jsx-runtime"]!;
  for (const name of ["jsx", "jsxs", "Fragment"]) assert.equal(jsxRuntime[name], host[name], `jsx-runtime shares ${name}`);

  assert.equal(shared["react-dom"]!.createPortal, host.createPortal, "react-dom is the host's");

  const sdk = shared["@droppen/plugin-sdk"]!;
  for (const name of ["definePlugin", "PluginHostContext", "usePluginHost"]) {
    assert.notEqual(sdk[name], undefined, `the SDK exports ${name}`);
    assert.equal(sdk[name], host[name], `${name} is the host's own`);
  }
});
