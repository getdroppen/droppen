// The browser's cloud client, run headless under node against a local stack
// from `server-rs/crates/rh-devkit/scripts/cloud-dev.sh up`:
//
//   npx tsx scripts/cloud-smoke.ts <hub http url> <invite> <path to cloud-dev.sh>
//
// Signs up and creates the account as the page does — on chain v3 from its
// genesis, with recovery words — enrolls the dev stack's throwaway agent
// (running `cloud-dev.sh enroll` with the code) as a host with its first
// machine key, hands that key over as "add a computer" does (by mailbox:
// the agent is not running yet), starts it, and lists sessions over an
// end-to-end channel through the hub — the same modules the page uses, with
// node's fetch, WebSocket and WebCrypto.
import { spawn } from "node:child_process";
import { MemoryKeyStore, formatEnrollCode } from "../src/lib/e2e/index.js";
import { CloudAccount } from "../src/lib/hub/account.js";
import { HubApi, chainV3Writable } from "../src/lib/hub/api.js";
import { linkCourier } from "../src/lib/hub/deliver.js";
import { HubLink } from "../src/lib/hub/link.js";
import { MachineChannel } from "../src/lib/hub/machine.js";
import { tunnelTransport } from "../src/lib/transport/tunnel.js";

const [hubUrl, invite, devScript] = process.argv.slice(2);
if (!hubUrl || !invite || !devScript) throw new Error("usage: cloud-smoke.ts <hub url> <invite> <cloud-dev.sh>");
const origin = hubUrl.replace(/^http/, "ws");

function run(args: string[]): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(devScript!, args, { stdio: ["ignore", "inherit", "inherit"] });
    child.on("exit", (code) => resolve(code ?? 1));
  });
}

const api = new HubApi(hubUrl);
const session = await api.signUp(`smoke-${Date.now()}@example.com`, "correct horse battery", { invite });
const store = new MemoryKeyStore();
const account = await CloudAccount.open(api, session, store, { origin });
console.log("account", account.account, "device", account.deviceId);
// Only while the hub takes chain v3 entries (`/v1/me` `chain_v3.writable`): there is no v2 fallback.
await account.createGenesis(true);
console.log("chain v3 from seq", account.chain?.v3From);
const code = await account.startEnrollment();
const enrolled = run(["enroll", formatEnrollCode(code)]);
// A new host on a v3 chain: `[addDevice3{host}, keyEpochs]`, again only while the hub takes v3 entries.
const writable = chainV3Writable(await api.me(account.session.token));
const machine = await account.completeEnrollment(code, undefined, 300, undefined, { writable });
console.log("machine added", machine.id, "enroll exit", await enrolled);
const link = new HubLink({
  url: `${origin}/v1/client`,
  origin,
  account: account.account,
  deviceId: account.deviceId,
  token: () => account.session.token,
  signKey: account.keys.sign.privateKey,
  renew: () => account.renewSession(),
});
link.start();
await link.waitOpen();
// Its first machine key went into the keyring with it: handed over now.
const { mkEpoch, result } = await account.handOverMachineKey(machine.id, linkCourier(link, account));
console.log("machine key epoch", mkEpoch, "via", result.via);
await run(["agent"]);
await new Promise<void>((resolve) => {
  const check = () => link.presence.get(machine.id)?.state === "online" && resolve();
  link.onPresence(check);
  check();
});
console.log("machine online");
const channel = new MachineChannel(link, account, machine.id);
const transport = tunnelTransport(channel);
const t0 = performance.now();
const res = await transport.fetch(new Request("http://machine/api/sessions?scope=all"));
const body = (await res.json()) as { total: number; sessions: { id: string }[] };
console.log("handshake", channel.handshakeMs?.toFixed(1), "ms; first rpc", (performance.now() - t0).toFixed(1), "ms;", res.status, body.total, body.sessions.map((s) => s.id));
link.stop();
process.exit(res.status === 200 && body.total > 0 ? 0 : 1);
