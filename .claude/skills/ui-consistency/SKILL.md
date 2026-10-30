---
name: ui-consistency
description: Verify a Droppen web UI change against docs/ui-spec.md with the in-repo fixture, measurements and light/dark screenshots. Use before and after any change under web/src that paints, sizes, spaces or aligns UI.
---

# Droppen UI consistency

The rule: **a UI change is not done until `pnpm ui-audit --check` passes and
the before/after screenshots have been looked at.** Measurements come from the
real components rendered through the app's own Vite/Tailwind setup, never from
class names: `cn()` merges classes, `rem` follows the font-size preference, and
phones override form text to 16px.

## Workflow

1. Read `docs/ui-spec.md` and the component and callers you are about to touch.
2. Record the baseline:
   `pnpm ui-audit --out /tmp/ui-before`
   (add `--theme` / `--viewport` if you deliberately narrow the run).
3. Make the change. Keep the same fixture, themes, viewports, font size and
   scroll position for the comparison.
4. Verify:
   `pnpm ui-audit --check --out /tmp/ui-after`
   A failure is a blocker; `actual` and `expected` in `report.txt` say why.
5. Look at the screenshots (`settings-<theme>-<viewport>.png`,
   `select-open-…`, `select-empty-…`) in both themes at 390 and 1440. Equal
   numbers can still read wrong.
6. Run the web tests and typecheck:
   `pnpm --filter @remote-hand/web typecheck && pnpm --filter @remote-hand/web test`.
7. Report the evidence: the command, the pass/fail line, and which screenshots
   you compared. Do not call a change verified from source reading alone.

## If the fixture does not cover the surface

Extend `web/ui-audit/main.tsx` first: import the real component(s) and copy the
real composition and class strings from the page. Today it has `settings` and
`devices` fixtures; run both (the default) unless the change is scoped to one. Add `data-audit-row` for a
row the checker should measure. The fixture is excluded from the production
Tailwind scan (`@source not` in `web/src/index.css`), so every class must
already occur in `web/src`.

For a data-dependent page (long lists, many devices, empty/error states) that
cannot be composed without the app context, run the audit against a **local**
instance with `pnpm ui-audit --url http://127.0.0.1:<port>/<route>`; never
against a production account.

## Rules

- A new font size is a spec change. Use the named steps in `docs/ui-spec.md`:
  Tailwind's default `text-xs`…`text-5xl` on public pages, the compact
  `text-2xs`/`text-xs` scale inside the app. Never add a fixed
  `text-[13px]`-style one-off; if a step is missing, add a `@theme` token and
  update the spec first.
- Do not "fix" the fixture to make the check pass; fix the component.
- Do not disable or weaken a check without changing `docs/ui-spec.md` in the
  same change.
- The fixture proves component and row geometry, computed typography and
  interaction states. It does not prove live-data layout; say so when that is
  what was asked.
- Keep the visual language: monochrome, square corners, 1px rules, colour only
  for state. Prefer type scale, weight and whitespace over more borders or
  shadows.
