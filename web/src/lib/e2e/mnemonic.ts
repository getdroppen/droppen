/*
 * The recovery key as words (`droppen-proto/src/mnemonic.rs`): BIP-39's encoding of
 * `RK` itself — its bytes then the first `bits / 32` bits of their SHA-256,
 * in 11-bit indices into the English list. No PBKDF2: the words are `RK`.
 */
import { type Bytes, bytes } from "./bytes";
import { BIP39_ENGLISH } from "./wordlist";

export class MnemonicError extends Error {
  constructor(
    readonly kind: "wordCount" | "unknownWord" | "checksum" | "length",
    message: string,
    readonly word?: string,
  ) {
    super(message);
    this.name = "MnemonicError";
  }
}

const INDEX = new Map(BIP39_ENGLISH.map((w, i) => [w, i]));

async function sha256(data: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", Uint8Array.from(data)));
}

/** `entropy` (16–32 bytes, a multiple of 4) as words. */
export async function mnemonicFromEntropy(entropy: Uint8Array): Promise<string[]> {
  if (entropy.length < 16 || entropy.length > 32 || entropy.length % 4 !== 0) throw new MnemonicError("length", "a recovery key is 16 to 32 bytes, a multiple of 4");
  const hash = await sha256(entropy);
  const checksumBits = (entropy.length * 8) / 32;
  const bits: number[] = [];
  for (const byte of [...entropy, hash[0]!]) for (let i = 7; i >= 0; i--) bits.push((byte >> i) & 1);
  bits.length = entropy.length * 8 + checksumBits;
  const words: string[] = [];
  for (let i = 0; i < bits.length; i += 11) words.push(BIP39_ENGLISH[bits.slice(i, i + 11).reduce((acc, b) => (acc << 1) | b, 0)]!);
  return words;
}

/** Normalise what a person typed: lower case, single spaces. */
export function normalizeWords(phrase: string): string[] {
  return phrase.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

/** Words back to the entropy, the checksum checked. */
export async function entropyFromMnemonic(phrase: string): Promise<Bytes> {
  const words = normalizeWords(phrase);
  if (![12, 15, 18, 21, 24].includes(words.length)) throw new MnemonicError("wordCount", `a recovery key is 12, 15, 18, 21 or 24 words, not ${words.length}`);
  const bits: number[] = [];
  for (const word of words) {
    const index = INDEX.get(word);
    if (index === undefined) throw new MnemonicError("unknownWord", `"${word}" is not a recovery word`, word);
    for (let i = 10; i >= 0; i--) bits.push((index >> i) & 1);
  }
  const checksumBits = (words.length * 11) / 33;
  const entropyBits = bits.length - checksumBits;
  const entropy = bytes(entropyBits / 8);
  for (let i = 0; i < entropy.length; i++) entropy[i] = bits.slice(i * 8, i * 8 + 8).reduce((acc, b) => (acc << 1) | b, 0);
  const hash = await sha256(entropy);
  for (let i = 0; i < checksumBits; i++) {
    if (((hash[0]! >> (7 - i)) & 1) !== bits[entropyBits + i]) {
      entropy.fill(0);
      throw new MnemonicError("checksum", "the words do not check out: one is wrong or out of place");
    }
  }
  return entropy;
}

/** Whether `word` is a recovery word (for autocomplete and live checks). */
export function isMnemonicWord(word: string): boolean {
  return INDEX.has(word.trim().toLowerCase());
}

/** The recovery words starting with `prefix` (at most `limit`). */
export function mnemonicCompletions(prefix: string, limit = 6): string[] {
  const p = prefix.trim().toLowerCase();
  if (!p) return [];
  return BIP39_ENGLISH.filter((w) => w.startsWith(p)).slice(0, limit);
}
