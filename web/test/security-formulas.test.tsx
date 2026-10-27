// The formulas of droppen.app/security, part by part
// (`features/landing/security/formula-data.ts`, `formula-model.ts`,
// `formula.tsx`).
//
// What these pin: every token of every formula on the page names a term, and
// every term has a kind, the parties that can know it, words in en, zh-CN,
// zh-TW and ja, and a code reference that exists — a token without them fails
// here; a symbol that means different things in different places is read per
// occurrence; the flow is read off the formula's shape (kek is made by
// HKDF-SHA256 and taken by AES-256-GCM); the formulas still read and copy as
// the plain text they were; every term is focusable and described for screen
// readers, in every language; the page's two kinds of device are defined at
// its top, and every later mention of either explains itself, in every
// language. Pointer, keyboard and touch:
// `security-formulas-dom.test.tsx`.
import * as React from "react";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { JSDOM } from "jsdom";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import { SECURITY_PATH } from "../src/features/landing/route.js";
import { BLOCKS, type BlockId, type FormulaBlock, KINDS, LANGS, PARTIES, TERMS } from "../src/features/landing/security/formula-data.js";
import { analyze, flowOf, lights } from "../src/features/landing/security/formula-model.js";
import { CONCEPT_WORDS, KIND_CLASS, LIT_CLASS, HIDDEN_CLASS } from "../src/features/landing/security/formula.js";
import SecurityPage from "../src/features/landing/security/security.js";
import { securityBundles } from "../src/features/landing/security/security-copy.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    ...Object.fromEntries((["zh-CN", "zh-TW", "ja"] as const).map((lng) => [lng, { translation: { ...securityBundles[lng], ...landingBundles[lng], ...app[lng] } }]))
  },
  keySeparator: false,
  nsSeparator: false
});
Object.assign(globalThis, { location: { search: "", pathname: SECURITY_PATH, origin: "https://droppen.app", hash: "" } });

function draw(lng: string): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <SecurityPage />
    </I18nextProvider>,
  );
}

const decode = (html: string) =>
  html
    .replace(/<[^>]+>/g, "")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");

const analysis = analyze();
const blockIds = Object.keys(BLOCKS) as BlockId[];
const lineText = (line: FormulaBlock["lines"][number]) => (typeof line === "string" ? line : line.text);

test("every token of every formula names a term with a kind, its parties, words in four languages and a code reference", () => {
  let tokens = 0;
  for (const id of blockIds) {
    analysis.lines[id].forEach((line, li) => {
      for (const tok of line) {
        if (tok.type === "punct") {
          // Only punctuation is left plain: a symbol nobody named would show up here.
          assert.match(tok.text, /^[\s()[\]{},;:=·…_]+$/, `${id}:${li}: ${JSON.stringify(tok.text)} is not punctuation`);
          continue;
        }
        tokens++;
        assert.ok(tok.term, `${id}:${li}: ${JSON.stringify(tok.text)} names no term`);
        const term = TERMS[tok.term];
        assert.ok(term, `${id}:${li}: ${JSON.stringify(tok.text)} names ${tok.term}, which does not exist`);
      }
    });
  }
  assert.ok(tokens > 300, `${tokens} tokens`);
  const root = new URL("../../", import.meta.url);
  for (const [id, term] of Object.entries(TERMS)) {
    assert.ok((KINDS as readonly string[]).includes(term.kind), `${id}: kind`);
    assert.equal(term.says.length, LANGS.length, `${id}: one text per language`);
    term.says.forEach((words, i) => {
      assert.ok(words.trim().length > 0, `${id}: ${LANGS[i]} is empty`);
      if (i > 0) assert.notEqual(words, term.says[0], `${id}: ${LANGS[i]} is the English`);
    });
    // Short: one or two lines in the card.
    assert.ok(term.says[0].length <= 180, `${id}: ${term.says[0].length} characters`);
    assert.ok(term.known.length > 0 && term.known.every((p) => (PARTIES as readonly string[]).includes(p)), `${id}: parties`);
    assert.equal(new Set(term.known).size, term.known.length, `${id}: a party twice`);
    if (term.kind === "label" || term.kind === "fn" || term.kind === "op") assert.deepEqual([...term.known].sort(), [...PARTIES].sort(), `${id}: public`);
    if (term.kind === "secret" || term.kind === "derived") assert.ok(!term.known.includes("hub") && !term.known.includes("relay"), `${id}: a secret no server knows`);
    for (const rel of term.rel ?? []) assert.ok(TERMS[rel], `${id}: rel ${rel}`);
    assert.ok(analysis.byTerm.has(id), `${id} is on no formula`);
    // The code it was read from.
    const ref = /^([\w./-]+):(\d+)(?:-(\d+))?$/.exec(term.src);
    assert.ok(ref, `${id}: ${term.src}`);
    const file = new URL(ref[1]!, root);
    assert.ok(existsSync(file), `${id}: ${ref[1]} exists`);
    const lines = readFileSync(file, "utf8").split("\n").length;
    const [from, to] = [Number(ref[2]), Number(ref[3] ?? ref[2])];
    assert.ok(from >= 1 && from <= to && to <= lines, `${id}: ${term.src} is inside the file (${lines} lines)`);
  }
});

