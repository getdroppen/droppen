// Compatibility mode's curves (`lib/e2e/soft25519.ts`, `@noble/curves` behind
// WebCrypto's interface) give exactly the bytes the browser's own WebCrypto
// does: RFC 8032's Ed25519 and RFC 7748's X25519 vectors on both, node's
// WebCrypto against them both ways (keys made by one used by the other,
// signatures made by one verified by the other, agreements both ways), the
// low-order refusals the protocol's checks rest on, the edge cases of
// "Taming the many EdDSAs" decided as the browsers and Rust's `verify_strict`
// decide them, and what a key held in the page may and may not do
// (`extractable`, its usages, structured clone — and changing what it shows).
// The repo's own vectors run through them in `e2e-soft-interop.test.ts`.
import assert from "node:assert/strict";
import test from "node:test";
import { concat, ED25519_PKCS8_PREFIX, ed25519VerifyStrict, fromHex, hex, isLowOrderX25519, isValidX25519, X25519_PKCS8_PREFIX } from "../src/lib/e2e/index.js";
import { installSoftCurves, softCurvesOn, softSubtle, uninstallSoftCurves } from "../src/lib/e2e/soft25519.js";

const native = crypto.subtle;
type Subtle = Pick<SubtleCrypto, "generateKey" | "importKey" | "exportKey" | "sign" | "verify" | "deriveBits">;
const soft = softSubtle(native, ["Ed25519", "X25519"]) as unknown as Subtle;
const both: [string, Subtle][] = [
  ["native", native],
  ["soft", soft],
];

const b64url = (b: Uint8Array) => Buffer.from(b).toString("base64url");
const ED = { name: "Ed25519" };
const X = { name: "X25519" };

/** RFC 8032 §7.1, tests 1–3 and the 1023-byte message's start (test SHA(abc) is Ed25519ph, not used). */
const RFC8032 = [
  {
    seed: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    public: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    message: "",
    signature: "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
  },
  {
    seed: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    public: "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    message: "72",
    signature: "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
  },
  {
    seed: "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
    public: "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
    message: "af82",
    signature: "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
  },
];

/** RFC 7748 §5.2: one X25519 each. */
const RFC7748 = [
  {
    scalar: "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
    u: "e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",
    out: "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",
  },
  {
    scalar: "4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d",
    u: "e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493",
    out: "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957",
  },
];

/** RFC 7748 §6.1: Alice and Bob. */
const ALICE = { scalar: "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a", public: "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a" };
const BOB = { scalar: "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb", public: "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f" };
const SHARED = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";

/** X25519 public keys of low order (every agreement with them is zero), canonical ones. */
const LOW_ORDER = [
  "0000000000000000000000000000000000000000000000000000000000000000",
  "0100000000000000000000000000000000000000000000000000000000000000",
  "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
  "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
  "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
];

/**
 * "Taming the many EdDSAs" (Chalkias, Garillot, Nikolaenko, CT-RSA 2021), the
 * twelve cases of ed25519-speccheck's `cases.json`: message, public key,
 * signature. Of them ed25519-dalek's `verify_strict` (the agent's, and what
 * `ed25519VerifyStrict` matches) accepts case 3 only. Case 4 is the one a
 * cofactored check alone accepts: its `R` has a small-order part.
 */
