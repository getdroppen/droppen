// The select every picker in the app is now (`components/ui/select.tsx`),
// driven in a real DOM: open it by pointer and by key, pick, move with the
// arrows and by typing, leave with Escape, and refuse what is disabled. The
// native `<select>` it replaced drew the system popup (a blue highlight, a
// rounded menu); what is pinned here is that ours behaves like one and draws
// the drafting sheet instead.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, document, container } = installJsdom();
// jsdom's animation frames (and Base UI's timers) keep the loop alive: once every test has run, go.
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

// After the DOM: react-dom and Base UI look at the page as they load.
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { SelectField } = await import("../src/components/ui/select.js");
const { ComposerSelect, ModelSelect } = await import("../src/features/sessions/composer-controls.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } } });

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  // A popup left mounted by a test would answer the next one's queries.
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((n) => n.remove());
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 20))));
const trigger = () => {
  const found = container.querySelector<HTMLElement>('[role="combobox"]');
  assert.ok(found, "the trigger is on the page");
  return found;
};
// Once focused, a select keeps its list mounted while closed — hidden, for
// typeahead and labels — so "the list" is the one not inside a hidden positioner.
const listbox = () =>
  [...document.querySelectorAll<HTMLElement>('[role="listbox"]')].find((l) => !l.closest("[hidden]")) ?? null;
const options = () => [...(listbox()?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])];
const option = (text: string) => {
  const found = options().find((o) => o.textContent === text);
  assert.ok(found, `an option says ${text}`);
  return found;
};
const highlighted = () => options().find((o) => o.hasAttribute("data-highlighted"))?.textContent;

/** A mouse press and release on `el`, as a browser sends them. */
async function press(el: HTMLElement, pointerType = "mouse"): Promise<void> {
  await act(async () => {
    el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, pointerType, button: 0 }));
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
  await act(async () => {
    el.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerType, button: 0 }));
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
}

/** Move the mouse onto an option and click it. */
async function pick(el: HTMLElement): Promise<void> {
  await act(async () => {
    el.dispatchEvent(new PointerEvent("pointerenter", { bubbles: false, pointerType: "mouse" }));
    el.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, movementX: 1, movementY: 1 } as MouseEventInit));
  });
  await press(el);
}

/** A key pressed on whatever has focus. */
async function key(name: string): Promise<void> {
  const target = (document.activeElement as HTMLElement | null) ?? document.body;
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
  });
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keyup", { key: name, bubbles: true, cancelable: true }));
  });
  await settle();
}

const EFFORTS = ["low", "medium", "high", "xhigh", "max"].map((v) => ({ value: v, label: v }));

/** A field whose value lives in state, as every caller keeps it. */
function Field({ initial = "xhigh", onPick = () => {}, disabled = false, options = EFFORTS }: {
  initial?: string;
  onPick?: (v: string) => void;
  disabled?: boolean;
  options?: { value: string; label: string; disabled?: boolean }[];
}) {
  const [value, setValue] = React.useState(initial);
  return (
    <SelectField
      aria-label="effort"
      value={value}
      options={options}
      disabled={disabled}
      onValueChange={(v) => {
        setValue(v);
        onPick(v);
      }}
    />
  );
}

test("closed, it is a labelled combobox that says the chosen label, and no list is on the page", async () => {
  await render(<Field />);
  const t = trigger();
  assert.equal(t.tagName, "BUTTON");
  assert.equal(t.getAttribute("aria-label"), "effort", "the name a screen reader reads");
  assert.equal(t.getAttribute("aria-expanded"), "false");
  assert.equal(t.getAttribute("aria-haspopup"), "listbox");
  assert.match(t.textContent ?? "", /^xhigh$/);
  assert.equal(listbox(), null);
  assert.equal(container.querySelector("select"), null, "no native select underneath");
});

test("a press opens the list on the drafting sheet: every option, the chosen one checked in ink, nothing blue or rounded", async () => {
  await render(<Field />);
  await press(trigger());
  const list = listbox();
  assert.ok(list, "the list is open");
  assert.equal(trigger().getAttribute("aria-expanded"), "true");
  assert.deepEqual(options().map((o) => o.textContent), ["low", "medium", "high", "xhigh", "max"]);
  const chosen = option("xhigh");
  assert.equal(chosen.getAttribute("aria-selected"), "true");
  assert.ok(chosen.hasAttribute("data-selected"));
  assert.ok(chosen.querySelector("svg"), "the ink check");
  assert.equal(option("low").getAttribute("aria-selected"), "false");
  const sheet = list.closest<HTMLElement>("[class*='border-border-strong']") ?? list;
  assert.match(sheet.className, /border-border-strong/, "a hairline frame");
  assert.match(sheet.className, /\bbg-bg\b/);
  for (const el of [sheet, ...options()]) {
    assert.doesNotMatch(el.className, /blue|accent|rounded|shadow/, "no system blue, no corners, no shadow");
  }
  assert.match(option("low").className, /data-\[highlighted\]:bg-hover/, "the highlight is an ink-tinted row");
});

