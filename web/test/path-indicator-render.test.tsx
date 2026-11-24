// The path indicator in the cloud bar (`features/cloud/cloud-bar.tsx`
// `PathIndicator`) and the direct-connection setting under 「这台设备」
// (`features/cloud/devices.tsx` `DirectConnectionSetting`).
//
// What these pin (relay-and-plans §2.14): the indicator is two icons —
// direct, through a relay (or the hub, when no relay could carry the
// channel) — the active one in ink, then the active path's
// latest round trip, rounded, and "—" until one was measured; the number is
// coloured by how fast it is (green < 100 ms, amber < 250 ms, red beyond); a
// hub probe out a probe interval or more, longer than the last round trip,
// reads "slow · ≥5 s" (red), never as a round trip, until an answer comes; no
// words, but an aria-label naming the path and the relay's city; an amber
// dot for an emergency relay; nothing says "encrypted" (every path is); the
// direct-connection setting is a switch, said as what it allows (faster, not
// counted as transfer) and what that shows to whom; and all of it reads in
// zh-CN, zh-TW and ja.
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { latencyTone, PathIndicator } from "../src/features/cloud/cloud-bar.js";
import { DirectConnectionSetting } from "../src/features/cloud/devices.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import type { PathStats } from "../src/lib/e2e/index.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } },
    "zh-TW": { translation: { ...cloudDevicesZhTW, ...zhTW } },
    ja: { translation: { ...cloudDevicesJa, ...ja } }
  },
  keySeparator: false,
  nsSeparator: false
});

const draw = pageDraw(i18n);

const stats = (over: Partial<PathStats>): PathStats => ({
  direct: false,
  relayRttMs: null,
  directRttMs: null,
  relayProbeAgeMs: null,
  relayQuietMs: null,
  ups: 0,
  downs: 0,
  resent: 0,
  duplicates: 0,
  ...over,
});
const text = (html: string) => html.replace(/<[^>]+>/g, "");

const relay = { kind: "relay" as const, relay: { id: "nrt-1", city: "tokyo", region: "nrt", origin: "wss://nrt-1.relay.example", state: "up", load: 0.1 }, mode: "auto" as const };

test("the indicator is two icons and the active path's round trip, no words", () => {
  const connecting = draw(<PathIndicator paths={null} />);
  assert.match(connecting, /data-path="connecting"/);
  assert.equal(text(connecting), "—", "no number until one is known");
  assert.doesNotMatch(connecting, /data-latency=/);
  assert.match(connecting, /aria-label="connecting…"/);

  const via = draw(<PathIndicator paths={stats({ relayRttMs: 38.4 })} route={relay} />);
  assert.match(via, /data-path="relay"/);
  assert.equal(text(via), "38 ms", "the icons say the path; no label");
  assert.match(via, /class="[^"]*text-faint"[^>]*data-icon="direct"/);
  assert.match(via, /class="[^"]*text-fg"[^>]*data-icon="relay"/);
  assert.match(via, /aria-label="through the Tokyo relay, round trip 38 ms"/);
  assert.match(via, /data-latency="ok"[^>]*class="[^"]*text-ok/);

  const direct = draw(<PathIndicator paths={stats({ direct: true, directRttMs: 0.7, relayRttMs: 160.2, ups: 1 })} route={relay} />);
  assert.match(direct, /data-path="direct"/);
  assert.equal(text(direct), "1 ms");
  assert.match(direct, /class="[^"]*text-fg"[^>]*data-icon="direct"/);
  assert.match(direct, /class="[^"]*text-faint"[^>]*data-icon="relay"/);
  assert.match(direct, /aria-label="direct, round trip 1 ms"/);

  const far = draw(<PathIndicator paths={stats({ relayRttMs: 523 })} route={relay} />);
  assert.match(far, /data-latency="danger"[^>]*class="[^"]*text-danger/);
  const emergency = draw(<PathIndicator paths={stats({ relayRttMs: 80 })} route={{ ...relay, emergency: true }} />);
  assert.match(emergency, /data-signal="warn"/, "an emergency ticket shows an amber dot");
  assert.doesNotMatch(via, /data-signal/);
  for (const html of [connecting, via, direct, far]) assert.doesNotMatch(html, /encrypt/i, "every path is end-to-end encrypted: not said");
});

