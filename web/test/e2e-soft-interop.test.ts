// The repo's cross-language vectors once more, on a WebCrypto without Ed25519
// and X25519 (taken away as an old Chromium lacks them: `no-native-curves.ts`)
// in compatibility mode (`lib/e2e/soft25519.ts`): Rust's chains, handshakes,
// sealed keys, recovery and pairing runs verify and open here, and what the
// TypeScript side signs and agrees comes out byte for byte as the committed
// `ts.json` (`e2e-ts-vectors.test.ts`), which Rust replays. Each file's tests
// run in this process, after the curves were swapped.
import assert from "node:assert/strict";
import { checkE2eSupport } from "../src/lib/e2e/support.js";
import { installSoftCurves, softCurvesOn } from "../src/lib/e2e/soft25519.js";
import { removeNative } from "./no-native-curves.js";

removeNative(["Ed25519", "X25519"]);
// Without them: what the browser in the report would say.
const before = await checkE2eSupport({ requireIndexedDb: false });
assert.deepEqual(before, { ok: false, missing: ["X25519", "Ed25519"] });
installSoftCurves(["Ed25519", "X25519"]);
assert.deepEqual(softCurvesOn(), ["Ed25519", "X25519"]);
assert.deepEqual(await checkE2eSupport({ requireIndexedDb: false }), { ok: true, missing: [] });

await import("./e2e-interop.test.js");
await import("./e2e-ts-vectors.test.js");
