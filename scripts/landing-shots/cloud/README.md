# Landing shots: the cloud app

Screenshots of droppen.app for the homepage's product tour. There is no hub to
sign in to here: `web/tour/` renders the real components in their real
composition with made-up data (no API, router or account), the way
`web/ui-audit/` does for the UI audit, and `run.mjs` captures them with
Playwright through the app's own Vite/Tailwind setup.

## Regenerate

```sh
pnpm install --frozen-lockfile
node scripts/landing-shots/cloud/run.mjs --channel chrome
```

`--channel chrome` uses an installed Chrome; without it the e2e workspace's
Chromium is used (`pnpm --filter @remote-hand/e2e exec playwright install chromium`
once). The pages use the system fonts (`--font-sans`, `--font-mono` in
`web/src/index.css`), so the shots look like the machine they are taken on:
take them on a Mac for SF and PingFang.

Options: `--out <dir>` (default `/tmp/droppen-tour/shots/cloud`),
`--shot <name>`, `--lang en|zh-CN|both` (default `en`), `--theme light|dark`.

## Shots

Each in English, light and dark: `<shot>-en-<theme>.png`. The homepage shows
the English shots whatever the page's language.

| shot | viewport | what |
|---|---|---|
| `cloud-devices` | 1280×800, 2× | 「我的设备」: the page's top bar, three devices that run AI (one offline), a browser and a phone, and 「需要你 · 所有设备」 across them |
| `phone-needs-you` | 390×844, 3×, touch | the same page on the phone that is looking, scrolled to its end: the device list and the ledger |
| `security-pair` | 390×844, 3×, touch | a new browser waiting to be let in: the QR code tab of the pairing screen, its offer text and fingerprint |

No browser chrome, no scrollbars (hidden by the runner), reduced motion (the
live dots hold still) and animations stopped.

## The fixture

`web/tour/main.tsx`, `?shot=devices|pair`, `&as=phone`, `&lang=`, `&theme=dark`.

- Rendered whole: `CloudPage` (the bar and the framed column), `AccountName`,
  `SignOutChoice`, `SectionHead`, `DeviceRow`, `NeedsYouLedger` fed by the real
  `needsYou()` and `recentSessions()`, `Centered`, `Panel`, `Tabs`, `QrCode`,
  `CopyButton`, `Fingerprint`.
- Copied from the page that draws it (not a component of its own):
  `MyDevicesFrame`'s back, account and sign-out buttons, `DevicesPanel`'s
  heading and device grid, `PairingAttempt`'s offer body and footer. Keep
  them in step when those pages change.
- Left out: the plan usage and 「这台设备」 sections between the heading and the
  list (they would push the ledger below the fold), the account notices
  (none apply), and the main-device badge (next to "can run AI" it cuts an
  English name short in a 1280px card).

`web/src/index.css` excludes `web/tour` from Tailwind's scan
(`@source not "../tour"`), so the fixture cannot change the production
stylesheet; every class it uses must already occur in `web/src`.
