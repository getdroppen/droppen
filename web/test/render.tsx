// The page tests' one way to stand up i18next and draw a tree. A page is
// rendered with `renderToStaticMarkup` inside `I18nextProvider`; the instance
// is the app's plugin (`react-i18next`) with the app's interpolation, and the
// resources are the file's own (English keys are the source strings, so `en`
// is empty where the page adds none). A file that changes language between
// draws passes `lng`; the page tests that need a query client wrap `node`.
import * as React from "react";
import i18next, { type i18n as I18n, type InitOptions } from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";

/** An i18next instance for a page under test, exactly as the app's own is initialised. */
export function makeI18n(options: InitOptions = {}): I18n {
  const instance = i18next.createInstance();
  void instance.use(initReactI18next).init({ interpolation: { escapeValue: false }, ...options });
  return instance;
}

/** `draw(node, lng)` for `i18n`: the page's markup in `lng` (English by default). */
export function pageDraw(i18n: I18n, wrap: (node: React.ReactNode) => React.ReactNode = (node) => node) {
  return (node: React.ReactNode, lng = "en"): string => {
    void i18n.changeLanguage(lng);
    return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{wrap(node)}</I18nextProvider>);
  };
}

/** Every element React made in `node`, depth first (a static tree has no DOM to walk). */
export type El = React.ReactElement<Record<string, unknown>>;

export function elements(node: unknown, out: El[] = []): El[] {
  if (Array.isArray(node)) for (const n of node) elements(n, out);
  else if (node && typeof node === "object" && "props" in node) {
    out.push(node as El);
    elements((node as El).props.children, out);
  }
  return out;
}

/**
 * Render `Component` inline in a probe and run `steps[i]` on its tree at
 * render `i` (the devices tests' driver): a statically rendered component
 * cannot be clicked, so a step reads the tree and the next render sees what
 * it changed.
 */
export function driveWith(draw: (node: React.ReactNode, lng?: string) => string) {
  return function drive<P>(Component: (props: P) => React.ReactNode, props: P, steps: ((tree: El[]) => void)[], lng = "en"): { html: string; tree: El[]; ran: number } {
    let i = 0;
    let tree: El[] = [];
    function Probe() {
      const out = Component(props);
      tree = elements(out);
      if (i < steps.length) steps[i++]!(tree);
      return out;
    }
    const html = draw(<Probe />, lng);
    return { html, tree, ran: i };
  };
}