test("a symbol that means different things is read per occurrence: e, S and signer", () => {
  const terms = (block: BlockId, line: number, text: string) => analysis.lines[block][line]!.filter((t) => t.text === text).map((t) => t.term);
  assert.deepEqual(terms("seal", 0, "e"), ["seal.e-priv", "seal.e-pub"]);
  assert.deepEqual(terms("seal", 0, "S"), ["seal.S-priv", "seal.S-pub"]);
  assert.deepEqual(terms("seal", 0, "R"), ["seal.R", "seal.R", "seal.R"]);
  assert.deepEqual(terms("chain", 1, "signer"), ["chain.signer-key", "chain.signer"]);
  assert.deepEqual(terms("head", 0, "host"), ["host.key"]);
  assert.deepEqual(terms("noise", 1, "host"), ["id.host"]);
  assert.deepEqual(terms("confirm", 0, "new_dh"), ["rot.new-dh-priv"]);
  assert.deepEqual(terms("rotate", 0, "new_dh"), ["rot.new-dh"]);
  // Every per-occurrence list is exactly as long as the symbol's occurrences in its line.
  for (const id of blockIds) {
    (BLOCKS[id] as FormulaBlock).lines.forEach((line, li) => {
      if (typeof line === "string") return;
      for (const [text, meaning] of Object.entries(line.terms ?? {})) {
        if (typeof meaning === "string") continue;
        assert.equal(analysis.lines[id][li]!.filter((t) => t.text === text).length, meaning.length, `${id}:${li} ${text}`);
      }
    });
  }
  // The Noise suite is split at its underscores, its parts the functions they name.
  assert.deepEqual(
    analysis.lines.noise[0]!.filter((t) => t.term).map((t) => [t.text, t.term]),
    [
      ["Noise", "noise.Noise"],
      ["IK", "noise.IK"],
      ["25519", "fn.x25519"],
      ["AESGCM", "fn.aesgcm"],
      ["SHA256", "fn.sha256"],
    ],
  );
});

test("the flow is read off the formula: kek is made by HKDF-SHA256 and taken by AES-256-GCM", () => {
  assert.deepEqual(flowOf(analysis, "seal.kek"), { madeBy: ["HKDF-SHA256"], usedBy: ["AES-256-GCM"] });
  assert.deepEqual(flowOf(analysis, "seal.ctx"), { madeBy: [], usedBy: ["HKDF-SHA256", "AES-256-GCM"] });
  assert.deepEqual(flowOf(analysis, "stream.SK"), { madeBy: ["HKDF-SHA256"], usedBy: ["AES-256-GCM"] });
  assert.deepEqual(flowOf(analysis, "chain.signature"), { madeBy: ["Ed25519"], usedBy: [] });
  const [kekDef, kekUse] = analysis.byTerm.get("seal.kek")!;
  assert.ok(analysis.occurrences.get(kekDef!)!.def && !analysis.occurrences.get(kekUse!)!.def);
  const lit = lights(analysis, kekUse!);
  assert.equal(lit.get(kekDef!), "def");
  assert.equal(lit.get(kekUse!), "same");
  const at = (block: BlockId, line: number, text: string) => analysis.lines[block][line]!.find((t) => t.text === text)!.occ!;
  assert.equal(lit.get(at("seal", 0, "HKDF-SHA256")), "flow", "made by");
  assert.equal(lit.get(at("seal", 1, "AES-256-GCM")), "flow", "taken by");
  assert.equal(lit.get(at("stream", 0, "HKDF-SHA256")), undefined, "another HKDF, not this value's");
  // A call lights what it takes and what it makes.
  const hkdf = lights(analysis, at("seal", 0, "HKDF-SHA256"));
  assert.equal(hkdf.get(kekDef!), "flow");
  assert.equal(hkdf.get(at("seal", 0, "ctx")), "flow");
  assert.equal(hkdf.get(at("stream", 0, "HKDF-SHA256")), "same", "every HKDF-SHA256 on the page");
  // The same value across formulas: the account id everywhere it is.
  assert.equal(analysis.byTerm.get("id.account")!.length, 10);
  // A label in the list of 8.2 is the one in the formula.
  assert.deepEqual(new Set(analysis.byTerm.get("label.chain-entry")!.map((o) => o.split(":")[0])), new Set(["chain", "rotate", "labels"]));
});

