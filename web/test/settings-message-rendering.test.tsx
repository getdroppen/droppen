// Settings → message rendering (getdroppen/droppen#167): LaTeX math is a group
// of its own, not a row among the app's preferences, and it reads the `math`
// pref — a server from before the pref sends none, which is "rendered".
//
// `react-dom/server` with the prefs already in the query cache, like the other
// render tests. `import * as React` because the test directory is outside the
// tsconfig's `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";

// The settings page reaches the plugin host, which imports the build's
// bundled-plugin list: answer it as the open build does.
const bundled = `data:text/javascript,${encodeURIComponent('export const distribution = "public"; export const bundledPlugins = [];')}`;
register(
  `data:text/javascript,${encodeURIComponent(
    `export async function resolve(specifier, context, next) { return specifier === "virtual:droppen/bundled-plugins" ? { url: ${JSON.stringify(bundled)}, shortCircuit: true } : next(specifier, context); }`,
  )}`,
);

const { renderToStaticMarkup } = await import("react-dom/server");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { default: i18next } = await import("i18next");
const { I18nextProvider, initReactI18next } = await import("react-i18next");
const { MessageRenderingSection, PreferencesSection } = await import("../src/features/settings/settings-view.js");
const { zhCN } = await import("../src/i18n/zh-CN.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const { mq } = await import("../src/lib/machines/query.js");

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { "zh-CN": { translation: zhCN } },
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

const PREFS = { sessionGrouping: "flat", composerSend: "enter", fontSize: "default", language: "auto", transcriptStyle: "flat" };

function draw(node: React.ReactNode, prefs: Record<string, string>, lang = "en"): string {
  void i18n.changeLanguage(lang);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "prefs"), prefs);
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

/** The label of the tab drawn as chosen. */
const chosen = (html: string) => /<button[^>]*aria-selected="true"[^>]*>([^<]*)</.exec(html)?.[1];

test("LaTeX math is its own group, titled for what it is about", () => {
  const group = draw(<MessageRenderingSection />, { ...PREFS, math: "rendered" });
  assert.match(group, /<h2[^>]*>message rendering<\/h2>/);
  assert.match(group, /LaTeX math/);
  assert.doesNotMatch(draw(<PreferencesSection />, { ...PREFS, math: "rendered" }), /LaTeX/, "and no longer a preferences row");
});

test("the tab drawn as chosen is the pref, and a server without the pref means rendered", () => {
  assert.equal(chosen(draw(<MessageRenderingSection />, { ...PREFS, math: "source" })), "source");
  assert.equal(chosen(draw(<MessageRenderingSection />, { ...PREFS, math: "rendered" })), "rendered");
  assert.equal(chosen(draw(<MessageRenderingSection />, PREFS)), "rendered");
});

test("in Chinese", () => {
  const html = draw(<MessageRenderingSection />, { ...PREFS, math: "rendered" }, "zh-CN");
  assert.match(html, /消息渲染/);
  assert.match(html, /LaTeX 公式/);
  assert.equal(chosen(html), "渲染");
});