test("a channel the hub carries (no relay could) shows the relay icon and says the Hub", () => {
  const hub = { kind: "hub" as const };
  const html = draw(<PathIndicator paths={stats({ relayRttMs: 41.2 })} route={hub} />);
  assert.match(html, /data-path="hub"/);
  assert.equal(text(html), "41 ms");
  assert.match(html, /class="[^"]*text-faint"[^>]*data-icon="direct"/);
  assert.match(html, /class="[^"]*text-fg"[^>]*data-icon="relay"/);
  assert.match(html, /aria-label="through the Hub, round trip 41 ms"/);
  assert.doesNotMatch(html, /data-signal/, "no emergency dot");
  const before = draw(<PathIndicator paths={null} route={hub} />);
  assert.match(before, /data-path="hub"/, "the route is known before a round trip is");
  assert.match(before, /aria-label="through the Hub, round trip —"/);
  assert.match(draw(<PathIndicator paths={stats({ relayRttMs: 41 })} route={hub} />, "zh-CN"), /aria-label="经 Hub，往返 41 ms"/);
  assert.match(draw(<PathIndicator paths={stats({ relayRttMs: 41 })} route={hub} />, "zh-TW"), /aria-label="經 Hub，往返 41 ms"/);
  assert.match(draw(<PathIndicator paths={stats({ relayRttMs: 41 })} route={hub} />, "ja"), /aria-label="Hub 経由、往復 41 ms"/);
  // Direct next to the hub: direct, as with a relay.
  assert.match(draw(<PathIndicator paths={stats({ direct: true, directRttMs: 2, relayRttMs: 41 })} route={hub} />), /aria-label="direct, round trip 2 ms"/);
});

test("a hub probe out a probe interval says the path is slow, never a round trip nobody measured", () => {
  const slow = draw(<PathIndicator paths={stats({ relayRttMs: 38.4, relayProbeAgeMs: 5_001, relayQuietMs: 200 })} route={relay} />);
  assert.equal(text(slow), "slow · ≥5 s");
  assert.doesNotMatch(slow, /5001|38 ms/);
  assert.match(slow, /data-latency="danger" data-slow=""[^>]*class="[^"]*text-danger/);
  assert.match(slow, /aria-label="through the Tokyo relay, slow: round trip at least 5 s"/);
  assert.equal(text(draw(<PathIndicator paths={stats({ relayRttMs: 38.4, relayProbeAgeMs: 10_001 })} route={relay} />)), "slow · ≥10 s", "whole seconds, rounded down");
  assert.equal(text(draw(<PathIndicator paths={stats({ relayProbeAgeMs: 5_000 })} route={relay} />)), "slow · ≥5 s", "before any round trip was measured");
  // Out less than an interval: the round trip says it.
  assert.equal(text(draw(<PathIndicator paths={stats({ relayRttMs: 38.4, relayProbeAgeMs: 4_999 })} route={relay} />)), "38 ms");
  // The answer came: its round trip, however slow, is what shows again.
  const answered = draw(<PathIndicator paths={stats({ relayRttMs: 6_577, relayProbeAgeMs: 1_400, relayQuietMs: 0 })} route={relay} />);
  assert.equal(text(answered), "6577 ms");
  assert.doesNotMatch(answered, /data-slow/);
  assert.match(answered, /aria-label="through the Tokyo relay, round trip 6577 ms"/);
  assert.equal(text(draw(<PathIndicator paths={stats({ relayRttMs: 6_577, relayProbeAgeMs: 5_001 })} route={relay} />)), "6577 ms", "a slower round trip measured says more");
  // On the hub, and in each language.
  const hub = { kind: "hub" as const };
  assert.match(draw(<PathIndicator paths={stats({ relayRttMs: 41, relayProbeAgeMs: 5_001 })} route={hub} />), /aria-label="through the Hub, slow: round trip at least 5 s"/);
  const zh = draw(<PathIndicator paths={stats({ relayRttMs: 41, relayProbeAgeMs: 5_001 })} route={relay} />, "zh-CN");
  assert.equal(text(zh), "慢 · ≥5 秒");
  assert.match(zh, /aria-label="经东京中继，较慢：往返至少 5 秒"/);
  const tw = draw(<PathIndicator paths={stats({ relayRttMs: 41, relayProbeAgeMs: 5_001 })} route={relay} />, "zh-TW");
  assert.equal(text(tw), "慢 · ≥5 秒");
  assert.match(tw, /aria-label="經東京中繼，較慢：往返至少 5 秒"/);
  const ja = draw(<PathIndicator paths={stats({ relayRttMs: 41, relayProbeAgeMs: 5_001 })} route={hub} />, "ja");
  assert.equal(text(ja), "低速 · ≥5 秒");
  assert.match(ja, /aria-label="Hub 経由、低速：往復 5 秒以上"/);
  // On a direct path the direct round trip shows; a slow hub path next to it does not take over.
  const direct = draw(<PathIndicator paths={stats({ direct: true, directRttMs: 2, relayRttMs: 41, relayProbeAgeMs: 5_001 })} route={relay} />);
  assert.equal(text(direct), "2 ms");
  assert.doesNotMatch(direct, /data-slow/);
});

