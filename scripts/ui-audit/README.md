# UI audit

The executable half of the `ui-consistency` skill (`.claude/skills/ui-consistency/SKILL.md`).
It renders the real components in a fixture with the app's own Vite/Tailwind
setup, measures computed styles and boxes with Playwright, and captures
light/dark screenshots at desktop and phone widths.

The point is evidence, not a screenshot scrapbook: `className` is not the
rendered result (`cn()` merges, `rem` follows the font-size preference, phones
override form text to 16px). Every claim below is a computed value or a
`getBoundingClientRect()`.

## Run

```sh
pnpm install --frozen-lockfile
pnpm --filter @droppen/e2e exec playwright install chromium   # once
pnpm ui-audit                     # report + screenshots (both fixtures)
pnpm ui-audit --check             # fail on a spec violation (the gate)
pnpm ui-audit --fixture devices   # only the devices page
```

Options:

- `--out <dir>` (default `/tmp/droppen-ui-audit-<date>`)
- `--fixture settings|devices|agents|new-chat|cloud-bar|session-opening|no-copy|both` (default `both`)
- `--theme light|dark|both`, `--viewport desktop|mobile|both`
- `--channel chrome` — use a system Chrome instead of the bundled Chromium
- `--url <url>` — measure a running page instead of the fixture (no target
  checks; useful for a local instance, never a production account)
- `--check` — exit 1 on the first target invariant that fails

Outputs in `--out`: `measurements.json`, `report.txt`, and
`settings-<theme>-<viewport>.png`, `select-open-…`, `select-empty-…`, `devices-<theme>-<viewport>.png`, `devices-hover-…`.
A live page measured with `--url` is captured from the top in viewport-sized
steps: `live-<viewport>.png`, `live-<viewport>-2.png`, … . The app scrolls a
container inside a fixed body, so a full-page screenshot is mostly blank below
the fold (the `/security` page came out 1440×20602) and too tall for an image
viewer or a model.
For a UI change, keep the before and after directories and compare the same
state, theme, viewport and scroll position.

## What `--check` enforces

The invariants live in `docs/ui-spec.md`; the runner encodes the ones a machine
can see:

- a visible 2px focus ring on text fields and on the active segmented tab;
- field/trigger heights at the settings scale (32px desktop, 40px phone);
- values at 13px sans (mono only for ids/paths/model ids);
- one control-column width across the settings rows (spread ≤ 1px);
- select: border change on hover, chevron rotated while open, row height,
  selected row visually distinct from a normal one, a check, an empty-state
  sentence;
- no border radius and no drop shadow on controls.

The `devices` fixture adds: two or more card columns at desktop, one 1px card
frame, 16px card padding, a card at least 88px tall, a hover that changes the
frame and underlines the name, no row-wide background fill on hover, and — at
phone width — one column of 56px rows with no card frame.

The `cloud-bar` fixture adds, for each state of the cloud bar's connection
(the refusals included), in English, zh-CN, zh-TW and ja: a 28px strip with
the real device switcher drawn, as for an account of two devices (the fixture
runs under a router and puts a shell with two devices in the registry's
slot); words for every state but live, which shows the path indicator; the
English words themselves; neither the words nor the device's name truncated
(no ellipsis) at either width, each state drawn at its widest (two-digit
countdowns, the longest relay city); and, between tries, a countdown that is
a `<button>` with an `aria-label` and the 2px focus ring, never says "0 s",
and stays whole inside the strip. Outputs `cloud-bar-<theme>-<viewport>.png`
(every state, as tall as the fixture) and `cloud-bar-<lang>-…` for each other
language, one `cloud-bar-<state>-…` per state and `cloud-bar-focus-…`; each
state's free space in the strip is in `measurements.json`.

Its catch-up states (`&states=catch-up`: the offline copy catching up from the
hub — the session list, sessions 3/7, and sessions 3/7 · 42 %, the longest —
drawn the same way and held to the same checks) add, in the same four
languages: the catch-up's words themselves, whole (no ellipsis) at either
width, where they belong — in the offline notice after the copy's time at
desktop width and on a line of their own at phone width; with no notice, in
the strip after its words at desktop width and on a line of their own under
the strip at phone width — with a tooltip for the counts, and the retry
countdown beside them whole. Outputs `cloud-bar-catch-up-<theme>-<viewport>.png`
and `cloud-bar-catch-up-<lang>-…` for each other language, and, in English and
zh-CN, one `cloud-bar-catch-up-<state>[-zh-CN]-…` per state with what is under
the strip; each state's free space after the words is in `measurements.json`.

The `session-opening` fixture (a session being opened with nothing to show
yet, `features/sessions/session-opening.tsx`: connecting, waiting for the
server's answer, the offline copy downloading from the hub with its pages and
without, retrying after `closed`) adds, in English, zh-CN, zh-TW and ja: the
spinner in the middle of the view, where it always was; one line under it
saying the stage's words (checked word for word in every language), mono at
11px, whole (no ellipsis, no clip) inside the view's 24px padding at either
width; the thin bar, 2px and square, filled to its share, only for the
copy's pages; and nothing below the view. Outputs
`session-opening-<theme>-<viewport>.png` and `session-opening-<lang>-…` for
each other language, and, in English and zh-CN, one
`session-opening-<state>[-zh-CN]-…` per stage, with the spinner held still;
each stage's free space beside its line is in `measurements.json`.

The `no-copy` fixture is the foot of a session with no copy on this device yet:
the session composer's box, read-only, its placeholder the reason the offline
copy gives — the device offline (only when the hub says so), or only the copy
missing (online, or not said yet). In English, zh-CN, zh-TW and ja: the words
themselves, the box disabled with its 1px square frame, and the reason whole
in the box at either width, no line of it cut off (a phone sets form text at
16px, and the longest take two lines there). Outputs `no-copy-<theme>-<viewport>.png` and
`no-copy-<lang>-…` for each other language, the fixture alone.

The fixture deliberately exercises the target state, so on a revision that
still has the old controls these checks fail. That is the point: run it before
and after, and do not merge with a failing check.

## The fixture

`web/ui-audit/main.tsx` has two fixtures:

- `settings` imports the real `SelectField`, `Input`, `Tabs`, `Button`,
  `Switch` and `Checkbox` and reproduces the settings-row composition.
- `devices` imports the real `DeviceRow` and `SectionHead` and renders five
  fixed views (this host, an online host, an offline host with a copy, a
  browser, one waiting for approval) in the real card grid.

Neither is a product page and neither makes requests.

- `web/src/index.css` excludes it with `@source not "../ui-audit"`: the fixture
  must never change the production stylesheet or its hash. Every class in the
  fixture therefore has to be one that already occurs in `web/src`; copy the
  strings from the real page.
- When a change touches a surface the fixture does not render, extend the
  fixture first (the real composition, same classes, `data-audit-row` for a
  row), then audit. A page that cannot be composed without the app context can
  be measured with `--url` against a local instance.

## Evidence level

Fixture = the real components and the real JSX composition, with no API,
router, machine context or live data. It proves component and row geometry,
computed typography, and interaction states. It does not prove a live page's
data-dependent layout (long names, empty lists, many devices) — those need a
state fixture or a local instance.