test("the formulas still read as one line of monospace each, and copy as the plain text they were", () => {
  const html = draw("en");
  for (const id of blockIds) {
    const start = html.indexOf(`data-formula="${id}"`);
    assert.ok(start > 0, id);
    const block = html.slice(start, html.indexOf("</div></div>", start));
    const codes = [...block.matchAll(/<code class="block">([\s\S]*?)<\/code>/g)].map((m) => decode(m[1]!));
    assert.deepEqual(codes, (BLOCKS[id] as FormulaBlock).lines.map(lineText), id);
  }
  // Highlights and hiding change no box: no padding, margin, border or size.
  for (const cls of [...Object.values(LIT_CLASS), HIDDEN_CLASS, ...Object.values(KIND_CLASS)]) {
    assert.doesNotMatch(cls, /(^|\s)(p|m|px|py|mx|my|border|text-(xs|sm|base|lg|2xs))(-|\s|$)/, cls);
  }
});

test("every part of every formula is focusable and described, in every language", () => {
  for (const lng of LANGS) {
    const html = draw(lng);
    const terms = [...html.matchAll(/<span role="button" tabindex="(0|-1)" data-occ="([^"]+)" data-term="([^"]+)"[^>]*aria-describedby="([^"]+)"/g)];
    assert.equal(terms.length, analysis.occurrences.size, `${lng}: every token`);
    // One tab stop per formula; the arrow keys move inside it.
    const stops = terms.filter((m) => m[1] === "0").map((m) => m[2]!.split(":")[0]);
    assert.deepEqual(stops, blockIds, `${lng}: one tab stop per formula, its first part`);
    for (const [, , occ, term, described] of terms) {
      assert.equal(analysis.occurrences.get(occ!)!.term, term);
      const id = described!;
      const at = html.indexOf(`id="${id}"`);
      assert.ok(at > 0, `${lng}: ${id} is on the page`);
      const text = decode(html.slice(html.indexOf(">", at) + 1, html.indexOf("</span>", at)));
      assert.ok(text.includes(TERMS[term!]!.says[LANGS.indexOf(lng)]!), `${lng}: ${term} described in its words`);
    }
    // The descriptions are never shown, and the hint for the whole thing is there.
    assert.match(html, /<div hidden="" data-formula-glossary="">/);
    assert.match(html, /aria-describedby="fx-hint"/);
    // One live region for "view as", silent until the choice changes.
    assert.equal([...html.matchAll(/aria-live=/g)].length, 1, `${lng}: one live region`);
    assert.match(html, /<span aria-live="polite" class="sr-only" data-formula-announcer=""><\/span>/);
  }
  // The reader's own words, not English, around the protocol's names.
  const zh = decode(draw("zh-CN"));
  assert.ok(zh.includes("由 HKDF-SHA256 产生。"));
  assert.ok(zh.includes("知道它的："));
  assert.ok(decode(draw("en")).includes("Made by HKDF-SHA256. Used by AES-256-GCM. Known to: Your device, The other device. Not known to: Droppen Hub, Relay."));
});

