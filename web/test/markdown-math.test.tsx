// LaTeX in an agent's reply (getdroppen/droppen#141).
//
// What is pinned here: which text is math (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`)
// and which stays as written (code of every kind, prices, shell variables in
// prose, TeX KaTeX cannot read); that the pref turns it off; and that the TeX
// is typeset with nothing in it that could run or navigate. `react-dom/server`
// rather than a DOM, like the other render tests, and the math kit installed
// by hand: there is no bundler here to fetch its chunk and stylesheet.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { installMath, Markdown, MathRendering } from "../src/components/markdown.js";
import * as kit from "../src/components/markdown-math.js";
import { makeI18n } from "./render.js";

const { prepareMath } = kit;

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } },
  keySeparator: false,
  nsSeparator: false
});

function draw(text: string, on = true): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <MathRendering.Provider value={on}>
        <Markdown text={text} />
      </MathRendering.Provider>
    </I18nextProvider>,
  );
}

/** How many formulas KaTeX drew. */
const formulas = (html: string) => html.match(/<span class="katex">/g)?.length ?? 0;

/* ── before the kit arrives ─────────────────────────────────────────────── */

// First, while nothing has installed the kit: a message is drawn as written
// until KaTeX has loaded, never blank.
test("until KaTeX has loaded, a formula is the text the agent wrote", () => {
  const html = draw("energy: $E = mc^2$");
  assert.equal(formulas(html), 0);
  assert.match(html, /\$E = mc\^2\$/);
});

/* ── the delimiters ─────────────────────────────────────────────────────── */

test("\\( \\) and \\[ \\] become the dollars the parser reads", () => {
  assert.equal(prepareMath("so \\(E = mc^2\\) holds"), "so $E = mc^2$ holds");
  assert.equal(prepareMath("so \\( x \\) holds"), "so $x$ holds", "inner spaces trimmed, or Pandoc's rule would refuse it");
  assert.equal(prepareMath("\\[ a^2 + b^2 = c^2 \\]"), "$$a^2 + b^2 = c^2$$");
  assert.equal(prepareMath("\\[\n\\sum_i x_i\n\\]"), "$$\n\\sum_i x_i\n$$", "own lines: a block in place");
  assert.equal(
    prepareMath("1. the sum:\n   \\[\n   \\sum_i x_i\n   \\]\n2. next"),
    "1. the sum:\n   $$\n   \\sum_i x_i\n   $$\n2. next",
    "the list's indent kept, so the block stays in its item",
  );
  assert.equal(prepareMath("> \\[\n> x\n> \\]"), "> $$\n> x\n> $$", "and a quote's markers");
  assert.equal(
    prepareMath("\\[ a = b\n   + c \\]"),
    "$$a = b + c$$",
    "text beside a delimiter: folded onto one line, so no `$$` opens a block that never closes",
  );
});

test("code, escapes and unclosed delimiters are left as they are", () => {
  for (const text of [
    "`\\(x\\)` in code",
    "``a ` \\(x\\)`` in a longer run",
    "```\n\\(x\\)\n\\[y\\]\n```",
    "~~~tex\n\\[y\\]\n~~~",
    "   ```\n   \\(x\\)\n   ```",
    "\\\\(x\\\\) is two escaped backslashes",
    "\\(never closed",
    "\\(\\) is empty",
    "\\(across\n\na paragraph\\)",
    "no math here at all",
  ]) {
    assert.equal(prepareMath(text), text, text);
  }
  assert.equal(
    prepareMath("```\n\\(x\\)\n```\nafter \\(y\\)"),
    "```\n\\(x\\)\n```\nafter $y$",
    "the fence closes, and what follows it is prose again",
  );
});

/* ── typeset ────────────────────────────────────────────────────────────── */