test("round trips are green under 100 ms, amber under 250 ms and red beyond", () => {
  assert.equal(latencyTone(1), "ok");
  assert.equal(latencyTone(99), "ok");
  assert.equal(latencyTone(100), "warn");
  assert.equal(latencyTone(249), "warn");
  assert.equal(latencyTone(250), "danger");
});

test("the indicator's label reads in zh-CN, zh-TW and ja", () => {
  const direct = stats({ direct: true, directRttMs: 3.2, relayRttMs: 170 });
  const through = stats({ relayRttMs: 38 });
  assert.match(draw(<PathIndicator paths={direct} route={relay} />, "zh-CN"), /aria-label="直连，往返 3 ms"/);
  assert.match(draw(<PathIndicator paths={through} route={relay} />, "zh-CN"), /aria-label="经东京中继，往返 38 ms"/);
  assert.match(draw(<PathIndicator paths={through} route={relay} />, "zh-TW"), /aria-label="經東京中繼，往返 38 ms"/);
  assert.match(draw(<PathIndicator paths={through} route={relay} />, "ja"), /aria-label="東京の中継経由、往復 38 ms"/);
  assert.match(draw(<PathIndicator paths={direct} route={relay} />, "ja"), /aria-label="直結、往復 3 ms"/);
});

test("the direct-connection setting is a switch, says what allowing it gives and what it shows to whom", () => {
  const on = draw(<DirectConnectionSetting allowed onChange={() => undefined} />);
  assert.match(on, /data-testid="direct-connections"[^>]*data-allowed=""/);
  assert.match(on, /role="switch"[^>]*aria-checked="true"/);
  assert.ok(on.includes("Allow direct connections (faster)"));
  assert.ok(on.includes("not counted as transfer"), "faster, and the relays' transfer is not spent");
  assert.ok(on.includes("IP address") && on.includes("Cloudflare and Google") && on.includes("end-to-end encrypted"));
  const off = draw(<DirectConnectionSetting allowed={false} onChange={() => undefined} />);
  assert.match(off, /role="switch"[^>]*aria-checked="false"/);
  assert.doesNotMatch(off, /data-allowed=""/);
  const zh = draw(<DirectConnectionSetting allowed onChange={() => undefined} />, "zh-CN");
  assert.ok(zh.includes("允许直连（更快）") && zh.includes("不计入流量") && zh.includes("端到端加密"), zh);
  assert.ok(draw(<DirectConnectionSetting allowed onChange={() => undefined} />, "zh-TW").includes("允許直連（更快）"));
  assert.ok(draw(<DirectConnectionSetting allowed onChange={() => undefined} />, "ja").includes("直接接続を許可"));
  assert.doesNotMatch(on, /text-(danger|warn|ok)|data-signal/);
  // Flipping it says the new state: allowed is the stored "hub only" switch, inverted.
  const seen: boolean[] = [];
  let tree: React.ReactElement<{ children: React.ReactElement[] }> | null = null;
  function Probe() {
    tree = DirectConnectionSetting({ allowed: true, onChange: (next) => void seen.push(next) }) as React.ReactElement<{ children: React.ReactElement[] }>;
    return null;
  }
  draw(<Probe />);
  const flip = ([] as React.ReactElement<Record<string, unknown>>[]).concat(tree!.props.children as never).find((k) => (k.props as Record<string, unknown>).testId === "direct-switch");
  (flip!.props.onChange as (on: boolean) => void)(false);
  assert.deepEqual(seen, [false]);
});