test("a formula's parts and the explained words stay selectable, so formulas and paragraphs copy whole: no user-select: none rule reaches them", () => {
  const css = readFileSync(new URL("../src/index.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const doc = new JSDOM('<span role="button" data-occ="seal:0:0">kek</span><span role="button">x</span><button>y</button><span role="button" data-occ="c:host:r1" data-concept="host">device that runs AI</span>').window.document;
  const [part, control, button, mention] = [...doc.body.children];
  const none = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].filter((m) => /(?:^|;)\s*(?:-webkit-)?user-select:\s*none/.test(m[2]!));
  assert.ok(none.length > 0, "the rule for controls is still there");
  for (const [, selectors] of none) {
    assert.ok(!part!.matches(selectors!.trim()), `${selectors!.trim()} makes a formula part unselectable`);
    assert.ok(!mention!.matches(selectors!.trim()), `${selectors!.trim()} makes a mention unselectable: a paragraph would copy without it`);
  }
  // The rule still reaches what it is for.
  assert.ok(none.some(([, sel]) => control!.matches(sel!.trim())) && none.some(([, sel]) => button!.matches(sel!.trim())));
});

test("the two kinds of device are defined at the top, and every later mention explains itself, in every language", () => {
  for (const lng of LANGS) {
    const html = draw(lng);
    const doc = new JSDOM(html).window.document;
    const defs = doc.querySelector('[data-testid="security-terms"]');
    assert.ok(defs, `${lng}: the definitions`);
    assert.deepEqual([...defs.querySelectorAll("[data-concept-def]")].map((d) => d.getAttribute("data-concept-def")), ["host", "client"]);
    assert.equal(defs.querySelectorAll("dfn").length, 2);
    assert.equal(defs.querySelectorAll("dt").length, 6, `${lng}: what it is, its keys, what it may do`);
    // Before the table of contents and every section: the start of the page.
    const order = [...doc.querySelectorAll('[data-testid="security-terms"], [data-testid="security-contents"], section[id]')].map((e) => e.getAttribute("data-testid") ?? e.id);
    assert.equal(order[0], "security-terms", lng);

    // Every mention outside the definitions: wrapped, focusable, described.
    const unwrapped: string[] = [];
    const walker = doc.createTreeWalker(doc.body, 4 /* SHOW_TEXT */);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const parent = node.parentElement!;
      // Not inside a word already explained, the definitions themselves, the glossary, a formula, a link (the
      // table of contents: no control inside a link) or the overview figure's drawing (an image with its own label).
      if (parent.closest("[data-concept], [data-concept-def], [hidden], [data-formula], a, svg")) continue;
      for (const m of node.textContent!.matchAll(new RegExp(CONCEPT_WORDS))) unwrapped.push(m[0]);
    }
    assert.deepEqual(unwrapped, [], `${lng}: a mention that does not explain itself`);
    const wrapped = [...doc.querySelectorAll<HTMLElement>("[data-concept]")];
    const count = (id: string) => wrapped.filter((w) => w.dataset.concept === id).length;
    assert.ok(count("host") >= 40 && count("client") >= 8, `${lng}: ${count("host")} and ${count("client")} mentions`);
    // One tab stop per word in each section: its first mention there; the arrow keys reach the others.
    for (const section of doc.querySelectorAll("main section[id]")) {
      for (const id of ["host", "client"]) {
        const here = [...section.querySelectorAll(`[data-concept="${id}"]`)];
        if (here.length === 0) continue;
        assert.deepEqual(here.map((w) => w.getAttribute("tabindex")), ["0", ...here.slice(1).map(() => "-1")], `${lng}: #${section.id} ${id}`);
      }
    }
    for (const w of wrapped) {
      assert.equal(w.getAttribute("role"), "button");
      assert.ok(w.textContent!.match(new RegExp(CONCEPT_WORDS)), w.textContent!);
      const described = doc.getElementById(w.getAttribute("aria-describedby")!);
      assert.ok(described && described.textContent!.length > 20, `${lng}: described`);
      assert.equal(w.closest("a"), null, "never inside a link");
    }
    // What the left-out places hold: only the table of contents' links and the drawing.
    for (const el of doc.querySelectorAll("a, svg")) {
      if (!new RegExp(CONCEPT_WORDS).test(el.textContent!)) continue;
      assert.ok(el.closest('[data-testid="security-contents"]') || el.closest("svg[role=img]"), `${lng}: ${el.outerHTML.slice(0, 80)}`);
    }
  }
});