const SPECCHECK: [m: string, a: string, sig: string][] = [
  ["8c93255d71dcab10e8f379c26200f3c7bd5f09d9bc3068d3ef4edeb4853022b6", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a0000000000000000000000000000000000000000000000000000000000000000"],
  ["9bd9f44f4dcc75bd531b56b2cd280b0bb38fc1cd6d1230e14861d861de092e79", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa", "f7badec5b8abeaf699583992219b7b223f1df3fbbea919844e3f7c554a43dd43a5bb704786be79fc476f91d3f3f89b03984d8068dcf1bb7dfc6637b45450ac04"],
  ["aebf3f2601a0c8c5d39cc7d8911642f740b78168218da8471772b35f9d35b9ab", "f7badec5b8abeaf699583992219b7b223f1df3fbbea919844e3f7c554a43dd43", "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa8c4bd45aecaca5b24fb97bc10ac27ac8751a7dfe1baff8b953ec9f5833ca260e"],
  ["9bd9f44f4dcc75bd531b56b2cd280b0bb38fc1cd6d1230e14861d861de092e79", "cdb267ce40c5cd45306fa5d2f29731459387dbf9eb933b7bd5aed9a765b88d4d", "9046a64750444938de19f227bb80485e92b83fdb4b6506c160484c016cc1852f87909e14428a7a1d62e9f22f3d3ad7802db02eb2e688b6c52fcd6648a98bd009"],
  ["e47d62c63f830dc7a6851a0b1f33ae4bb2f507fb6cffec4011eaccd55b53f56c", "cdb267ce40c5cd45306fa5d2f29731459387dbf9eb933b7bd5aed9a765b88d4d", "160a1cb0dc9c0258cd0a7d23e94d8fa878bcb1925f2c64246b2dee1796bed5125ec6bc982a269b723e0668e540911a9a6a58921d6925e434ab10aa7940551a09"],
  ["e47d62c63f830dc7a6851a0b1f33ae4bb2f507fb6cffec4011eaccd55b53f56c", "cdb267ce40c5cd45306fa5d2f29731459387dbf9eb933b7bd5aed9a765b88d4d", "21122a84e0b5fca4052f5b1235c80a537878b38f3142356b2c2384ebad4668b7e40bc836dac0f71076f9abe3a53f9c03c1ceeeddb658d0030494ace586687405"],
  ["85e241a07d148b41e47d62c63f830dc7a6851a0b1f33ae4bb2f507fb6cffec40", "442aad9f089ad9e14647b1ef9099a1ff4798d78589e66f28eca69c11f582a623", "e96f66be976d82e60150baecff9906684aebb1ef181f67a7189ac78ea23b6c0e547f7690a0e2ddcd04d87dbc3490dc19b3b3052f7ff0538cb68afb369ba3a514"],
  ["85e241a07d148b41e47d62c63f830dc7a6851a0b1f33ae4bb2f507fb6cffec40", "442aad9f089ad9e14647b1ef9099a1ff4798d78589e66f28eca69c11f582a623", "8ce5b96c8f26d0ab6c47958c9e68b937104cd36e13c33566acd2fe8d38aa19427e71f98a473474f2f13f06f97c20d58cc3f54b8bd0d272f42b695dd7e89a8c22"],
  ["9bedc267423725d473888631ebf45988bad3db83851ee85c85e241a07d148b41", "f7badec5b8abeaf699583992219b7b223f1df3fbbea919844e3f7c554a43dd43", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff03be9678ac102edcd92b0210bb34d7428d12ffc5df5f37e359941266a4e35f0f"],
  ["9bedc267423725d473888631ebf45988bad3db83851ee85c85e241a07d148b41", "f7badec5b8abeaf699583992219b7b223f1df3fbbea919844e3f7c554a43dd43", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffca8c5b64cd208982aa38d4936621a4775aa233aa0505711d8fdcfdaa943d4908"],
  ["e96b7021eb39c1a163b6da4e3093dcd3f21387da4cc4572be588fafae23c155b", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff", "a9d55260f765261eb9b84e106f665e00b867287a761990d7135963ee0a7d59dca5bb704786be79fc476f91d3f3f89b03984d8068dcf1bb7dfc6637b45450ac04"],
  ["39a591f5321bbe07fd5a23dc2f39d025d74526615746727ceefd6e82ae65c06f", "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff", "a9d55260f765261eb9b84e106f665e00b867287a761990d7135963ee0a7d59dca5bb704786be79fc476f91d3f3f89b03984d8068dcf1bb7dfc6637b45450ac04"],
];
/** ed25519-dalek `verify_strict` on each case (ed25519-speccheck's table). */
const DALEK_STRICT = [false, false, false, true, false, false, false, false, false, false, false, false];

const edPrivate = (s: Subtle, seed: Uint8Array, extractable = false) => s.importKey("pkcs8", concat(ED25519_PKCS8_PREFIX, seed), ED, extractable, ["sign"]);
const edPublic = (s: Subtle, raw: Uint8Array) => s.importKey("raw", raw, ED, true, ["verify"]);
const xPrivate = (s: Subtle, scalar: Uint8Array, extractable = false) => s.importKey("pkcs8", concat(X25519_PKCS8_PREFIX, scalar), X, extractable, ["deriveBits"]);
const xPublic = (s: Subtle, raw: Uint8Array) => s.importKey("raw", raw, X, true, []);
const agree = async (s: Subtle, priv: CryptoKey, peer: Uint8Array) => new Uint8Array(await s.deriveBits({ name: "X25519", public: await xPublic(s, peer) }, priv, 256));

test("RFC 8032: the same public keys and signatures, natively and in compatibility mode", async () => {
  for (const v of RFC8032) {
    for (const [name, s] of both) {
      const key = await edPrivate(s, fromHex(v.seed), true);
      const jwk = await s.exportKey("jwk", key);
      assert.equal(jwk.x, b64url(fromHex(v.public)), `${name}: public key`);
      const sig = new Uint8Array(await s.sign(ED, key, fromHex(v.message)));
      assert.equal(hex(sig), v.signature, `${name}: signature`);
      const pub = await edPublic(s, fromHex(v.public));
      assert.equal(await s.verify(ED, pub, sig, fromHex(v.message)), true, `${name}: verifies`);
      assert.equal(await s.verify(ED, pub, sig, concat(fromHex(v.message), new Uint8Array([0]))), false, `${name}: another message does not`);
      const bad = sig.slice();
      bad[63] = bad[63]! ^ 0x10;
      assert.equal(await s.verify(ED, pub, bad, fromHex(v.message)), false, `${name}: a changed signature does not`);
    }
  }
});

test("RFC 7748: the same X25519, natively and in compatibility mode, both ways", async () => {
  for (const v of RFC7748) {
    for (const [name, s] of both) {
      assert.equal(hex(await agree(s, await xPrivate(s, fromHex(v.scalar)), fromHex(v.u))), v.out, name);
    }
  }
  for (const [name, s] of both) {
    const alice = await xPrivate(s, fromHex(ALICE.scalar), true);
    const bob = await xPrivate(s, fromHex(BOB.scalar), true);
    assert.equal((await s.exportKey("jwk", alice)).x, b64url(fromHex(ALICE.public)), `${name}: Alice's public key`);
    assert.equal((await s.exportKey("jwk", bob)).x, b64url(fromHex(BOB.public)), `${name}: Bob's public key`);
    assert.equal(hex(await agree(s, alice, fromHex(BOB.public))), SHARED, `${name}: Alice's side`);
    assert.equal(hex(await agree(s, bob, fromHex(ALICE.public))), SHARED, `${name}: Bob's side`);
    // Fewer bits: the first of the same.
    const half = new Uint8Array(await s.deriveBits({ name: "X25519", public: await xPublic(s, fromHex(BOB.public)) }, alice, 128));
    assert.equal(hex(half), SHARED.slice(0, 32), `${name}: 128 bits`);
  }
});

test("keys made by one work in the other: signatures verify across, agreements match both ways, PKCS #8 carries over", async () => {
  const msg = crypto.getRandomValues(new Uint8Array(100));
  for (const [makerName, maker, otherName, other] of [
    ["native", native, "soft", soft],
    ["soft", soft, "native", native],
  ] as const) {
    const pair = (await maker.generateKey(ED, true, ["sign", "verify"])) as CryptoKeyPair;
    const raw = new Uint8Array(await maker.exportKey("raw", pair.publicKey));
    const pkcs8 = new Uint8Array(await maker.exportKey("pkcs8", pair.privateKey));
    const sig = new Uint8Array(await maker.sign(ED, pair.privateKey, msg));
    assert.equal(await other.verify(ED, await edPublic(other, raw), sig, msg), true, `${otherName} verifies ${makerName}'s signature`);
    // The same key in the other: the same (deterministic) signature.
    const again = await other.importKey("pkcs8", pkcs8, ED, false, ["sign"]);
    assert.equal(hex(new Uint8Array(await other.sign(ED, again, msg))), hex(sig), `${otherName} signs as ${makerName} did`);

    const mine = (await maker.generateKey(X, true, ["deriveBits"])) as CryptoKeyPair;
    const theirs = (await other.generateKey(X, true, ["deriveBits"])) as CryptoKeyPair;
    const minePub = new Uint8Array(await maker.exportKey("raw", mine.publicKey));
    const theirsPub = new Uint8Array(await other.exportKey("raw", theirs.publicKey));
    const a = await agree(maker, mine.privateKey, theirsPub);
    const b = await agree(other, theirs.privateKey, minePub);
    assert.equal(hex(a), hex(b), `${makerName} and ${otherName} agree`);
    const moved = await other.importKey("pkcs8", new Uint8Array(await maker.exportKey("pkcs8", mine.privateKey)), X, false, ["deriveBits"]);
    assert.equal(hex(await agree(other, moved, theirsPub)), hex(a), `${makerName}'s X25519 key, moved, agrees the same`);
  }
});

test("Taming the many EdDSAs: every case decided as natively and as Rust's verify_strict — a small-order part in R included", async () => {
  // The protocol's verification (`ed25519VerifyStrict`: its own checks, then WebCrypto's), natively first.
  const natively = [];
  for (const [m, a, sig] of SPECCHECK) natively.push(await ed25519VerifyStrict(fromHex(a), fromHex(m), fromHex(sig)));
  assert.deepEqual(natively, DALEK_STRICT, "native");
  installSoftCurves(["Ed25519"]);
  try {
    const inCompat = [];
    for (const [m, a, sig] of SPECCHECK) inCompat.push(await ed25519VerifyStrict(fromHex(a), fromHex(m), fromHex(sig)));
    assert.deepEqual(inCompat, DALEK_STRICT, "compatibility mode");
  } finally {
    uninstallSoftCurves();
  }
  // Case 4 straight to the JavaScript verify, past the protocol's own checks: the cofactorless equation refuses it, as the browsers' do.
  const [m, a, sig] = SPECCHECK[4]!;
  assert.equal(await soft.verify(ED, await edPublic(soft, fromHex(a)), fromHex(sig), fromHex(m)), false, "soft, case 4");
  assert.equal(await native.verify(ED, await edPublic(native, fromHex(a)), fromHex(sig), fromHex(m)), false, "native, case 4");
  // And case 3, valid everywhere, verifies in both.
  const [m3, a3, sig3] = SPECCHECK[3]!;
  for (const [name, s] of both) assert.equal(await s.verify(ED, await edPublic(s, fromHex(a3)), fromHex(sig3), fromHex(m3)), true, `${name}, case 3`);
});

test("a low-order X25519 key is refused in compatibility mode as natively, and the protocol's checks say so", async () => {
  const mine = (await soft.generateKey(X, false, ["deriveBits"])) as CryptoKeyPair;
  const nativeMine = (await native.generateKey(X, false, ["deriveBits"])) as CryptoKeyPair;
  for (const k of LOW_ORDER) {
    await assert.rejects(agree(soft, mine.privateKey, fromHex(k)), { name: "OperationError" }, `soft refuses ${k}`);
    await assert.rejects(agree(native, nativeMine.privateKey, fromHex(k)), undefined, `native refuses ${k}`);
  }
  // keys.ts's checks, on the page's own `crypto.subtle` with the curves done
  // in JavaScript (installed before they first run, as the page does: their
  // probe key is made on first use).
  const valid = hex(new Uint8Array(await native.exportKey("raw", nativeMine.publicKey)));
  installSoftCurves(["Ed25519", "X25519"]);
  try {
    assert.deepEqual(softCurvesOn(), ["Ed25519", "X25519"]);
    for (const k of LOW_ORDER) assert.deepEqual([await isLowOrderX25519(fromHex(k)), await isValidX25519(fromHex(k))], [true, false], k);
    assert.deepEqual([await isLowOrderX25519(fromHex(valid)), await isValidX25519(fromHex(valid))], [false, true]);
  } finally {
    uninstallSoftCurves();
  }
  assert.deepEqual(softCurvesOn(), []);
});

test("a key held in the page keeps WebCrypto's rules: not extractable unless made so, its usages only, no bytes in a clone", async () => {
  const pair = (await soft.generateKey(ED, false, ["sign", "verify"])) as CryptoKeyPair;
  assert.equal(pair.privateKey.extractable, false);
  assert.equal(pair.publicKey.extractable, true, "a public key is always extractable");
  assert.deepEqual(pair.privateKey.usages, ["sign"]);
  assert.deepEqual(pair.publicKey.usages, ["verify"]);
  assert.equal(pair.privateKey.type, "private");
  assert.equal(pair.privateKey.algorithm.name, "Ed25519");
  await assert.rejects(soft.exportKey("pkcs8", pair.privateKey), { name: "InvalidAccessError" });
  await assert.rejects(soft.exportKey("jwk", pair.privateKey), { name: "InvalidAccessError" });
  await assert.rejects(soft.exportKey("pkcs8", pair.publicKey), { name: "InvalidAccessError" });
  assert.equal((await soft.exportKey("raw", pair.publicKey)).byteLength, 32);
  // A public key does not sign; a private one does not verify.
  await assert.rejects(soft.sign(ED, pair.publicKey, new Uint8Array(1)), { name: "InvalidAccessError" });
  await assert.rejects(soft.verify(ED, pair.privateKey, new Uint8Array(64), new Uint8Array(1)), { name: "InvalidAccessError" });
  // Usages outside the algorithm's are refused, as natively.
  await assert.rejects(soft.generateKey(ED, false, ["deriveBits"]), { name: "SyntaxError" });
  await assert.rejects(soft.generateKey(X, false, ["sign"]), { name: "SyntaxError" });
  await assert.rejects(soft.generateKey(ED, false, ["verify"]), { name: "SyntaxError" }, "a private key with no usage");
  // What a key shows cannot be changed, and changing a copy or a look-alike gets nothing either:
  // the methods go by what the key was made as, kept apart from the object.
  const shown = pair.privateKey as unknown as { extractable: boolean; usages: KeyUsage[]; type: string };
  assert.throws(() => {
    shown.extractable = true;
  }, TypeError);
  assert.throws(() => Object.defineProperty(shown, "extractable", { value: true }), TypeError);
  assert.throws(() => shown.usages.push("verify"), TypeError);
  assert.equal(pair.privateKey.extractable, false);
  await assert.rejects(soft.exportKey("pkcs8", pair.privateKey), { name: "InvalidAccessError" });
  const lookAlike = Object.create(Object.getPrototypeOf(pair.privateKey), { type: { value: "private" }, extractable: { value: true }, algorithm: { value: { name: "Ed25519" } }, usages: { value: ["sign"] } });
  await assert.rejects(soft.exportKey("pkcs8", lookAlike), "a look-alike is no key of these");
  await assert.rejects(soft.exportKey("pkcs8", { ...pair.privateKey, extractable: true } as unknown as CryptoKey));
  await assert.rejects(soft.sign(ED, lookAlike, new Uint8Array(1)), { name: "InvalidAccessError" });
  // Structured clone (IndexedDB, postMessage) keeps the fields and none of the bytes — and is no key either.
  const clone = structuredClone(pair.privateKey) as unknown as Record<string, unknown>;
  assert.deepEqual(Object.keys(clone).sort(), ["algorithm", "extractable", "type", "usages"]);
  assert.ok(!Object.values(clone).some((v) => v instanceof Uint8Array));
  await assert.rejects(soft.sign(ED, clone as unknown as CryptoKey, new Uint8Array(1)), { name: "InvalidAccessError" });
  // Bad input is refused, not taken.
  await assert.rejects(soft.importKey("raw", new Uint8Array(31), ED, true, ["verify"]), { name: "DataError" });
  await assert.rejects(soft.importKey("pkcs8", concat(X25519_PKCS8_PREFIX, new Uint8Array(32)), ED, false, ["sign"]), { name: "DataError" });
  // Every other algorithm is the browser's own.
  const aes = await soft.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt"]);
  assert.ok(aes instanceof CryptoKey);
});