test("the kit installed, every kind of delimiter is typeset", () => {
  installMath(kit);
  for (const text of ["$E = mc^2$", "\\(E = mc^2\\)", "$$E = mc^2$$", "\\[E = mc^2\\]", "$$\nE = mc^2\n$$"]) {
    const html = draw(text);
    assert.equal(formulas(html), 1, text);
    assert.doesNotMatch(html, /\$|\\\(|\\\[/, `${text}: no delimiter left on the page`);
  }
});

test("$…$ is inline; $$…$$ and \\[…\\] are displayed, as a block when alone", () => {
  const inline = draw("a $x^2$ b");
  assert.match(inline, /<p[^>]*>a <span class="math math-inline" data-tex="x\^2" role="button" tabindex="0" title="click to copy the TeX"><span class="katex">/);
  assert.doesNotMatch(inline, /katex-display/);

  for (const text of ["$$x^2$$", "\\[x^2\\]", "$$\nx^2\n$$"]) {
    const html = draw(text);
    assert.match(html, /^<div[^>]*><div class="math math-display" data-tex="x\^2"[^>]*><span class="katex-display">/, text);
    assert.doesNotMatch(html, /<p/, `${text}: a block, not a paragraph around one`);
  }

  // Inside a sentence it is still displayed, as LaTeX would: a block-level
  // span in the paragraph, never a <div> inside a <p>.
  const mid = draw("so $$x^2$$ holds");
  assert.match(mid, /<p[^>]*>so <span class="math math-display"[^>]*><span class="katex-display">/);
});

test("code is never typeset, ```math included", () => {
  const html = draw("`$x$` and\n\n```\n$$y$$\n```\n\n```math\nz^2\n```\n\n```tex\n\\[w\\]\n```");
  assert.equal(formulas(html), 0);
  assert.match(html, /<code[^>]*>\$x\$<\/code>/);
  assert.match(html, /\$\$y\$\$/);
  assert.match(html, /<code class="block[^"]*language-math[^"]*">z\^2/);
  assert.match(html, /\\\[w\\\]/);
});

test("a $ that would pair but is not math is escaped before parsing", () => {
  assert.equal(prepareMath("$5 and $10"), "\\$5 and $10", "the last has nothing to pair with: as is");
  assert.equal(prepareMath("$5-$10"), "\\$5-$10");
  assert.equal(prepareMath("$5 and $x$"), "\\$5 and $x$", "the price escaped, the formula after it kept");
  assert.equal(prepareMath("a $x$ b"), "a $x$ b");
  assert.equal(prepareMath("$5 (or `$6`)"), "\\$5 (or `$6`)", "a backtick between: not math, and the code untouched");
  assert.equal(prepareMath("$$x$$ costs $5"), "$$x$$ costs $5");
  assert.equal(prepareMath("\\$x\\$ escaped"), "\\$x\\$ escaped");
  assert.equal(prepareMath("$5 today\n\n$6 tomorrow"), "$5 today\n\n$6 tomorrow", "never paired across a paragraph");
});

// From a real render: the parser paired `$10 … $` and swallowed the code span
// between them, and turning the pair back into text could not bring it back.
test("Markdown between two prices is still Markdown", () => {
  const code = draw("A100s cost about $2 an hour, so $5 to $10 covers it. Run `echo $CUDA_VISIBLE_DEVICES` to check.");
  assert.equal(formulas(code), 0);
  assert.match(code, /<code[^>]*>echo \$CUDA_VISIBLE_DEVICES<\/code>/);
  assert.match(code, /about \$2 an hour, so \$5 to \$10 covers it/);

  const bold = draw("costs $5 for **pro** and $10 for _team_");
  assert.match(bold, /<strong>pro<\/strong>/);
  assert.match(bold, /<em>team<\/em>/);

  const both = draw("$5 and then $x^2$");
  assert.equal(formulas(both), 1, "the formula after a price still typesets");
  assert.match(both, /\$5 and then <span class="math math-inline"/);
});

test("prices and shell variables in prose stay text", () => {
  for (const text of [
    "It costs $5 and $10.",
    "between $5-$10 a month",
    "$5/$10",
    "set $HOME and $PATH first",
    "US$5 and US$6",
    "a lone $ sign",
    "escaped \\$x\\$ dollars",
  ]) {
    const html = draw(text);
    assert.equal(formulas(html), 0, text);
  }
  assert.match(draw("It costs $5 and $10."), /It costs \$5 and \$10\./, "and read exactly as written");
});

test("TeX KaTeX cannot read is left as written", () => {
  const html = draw("see $\\notamacro{x}$ here");
  assert.equal(formulas(html), 0);
  assert.match(html, /<span class="math-error">\$\\notamacro\{x\}\$<\/span>/);
});

test("text in another script inside math is drawn, not refused", () => {
  assert.equal(formulas(draw("$速度 = d/t$")), 1);
  assert.equal(formulas(draw("$\\text{距离} = vt$")), 1);
});

test("nothing in a formula links, loads or runs: such a formula is shown as written", () => {
  const html = draw(
    "$\\href{javascript:alert(1)}{x}$ $\\url{https://example.com}$ $\\includegraphics{https://example.com/a.png}$ $\\htmlClass{x}{y}$ $\\htmlStyle{color:red}{y}$",
  );
  assert.doesNotMatch(html, /<a |href=|<img |src=/);
  assert.equal(formulas(html), 0);
  assert.match(html, /<span class="math-error">\$\\href\{javascript:alert\(1\)\}\{x\}\$<\/span>/, "the argument still in view");
  assert.equal(html.match(/class="math-error"/g)?.length, 5);
});

test("a huge size is capped rather than drawn", () => {
  const html = draw("$\\rule{100000em}{1em}$");
  assert.equal(formulas(html), 1);
  assert.match(html, /border-right-width:20em/);
  assert.doesNotMatch(html, /style="[^"]*100000em/);
});

/* ── the pref ───────────────────────────────────────────────────────────── */

test("with the pref off, or outside a view that read it, a message is as written", () => {
  for (const html of [draw("$E = mc^2$ and \\(x\\)", false), renderToStaticMarkup(<Markdown text="$E = mc^2$ and \(x\)" />)]) {
    assert.equal(formulas(html), 0);
    assert.match(html, /\$E = mc\^2\$ and \(x\)/, "the text as CommonMark reads it");
  }
});