test("picking an option reports it, closes the list and shows the new label", async () => {
  const picked: string[] = [];
  await render(<Field onPick={(v) => picked.push(v)} />);
  await press(trigger());
  await pick(option("medium"));
  assert.deepEqual(picked, ["medium"]);
  await settle();
  assert.equal(trigger().getAttribute("aria-expanded"), "false");
  assert.match(trigger().textContent ?? "", /^medium$/);
});

test("a finger picks with one tap, without hovering first", async () => {
  const picked: string[] = [];
  await render(<Field onPick={(v) => picked.push(v)} />);
  await press(trigger(), "touch");
  assert.ok(listbox(), "open");
  await press(option("low"), "touch");
  assert.deepEqual(picked, ["low"]);
});

test("the keyboard: arrows open and move, typing jumps, Enter picks, Escape leaves without a change", async () => {
  const picked: string[] = [];
  await render(<Field onPick={(v) => picked.push(v)} />);
  await act(async () => trigger().focus());
  await key("ArrowDown");
  assert.ok(listbox(), "ArrowDown opens the list");
  assert.equal(highlighted(), "xhigh", "on the chosen option");
  await key("ArrowDown");
  assert.equal(highlighted(), "max");
  await key("ArrowUp");
  await key("ArrowUp");
  assert.equal(highlighted(), "high");
  await key("Home");
  assert.equal(highlighted(), "low");
  // Typing searches on from the chosen option, and the same letter again cycles.
  await key("m");
  assert.equal(highlighted(), "max", "typeahead: the next option starting with m");
  await key("m");
  assert.equal(highlighted(), "medium", "and m again, the one after that");
  await key("Escape");
  assert.equal(listbox(), null, "Escape closes");
  assert.deepEqual(picked, [], "and keeps the value");
  assert.match(trigger().textContent ?? "", /^xhigh$/);

  await act(async () => trigger().focus());
  await key("ArrowDown");
  await key("ArrowUp");
  assert.equal(highlighted(), "high");
  await key("Enter");
  assert.deepEqual(picked, ["high"], "Enter picks the highlighted option");
  assert.equal(listbox(), null);
  assert.equal(document.activeElement, trigger(), "focus goes back to the trigger");
});

test("a disabled select does not open, by pointer or by key", async () => {
  await render(<Field disabled />);
  assert.ok(trigger().hasAttribute("disabled") || trigger().hasAttribute("data-disabled"));
  await press(trigger());
  assert.equal(listbox(), null);
  await act(async () => trigger().focus());
  await key("ArrowDown");
  assert.equal(listbox(), null);
});

test("a disabled option is listed but cannot be picked", async () => {
  const picked: string[] = [];
  const options = EFFORTS.map((o) => (o.value === "low" ? { ...o, disabled: true } : o));
  await render(<Field options={options} onPick={(v) => picked.push(v)} />);
  await press(trigger());
  assert.equal(option("low").getAttribute("aria-disabled"), "true");
  await pick(option("low"));
  assert.deepEqual(picked, []);
});

test("a field trigger is 32px, 12px in, 13px sans: hover darkens the border, open inks it, and the chevron turns in 150ms", async () => {
  await render(<Field />);
  const t = trigger();
  assert.match(t.className, /\bh-8\b/, "32px on a desktop");
  assert.match(t.className, /\bpx-3\b/, "12px of side padding");
  assert.match(t.className, /text-\[13px\]/, "a 13px sans value");
  assert.doesNotMatch(t.className, /font-mono/, "the value is not a machine string");
  assert.match(t.className, /hover:border-border-strong/, "hover");
  assert.match(t.className, /data-\[popup-open\]:border-fg/, "open");
  assert.match(t.className, /max-md:h-10/, "40px under a finger");
  assert.match(t.className, /\bgroup\b/, "the icon turns with its open state");
  const icon = t.querySelector("svg")?.parentElement;
  assert.ok(icon, "the chevron");
  assert.match(icon.className, /group-data-\[popup-open\]:rotate-180/);
  assert.match(icon.className, /duration-150/, "150ms");
  assert.match(icon.className, /\[&>svg\]:size-3\.5/, "14px");
});

test("a disabled field is a fixed grey box, not the whole control faded", async () => {
  await render(<Field disabled />);
  const t = trigger();
  assert.match(t.className, /disabled:bg-raised/);
  assert.match(t.className, /disabled:text-faint/);
  assert.match(t.className, /disabled:border-border/);
  assert.doesNotMatch(t.className, /disabled:opacity-40/);
});

