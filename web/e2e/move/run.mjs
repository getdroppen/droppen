// The browser key move on two real hosts, in Chromium, Firefox and WebKit
// (Playwright): push, pull (also from the sign-in screen's offer, with
// another tab reloading as the device arrives), a counterpart that is not
// exactly the pinned one (refused), a blocked popup, a foreign opener (steal and
// plant), a failed verify (no wipe), and a move cut off (resumable, no key
// lost) — once before the app host signed in, once after, and once after
// with "Remove this browser" at the apex then (local-only: my. keeps it).
//
//   node e2e/move/run.mjs [chromium,firefox,webkit]      (from web/; run.sh does it all)
//
// Needs `server.ts` listening on MOVE_PORT (8443) and droppen.test,
// my.droppen.test, evil.test resolving to 127.0.0.1. PLAYWRIGHT_MODULE: where
// Playwright is, when it is not installed next to this file.
import { request } from "node:https";

const pw = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const PORT = process.env.MOVE_PORT || "8443";
const LEGACY = `https://droppen.test:${PORT}`;
const APP = `https://my.droppen.test:${PORT}`;
const EVIL = `https://evil.test:${PORT}`;
const which = (process.argv[2] || "chromium,firefox,webkit").split(",");
const T = 30_000;

/** The run's control of the stand-in hub. */
function hub(path, body) {
  return new Promise((resolve, reject) => {
    const data = body === undefined ? undefined : JSON.stringify(body);
    const req = request(
      { host: "127.0.0.1", port: Number(PORT), path, method: data ? "POST" : "GET", servername: "droppen.test", rejectUnauthorized: false, headers: { host: `droppen.test:${PORT}`, "content-type": "application/json" } },
      (res) => {
        let text = "";
        res.on("data", (c) => (text += c));
        res.on("end", () => resolve(JSON.parse(text || "null")));
      },
    );
    req.on("error", reject);
    req.end(data);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function pageResult(ctx, url) {
  const p = await ctx.newPage();
  await p.goto(url);
  await p.waitForFunction(() => window.__done !== undefined, null, { timeout: T });
  const v = await p.evaluate(() => window.__done);
  await p.close();
  return v;
}
const seed = (ctx, account) => pageResult(ctx, `${LEGACY}/seed?account=${account}&device=dev-1`);
const check = (ctx, origin) => pageResult(ctx, `${origin}/check`);

/** Push from droppen.test's tab; `during` runs once the popup is open. */
async function push(ctx, { block = false, during, query = "" } = {}) {
  const tab = await ctx.newPage();
  await tab.goto(`${LEGACY}/${block ? "?block=1" : query}`);
  const opened = block ? null : ctx.waitForEvent("page", { timeout: T });
  await tab.click("#move");
  const popup = opened ? await opened : null;
  if (during) await during({ tab, popup });
  await tab.waitForFunction(() => window.__outcome !== undefined, null, { timeout: 3 * T });
  const outcome = await tab.evaluate(() => window.__outcome);
  const steps = await tab.evaluate(() => window.__steps);
  return { outcome, steps, tab, popup };
}

/** Pull from my.droppen.test's tab, confirming in the popup; `during` runs once confirmed. */
async function pull(ctx, { block = false, during, query = "" } = {}) {
  const tab = await ctx.newPage();
  await tab.goto(`${APP}/${block ? "?block=1" : query}`);
  const opened = block ? null : ctx.waitForEvent("page", { timeout: T });
  await tab.click("#pull");
  const popup = opened ? await opened : null;
  if (popup) {
    await popup.waitForSelector('[data-testid="move-confirm"]', { timeout: T });
    await popup.click('[data-testid="move-confirm"]');
  }
  if (during) await during({ tab, popup });
  await tab.waitForFunction(() => window.__outcome !== undefined, null, { timeout: 3 * T });
  const outcome = await tab.evaluate(() => window.__outcome);
  const steps = await tab.evaluate(() => window.__steps);
  return { outcome, steps, tab, popup };
}

async function heldAtHub() {
  for (let i = 0; i < 200; i++) {
    if (((await hub("/__test/state")).held ?? 0) > 0) return true;
    await sleep(50);
  }
  return false;
}

const sameList = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** What the app host holds after a move that worked: the device, signing in from a fresh page. */
function movedHere(expect, app, seeded, tag) {
  expect(app.keys?.found === true && app.keys.signKey === seeded.signKey, `${tag}: my. holds the device's keys (after a reload)`);
  expect(app.signsIn === true, `${tag}: my. signs in afresh with them (${app.signsIn})`);
  expect(app.local.includes(`rh.hub.device.${seeded.account}`) && app.local.includes("rh.theme"), `${tag}: the notes moved`);
  expect(!app.local.includes("rh.hub.push.subscribed") && !app.local.includes("rh.draft.note"), `${tag}: nothing off the list moved`);
  expect(app.pushMoved === "host-1", `${tag}: "notifications were on" crossed (${app.pushMoved})`);
  expect(app.marker?.state === "verified", `${tag}: my. noted the move verified`);
}
/** What droppen.app holds after its copy went: nothing of the device. */
function wipedThere(expect, legacy, tag) {
  expect(legacy.device === null, `${tag}: droppen.test holds no device (${JSON.stringify(legacy.device)})`);
  expect(legacy.keyRecords === null || legacy.keyRecords.length === 0, `${tag}: droppen.test's key store is empty or gone`);
  expect(sameList(legacy.local, ["rh.draft.note"]), `${tag}: only what never moved is left (${legacy.local})`);
}
/** What droppen.app holds when nothing was removed: exactly what it had. */
function intactThere(expect, legacy, before, tag) {
  expect(legacy.keys?.found === true && legacy.keys.signKey === before.keys.signKey, `${tag}: droppen.test still holds the device's keys`);
  // Besides the note of a move that did not finish (`rh.hub.move.out`), checked where it is expected.
  const notes = (local) => local.filter((k) => k !== "rh.hub.move.out");
  expect(sameList(legacy.keyRecords, before.keyRecords) && sameList(notes(legacy.local), notes(before.local)), `${tag}: droppen.test holds all it had`);
  expect(legacy.signsIn === true, `${tag}: droppen.test still signs in with them (${legacy.signsIn})`);
}
function emptyHere(expect, app, tag) {
  expect(!app.device && (app.keyRecords === null || app.keyRecords.length === 0) && app.local.length === 0, `${tag}: my. holds nothing (${JSON.stringify({ d: app.device, k: app.keyRecords, l: app.local })})`);
}

const scenarios = {
  async parentDomainRefused(ctx, expect, account) {
    // A counterpart that is not exactly the pinned one — the apex's parent domain, the app host's parent (the apex
    // itself as the app), a lookalike — is refused before any window opens, on both sides.
    await seed(ctx, account);
    const before = await check(ctx, LEGACY);
    const outcomes = [];
    for (const [side, query] of [
      ["push", `?target=${encodeURIComponent(`https://test:${PORT}`)}`],
      ["push", `?target=${encodeURIComponent(`https://droppen.test.evil.test:${PORT}`)}`],
      ["pull", `?from=${encodeURIComponent(`https://test:${PORT}`)}`],
      ["pull", `?from=${encodeURIComponent(`https://evil.test:${PORT}`)}`],
    ]) {
      const pages = ctx.pages().length;
      const tab = await ctx.newPage();
      await tab.goto(`${side === "push" ? LEGACY : APP}/${query}`);
      await tab.click(side === "push" ? "#move" : "#pull");
      await tab.waitForFunction(() => window.__outcome !== undefined, null, { timeout: T });
      const outcome = await tab.evaluate(() => window.__outcome);
      outcomes.push(outcome);
      expect(outcome.kind === "failed" && /not the hub's own/.test(outcome.reason), `${side} ${query}: refused (${JSON.stringify(outcome)})`);
      expect(ctx.pages().length === pages + 1, `${side} ${query}: no window opened`);
      await tab.close();
    }
    intactThere(expect, await check(ctx, LEGACY), before, "parent domain");
    emptyHere(expect, await check(ctx, APP), "parent domain");
    return { outcomes };
  },
  async removeAfterUnfinished(ctx, expect, account) {
    // Cut off after my. signed in (the apex popup closed first): the apex notes the move unfinished, and its
    // "Remove this browser" then deletes only its own copy — my. keeps the device, still signing in.
    const seeded = await seed(ctx, account);
    await hub("/__test/hub", { hold: true });
    let first;
    try {
      first = await pull(ctx, {
        async during({ popup }) {
          expect(await heldAtHub(), "my. was signing in");
          await popup.close();
          await hub("/__test/hub", { release: true });
        },
      });
    } finally {
      await hub("/__test/hub", { release: true });
    }
    expect(first.outcome.kind === "moved" && first.outcome.senderWiped === false, `my. has it, the apex kept its copy: ${JSON.stringify(first.outcome)}`);
    const apex = await check(ctx, LEGACY);
    expect(apex.unfinished?.state === "sent" && apex.unfinished.device === "dev-1", `the apex noted the move unfinished (${JSON.stringify(apex.unfinished)})`);
    const logged = (await hub("/__test/state")).log.length;
    const removed = await pageResult(ctx, `${LEGACY}/remove`);
    expect(removed.localOnly === "sent", `Remove at the apex was local-only (${JSON.stringify(removed)})`);
    // Local-only never opens the account (no device sign-in at the apex), so it can append nothing to it.
    const opened = (await hub("/__test/state")).log.slice(logged).filter((l) => l.what.startsWith("session") && l.host === "droppen.test");
    expect(opened.length === 0, `the apex did not sign in as the device to remove it (${JSON.stringify(opened)})`);
    const after = await check(ctx, LEGACY);
    expect(after.device === null && !after.local.includes("rh.hub.session") && after.unfinished === null, `the apex copy is gone (${JSON.stringify({ d: after.device, l: after.local, u: after.unfinished })})`);
    movedHere(expect, await check(ctx, APP), seeded, "still at my.");
    return { first: first.outcome, removed };
  },
  async push(ctx, expect, account) {
    const seeded = await seed(ctx, account);
    const { outcome, steps, popup } = await push(ctx);
    expect(outcome.kind === "moved" && outcome.senderWiped && !outcome.resumed && outcome.pushWasOn === "host-1", `outcome ${JSON.stringify(outcome)}`);
    expect(sameList(steps, ["opening", "waiting", "sending", "verifying", "wiping"]), `steps ${steps}`);
    await popup.waitForEvent("close", { timeout: 8_000 }).catch(() => undefined);
    expect(popup.isClosed(), "the my. popup closed itself");
    movedHere(expect, await check(ctx, APP), seeded, "push");
    wipedThere(expect, await check(ctx, LEGACY), "push");
    return { outcome };
  },
  async pull(ctx, expect, account) {
    const seeded = await seed(ctx, account);
    const { outcome, steps, popup } = await pull(ctx);
    expect(outcome.kind === "moved" && outcome.senderWiped && !outcome.resumed, `outcome ${JSON.stringify(outcome)}`);
    expect(sameList(steps, ["opening", "waiting", "confirm", "verifying", "wiping"]), `steps ${steps}`);
    await popup.waitForEvent("close", { timeout: 8_000 }).catch(() => undefined);
    expect(popup.isClosed(), "the droppen.test popup closed itself");
    movedHere(expect, await check(ctx, APP), seeded, "pull");
    wipedThere(expect, await check(ctx, LEGACY), "pull");
    return { outcome };
  },
  async pullFromOffer(ctx, expect, account) {
    // The sign-in screen's own "Bring it over", with a second my. tab open that must reload as the device arrives.
    const seeded = await seed(ctx, account);
    const other = await ctx.newPage();
    await other.goto(`${APP}/watch`);
    await other.waitForFunction(() => window.__done === undefined && document.getElementById("out"), null, { timeout: T });
    const tab = await ctx.newPage();
    await tab.goto(`${APP}/`);
    await tab.waitForSelector('[data-testid="bring-it-over"]', { timeout: T });
    await tab.evaluate(() => (window.__sameDocument = true));
    const opened = ctx.waitForEvent("page", { timeout: T });
    await tab.click('[data-testid="bring-it-over"]');
    const popup = await opened;
    await popup.waitForSelector('[data-testid="move-confirm"]', { timeout: T });
    await popup.click('[data-testid="move-confirm"]');
    await tab.waitForFunction(() => window.__offerMoved === true, null, { timeout: 3 * T });
    await other.waitForFunction(() => sessionStorage.getItem("loads") === "2", null, { timeout: T }).catch(() => undefined);
    const loads = await other.evaluate(() => sessionStorage.getItem("loads"));
    expect(loads === "2", `the other my. tab reloaded once as the device arrived (loads ${loads})`);
    movedHere(expect, await check(ctx, APP), seeded, "offer");
    wipedThere(expect, await check(ctx, LEGACY), "offer");
    const after = await ctx.newPage();
    await after.goto(`${APP}/`);
    await after.waitForSelector("#pull", { timeout: T });
    await sleep(1_000);
    expect((await after.locator('[data-testid="bring-it-over"]').count()) === 0, "with a device here, the offer is gone");
    expect((await tab.evaluate(() => window.__sameDocument)) === true, "the tab that pulled did not reload from its own word (it goes on itself)");
    return { loads };
  },
  async popupBlocked(ctx, expect, account) {
    await seed(ctx, account);
    const before = await check(ctx, LEGACY);
    const a = await push(ctx, { block: true });
    const b = await pull(ctx, { block: true });
    expect(a.outcome.kind === "popupBlocked" && b.outcome.kind === "popupBlocked", `outcomes ${a.outcome.kind} ${b.outcome.kind}`);
    intactThere(expect, await check(ctx, LEGACY), before, "blocked");
    emptyHere(expect, await check(ctx, APP), "blocked");
    return { push: a.outcome, pull: b.outcome };
  },
  async foreignOpener(ctx, expect, account) {
    await seed(ctx, account);
    const before = await check(ctx, LEGACY);
    // Steal: another site opens droppen.test/move, asks for the keys, and the person clicks anything there is.
    const steal = await ctx.newPage();
    await steal.goto(`${EVIL}/steal`);
    const opened = ctx.waitForEvent("page", { timeout: T });
    await steal.click("#go");
    const victim = await opened;
    await steal.waitForFunction(() => window.__evil.done, null, { timeout: T });
    const confirmButtons = await victim.locator('[data-testid="move-confirm"]').count();
    const stolen = await steal.evaluate(() => window.__evil.received);
    const victimStep = await victim.locator('[data-testid="move-sender"]').getAttribute("data-step");
    expect(stolen.length === 0, `the other site received nothing (${JSON.stringify(stolen)})`);
    expect(confirmButtons === 0 && victimStep === "waiting", `droppen.test/move never offered to send (${victimStep}, ${confirmButtons} buttons)`);
    await victim.close();
    // Plant: another site opens my.droppen.test/move and hands it keys of its own.
    const plant = await ctx.newPage();
    await plant.goto(`${EVIL}/plant`);
    const opened2 = ctx.waitForEvent("page", { timeout: T });
    await plant.click("#go");
    const target = await opened2;
    await plant.waitForFunction(() => window.__evil.done, null, { timeout: T });
    const heard = await plant.evaluate(() => window.__evil.received);
    const targetStep = await target.locator('[data-testid="move-receiver"]').getAttribute("data-step");
    expect(heard.length === 0, `my.droppen.test/move told the other site nothing (${JSON.stringify(heard)})`);
    expect(targetStep === "waiting", `my.droppen.test/move took nothing in (${targetStep})`);
    await target.close();
    intactThere(expect, await check(ctx, LEGACY), before, "foreign");
    emptyHere(expect, await check(ctx, APP), "foreign");
    return { stolen, heard, victimStep, targetStep };
  },
  async verifyFailed(ctx, expect, account) {
    await seed(ctx, account);
    const before = await check(ctx, LEGACY);
    await hub("/__test/hub", { deny: true });
    let outcome;
    try {
      ({ outcome } = await push(ctx));
    } finally {
      await hub("/__test/hub", { deny: false });
    }
    expect(outcome.kind === "verifyFailed", `outcome ${JSON.stringify(outcome)}`);
    intactThere(expect, await check(ctx, LEGACY), before, "verify failed");
    const app = await check(ctx, APP);
    expect((app.keyRecords ?? []).length === 0 && app.local.length === 0 && app.marker === null, `verify failed: my. took back all it wrote (${JSON.stringify({ k: app.keyRecords, l: app.local })})`);
    return { outcome };
  },
  async interrupted(ctx, expect, account) {
    const seeded = await seed(ctx, account);
    const before = await check(ctx, LEGACY);
    await hub("/__test/hub", { hold: true });
    let first;
    try {
      first = await push(ctx, {
        async during({ popup }) {
          await popup.waitForSelector('[data-testid="move-receiver"][data-step="verifying"]', { timeout: T });
          expect(await heldAtHub(), "the my. popup was signing in when it was closed");
          await popup.close(); // the person closes the window mid-move
        },
      });
    } finally {
      await hub("/__test/hub", { release: true });
    }
    expect(first.outcome.kind === "unfinished" && first.outcome.why === "closed", `cut off after the keys went: ${JSON.stringify(first.outcome)}`);
    const cut = await check(ctx, LEGACY);
    intactThere(expect, cut, before, "cut off");
    expect(cut.unfinished?.state === "sent", `the apex noted the move unfinished (${JSON.stringify(cut.unfinished)})`);
    const residue = await check(ctx, APP);
    expect(!residue.signsIn, "my. never signed in with what it got");
    const again = await push(ctx);
    expect(again.outcome.kind === "moved" && again.outcome.senderWiped, `started again: ${JSON.stringify(again.outcome)}`);
    movedHere(expect, await check(ctx, APP), seeded, "resumed");
    wipedThere(expect, await check(ctx, LEGACY), "resumed");
    return { first: first.outcome, residue: { marker: residue.marker, keyRecords: residue.keyRecords?.length ?? null }, again: again.outcome };
  },
  async interruptedAfterVerify(ctx, expect, account) {
    const seeded = await seed(ctx, account);
    await hub("/__test/hub", { hold: true });
    let first;
    try {
      first = await pull(ctx, {
        async during({ popup }) {
          expect(await heldAtHub(), "my. was signing in");
          await popup.close(); // droppen.test's window goes before it heard my. signed in
          await hub("/__test/hub", { release: true });
        },
      });
    } finally {
      await hub("/__test/hub", { release: true });
    }
    expect(first.outcome.kind === "moved" && first.outcome.senderWiped === false, `my. has it, droppen.test kept its copy: ${JSON.stringify(first.outcome)}`);
    const both = await check(ctx, LEGACY);
    expect(both.keys?.found === true && both.keys.signKey === seeded.signKey, "no key lost: droppen.test still holds the device");
    const again = await push(ctx);
    expect(again.outcome.kind === "moved" && again.outcome.resumed === true && again.outcome.senderWiped, `finished from droppen.test: ${JSON.stringify(again.outcome)}`);
    movedHere(expect, await check(ctx, APP), seeded, "after verify");
    wipedThere(expect, await check(ctx, LEGACY), "after verify");
    return { first: first.outcome, again: again.outcome };
  },
};

const out = {};
let failed = 0;
for (const name of which) {
  const args = name === "chromium" ? [`--host-resolver-rules=MAP droppen.test 127.0.0.1, MAP my.droppen.test 127.0.0.1, MAP evil.test 127.0.0.1`] : [];
  let browser;
  try {
    browser = await pw[name].launch({ args });
  } catch (e) {
    out[name] = { launchError: String(e.message).split("\n")[0] };
    failed++;
    continue;
  }
  out[name] = { version: browser.version(), scenarios: {} };
  for (const [scenario, run] of Object.entries(scenarios)) {
    const ctx = await browser.newContext({ ignoreHTTPSErrors: true });
    const fails = [];
    const expect = (ok, what) => void (ok || fails.push(what));
    let detail;
    try {
      detail = await run(ctx, expect, `acct-${name}-${scenario}-${Date.now().toString(36)}`);
    } catch (e) {
      fails.push(`threw: ${String(e.message).split("\n")[0]}`);
    }
    await ctx.close();
    out[name].scenarios[scenario] = { pass: fails.length === 0, fails, detail };
    if (fails.length) failed++;
    console.log(`${name.padEnd(9)} ${scenario.padEnd(23)} ${fails.length ? `FAIL ${fails.join(" | ")}` : "pass"}`);
  }
  await browser.close();
}
console.log(JSON.stringify(out, null, 1));
console.log(failed ? `FAILED: ${failed}` : "all passed");
process.exit(failed ? 1 : 0);
