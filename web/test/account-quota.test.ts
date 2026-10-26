import assert from "node:assert/strict";
import test from "node:test";
import {
  accountUsageState,
  quotaAge,
  quotaDoubt,
  tightestWindow,
  usageNotice,
  type QuotaWindow,
} from "../src/features/accounts/use-accounts.js";

function win(over: Partial<QuotaWindow> & Pick<QuotaWindow, "label" | "remainingPercent">): QuotaWindow {
  return {
    resetsAt: null,
    usedPercent: 100 - over.remainingPercent,
    windowMinutes: 300,
    ...over,
  };
}

test("tightestWindow picks the window closest to running out", () => {
  const five = win({ label: "5h", remainingPercent: 4 });
  const weekly = win({ label: "weekly", remainingPercent: 81, windowMinutes: 10080 });
  assert.equal(tightestWindow([weekly, five]), five);
  assert.equal(tightestWindow([five, weekly]), five);
});

test("tightestWindow has nothing to show for no windows", () => {
  assert.equal(tightestWindow([]), null);
});

test("tightestWindow keeps the server's order on a tie", () => {
  const a = win({ label: "5h", remainingPercent: 50 });
  const b = win({ label: "weekly", remainingPercent: 50 });
  assert.equal(tightestWindow([a, b]), a);
  assert.equal(tightestWindow([b, a]), b);
});

test("tightestWindow reads an exhausted window as tightest, not as missing", () => {
  const empty = win({ label: "5h", remainingPercent: 0 });
  const some = win({ label: "weekly", remainingPercent: 30 });
  assert.equal(tightestWindow([some, empty]), empty);
});

test("accountUsageState keeps a failed probe apart from a missing login", () => {
  assert.equal(accountUsageState("oauth_token", true, undefined), "ready");
  assert.equal(accountUsageState("oauth_token", true, "probe_failed"), "unavailable");
  // Enabled once and now not logged in: expired, which is not the same as never
  // set up — the server still carries the last good windows for this one.
  assert.equal(accountUsageState("oauth_token", true, "not_logged_in"), "login_expired");
  assert.equal(accountUsageState("oauth_token", false, "not_logged_in"), "login_needed");
  assert.equal(accountUsageState("oauth_token", false, undefined), "login_needed");
  // An env account — either CLI's — has no /usage endpoint to probe at all.
  assert.equal(accountUsageState("env", false, undefined), "not_applicable");
});

test("quotaAge tells a reading nobody can date apart from a fresh one", () => {
  const now = Date.parse("2024-05-01T12:00:00.000Z");
  assert.equal(quotaAge("2024-05-01T11:58:00.000Z", now), "fresh");
  assert.equal(quotaAge("2024-05-01T11:45:00.000Z", now), "stale");
  // No timestamp is not "just now": it is a reading nobody can date.
  assert.equal(quotaAge(null, now), "unknown");
  assert.equal(quotaAge(undefined, now), "unknown");
  assert.equal(quotaAge("not a date", now), "unknown");
});

// An account's numbers now come mainly from its own sessions; the /usage probe
// is only the fallback once sessions go quiet. So a broken probe or an expired
// /usage login only alarms when the reading has aged — a fresh one is current
// whatever the probe last said.
const NOW = Date.parse("2024-05-01T12:00:00.000Z");
const FRESH = "2024-05-01T11:58:00.000Z";
const STALE = "2024-05-01T10:00:00.000Z";

test("usageNotice softens an expired /usage login behind a fresh reading", () => {
  assert.equal(usageNotice("login_expired", FRESH, true, NOW), "login_expired_idle");
  // Not fresh: the fallback was what should have kept the numbers current, and
  // it cannot, so today's amber line stays.
  assert.equal(usageNotice("login_expired", STALE, true, NOW), "login_expired");
  assert.equal(usageNotice("login_expired", null, true, NOW), "login_expired");
  assert.equal(usageNotice("login_expired", "not a date", true, NOW), "login_expired");
});

test("usageNotice drops a failed probe behind a fresh reading, and keeps it otherwise", () => {
  assert.equal(usageNotice("unavailable", FRESH, true, NOW), "none");
  assert.equal(usageNotice("unavailable", STALE, true, NOW), "unavailable");
  assert.equal(usageNotice("unavailable", undefined, true, NOW), "unavailable");
});

test("usageNotice turns a never-enabled /usage login into a fallback note once sessions report windows", () => {
  // Sessions report any account's windows, so the numbers are there; /usage
  // would only have served the idle-time refresh — fresh or not.
  assert.equal(usageNotice("login_needed", FRESH, true, NOW), "login_needed_idle");
  assert.equal(usageNotice("login_needed", STALE, true, NOW), "login_needed_idle");
  assert.equal(usageNotice("login_needed", null, false, NOW), "login_needed");
});

test("usageNotice reads exactly as before when there are no windows", () => {
  for (const at of [FRESH, STALE, null]) {
    assert.equal(usageNotice("login_needed", at, false, NOW), "login_needed");
    assert.equal(usageNotice("login_expired", at, false, NOW), "login_expired");
    assert.equal(usageNotice("unavailable", at, false, NOW), "unavailable");
    assert.equal(usageNotice("ready", at, false, NOW), "none");
    assert.equal(usageNotice("not_applicable", at, false, NOW), "none");
  }
});

test("usageNotice never gives a working login or an env block a line", () => {
  assert.equal(usageNotice("ready", STALE, true, NOW), "none");
  assert.equal(usageNotice("ready", FRESH, true, NOW), "none");
  // An env account on a relay that passes rate limits through has windows and
  // no /usage endpoint at all: the numbers, and nothing to say about them.
  assert.equal(usageNotice("not_applicable", FRESH, true, NOW), "none");
  assert.equal(usageNotice("not_applicable", STALE, true, NOW), "none");
});

test("quotaDoubt draws no ~ for a fresh reading, whatever the probe said", () => {
  assert.equal(quotaDoubt("login_expired", FRESH, NOW), null);
  assert.equal(quotaDoubt("unavailable", FRESH, NOW), null);
  assert.equal(quotaDoubt("ready", FRESH, NOW), null);
});

test("quotaDoubt names the probe's trouble ahead of the bare age once the reading has aged", () => {
  assert.equal(quotaDoubt("login_expired", STALE, NOW), "login_expired");
  assert.equal(quotaDoubt("unavailable", STALE, NOW), "unavailable");
  assert.equal(quotaDoubt("login_expired", null, NOW), "login_expired");
  assert.equal(quotaDoubt("ready", STALE, NOW), "stale");
  assert.equal(quotaDoubt("ready", null, NOW), "unknown");
});