test("the menu is 32px rows (44 touch), one hover fill, and a raised, 500-weight chosen row with a 14px check", async () => {
  await render(<Field />);
  await press(trigger());
  const list = listbox();
  assert.ok(list);
  assert.match(list.className, /max-h-\[min\(var\(--available-height\),20rem\)\]/);
  assert.match(list.className, /min-w-\[max\(var\(--anchor-width\),10rem\)\]/);
  assert.match(list.className, /max-w-\[min\(var\(--available-width\),20rem\)\]/);
  assert.match(list.className, /\bpy-1\.5\b/, "6px above the first row");
  assert.match(list.className, /popup-motion/, "the 120ms fade and 2px rise");
  assert.match(list.className, /text-\[13px\]/);
  const row = option("xhigh");
  assert.match(row.className, /\bmin-h-8\b/, "32px rows");
  assert.match(row.className, /pointer-coarse:min-h-11/, "44px under a finger");
  assert.match(row.className, /\bpx-3\b/, "12px in");
  assert.match(row.className, /data-\[highlighted\]:bg-hover/, "one fill for pointer and keyboard");
  assert.match(row.className, /data-\[selected\]:bg-raised/, "the chosen row is a well");
  assert.match(row.className, /data-\[selected\]:font-medium/, "and 500 weight");
  const check = row.querySelector("svg");
  assert.ok(check, "the ink check");
  assert.match(check.getAttribute("class") ?? "", /size-3\.5/, "14px");
  assert.equal(options().filter((o) => o.hasAttribute("data-selected")).length, 1, "one chosen row");
});

test("an empty list says so once, in muted 13px", async () => {
  await render(<SelectField aria-label="empty" value="" options={[]} onValueChange={() => {}} />);
  await press(trigger());
  const list = listbox();
  assert.ok(list, "the list still opens");
  assert.equal(list.querySelectorAll('[role="option"]').length, 0);
  assert.equal(list.textContent, "no options");
});

test("the composer's chip keeps its look — borderless, 24px, muted mono — and still picks", async () => {
  const picked: string[] = [];
  function Chip() {
    const [value, setValue] = React.useState("xhigh");
    return (
      <div data-composer="">
        <ComposerSelect ghost label="effort" value={value} options={EFFORTS.map((o) => o.value)} onChange={(v) => { setValue(v); picked.push(v); }} />
      </div>
    );
  }
  await render(<Chip />);
  const t = trigger();
  assert.match(t.className, /\bh-6\b/);
  assert.match(t.className, /font-mono/);
  assert.match(t.className, /text-2xs/);
  assert.match(t.className, /text-muted/);
  assert.doesNotMatch(t.className, /\bborder\b/, "a chip has no frame");
  assert.equal(t.getAttribute("aria-label"), "effort");
  assert.match(t.textContent ?? "", /^xhigh$/);
  await press(t);
  await pick(option("low"));
  assert.deepEqual(picked, ["low"]);
});

test("a permission that drops the guard rails stays drawn in the danger colour, as a signal", async () => {
  await render(<ComposerSelect ghost danger label="permission" value="bypassPermissions" options={["default", "bypassPermissions"]} onChange={() => {}} />);
  const t = trigger();
  assert.equal(t.getAttribute("data-signal"), "danger");
  assert.match(t.className, /text-danger/);
  assert.match(t.className, /bg-danger\/10/);
});

test("the model picker's last entry opens the box to type an id into", async () => {
  const models = ["opus", "sonnet"].map((id) => ({ id, label: null, efforts: ["low"], defaultEffort: "low" }));
  await render(<ModelSelect ghost models={models} value="opus" customId={null} onChange={() => {}} />);
  assert.equal(trigger().getAttribute("aria-label"), "model");
  await press(trigger());
  assert.deepEqual(options().map((o) => o.textContent), ["opus", "sonnet", "custom…"], "last, after every model");
  await pick(option("custom…"));
  const box = container.querySelector<HTMLInputElement>('input[aria-label="custom model id"]');
  assert.ok(box, "the box to type into");
  assert.equal(box.value, "", "empty: a listed model's name is already there");
  await settle();
  assert.equal(document.activeElement, box, "focused, even though the list handed focus back as it closed");
  assert.ok(container.querySelector('input[aria-label="custom model id"]'), "and still open");
});

test("a form's leading entry comes first, and a model the list does not carry is listed, marked and checked", async () => {
  const models = ["opus", "sonnet"].map((id) => ({ id, label: null, efforts: ["low"], defaultEffort: "low" }));
  await render(
    <ModelSelect models={models} value="GLM-4.6" customId="GLM-4.6" onChange={() => {}} leading={{ value: "", label: "CLI default" }} />,
  );
  await press(trigger());
  const labels = options().map((o) => o.textContent);
  assert.equal(labels[0], "CLI default", "before the models, not after");
  assert.equal(labels.at(-1), "custom…", "the box stays last");
  assert.ok(labels.includes("GLM-4.6 · custom"));
  assert.equal(option("GLM-4.6 · custom").getAttribute("aria-selected"), "true", "the chosen one is the model itself");
});

test("an empty value picks the form's leading entry, and choosing it reports \"\"", async () => {
  const picked: string[] = [];
  function Form() {
    const [value, setValue] = React.useState("high");
    return (
      <label>
        <span>effort</span>
        <SelectField value={value} options={[{ value: "", label: "CLI default" }, ...EFFORTS]} onValueChange={(v) => { setValue(v); picked.push(v); }} />
      </label>
    );
  }
  await render(<Form />);
  assert.equal(trigger().closest("label")?.textContent?.startsWith("effort"), true, "the form's label wraps the trigger, and names it");
  await press(trigger());
  await pick(option("CLI default"));
  assert.deepEqual(picked, [""]);
  assert.match(trigger().textContent ?? "", /^CLI default$/);
});
