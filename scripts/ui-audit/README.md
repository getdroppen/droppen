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
pnpm --filter @remote-hand/e2e exec playwright install chromium   # once
pnpm ui-audit                     # report + screenshots (both fixtures)
pnpm ui-audit --check             # fail on a spec violation (the gate)
pnpm ui-audit --fixture devices   # only the devices page
```

Options:

- `--out <dir>` (default `/tmp/droppen-ui-audit-<date>`)
- `--fixture settings|devices|both` (default `both`)
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
