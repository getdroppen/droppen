/*
 * One SHA-256 over everything a move copies, the same on both sides: the
 * sender's of what it read, the receiver's of what it got and again of what
 * it read back from its own storage. The sender forgets its copy only when
 * the receiver's answer names the digest it sent, and only if its own store
 * still reads to it.
 *
 * The values are whatever the stores hold (structured-clone values), written
 * out canonically: keys of objects sorted, bytes in hex, numbers exactly. A
 * `CryptoKey` cannot be read, so it counts by what it says of itself — type,
 * algorithm, usages, extractable; that it still works is what the receiver's
 * sign-in proves. A value of any other kind (a `Blob`, a function) is
 * {@link Unmovable}: the move stops before anything is sent.
 */
import { hex } from "@/lib/e2e/bytes";

/** A value the move does not know how to compare: nothing is moved. */
export class Unmovable extends Error {
  constructor(message: string) {
    super(message);
    this.name = "Unmovable";
  }
}

const isCryptoKey = (v: unknown): v is CryptoKey => typeof CryptoKey !== "undefined" && v instanceof CryptoKey;

function number(v: number): string {
  if (Object.is(v, -0)) return "-0";
  return Number.isNaN(v) ? "NaN" : String(v);
}

function write(v: unknown, out: string[], depth: number): void {
  if (depth > 64) throw new Unmovable("a value nested too deep");
  if (v === undefined) return void out.push("u");
  if (v === null) return void out.push("z");
  switch (typeof v) {
    case "string":
      return void out.push(`s${JSON.stringify(v)}`);
    case "number":
      return void out.push(`n${number(v)}`);
    case "boolean":
      return void out.push(v ? "T" : "F");
    case "bigint":
      return void out.push(`i${v.toString()}`);
    case "object":
      break;
    default:
      throw new Unmovable(`a value of type ${typeof v}`);
  }
  if (isCryptoKey(v)) {
    const alg = v.algorithm as unknown as Record<string, unknown>;
    out.push(`K{type:${v.type},x:${v.extractable ? 1 : 0},use:${[...v.usages].sort().join("+")},alg:`);
    write({ ...alg }, out, depth + 1);
    return void out.push("}");
  }
  if (v instanceof ArrayBuffer) return void out.push(`B${hex(new Uint8Array(v))}`);
  if (ArrayBuffer.isView(v)) {
    const bytes = new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
    return void out.push(`V${v.constructor.name}:${hex(bytes)}`);
  }
  if (v instanceof Date) return void out.push(`D${number(v.getTime())}`);
  if (Array.isArray(v)) {
    out.push(`A${v.length}[`);
    for (let i = 0; i < v.length; i++) write(v[i], out, depth + 1);
    return void out.push("]");
  }
  if (v instanceof Map) {
    out.push(`M${v.size}[`);
    for (const [k, x] of v) {
      write(k, out, depth + 1);
      write(x, out, depth + 1);
    }
    return void out.push("]");
  }
  if (v instanceof Set) {
    out.push(`S${v.size}[`);
    for (const x of v) write(x, out, depth + 1);
    return void out.push("]");
  }
  const proto = Object.getPrototypeOf(v);
  if (proto !== Object.prototype && proto !== null) throw new Unmovable(`a value of kind ${(v as object).constructor?.name ?? "unknown"}`);
  const keys = Object.keys(v).sort();
  out.push(`O${keys.length}{`);
  for (const k of keys) {
    out.push(JSON.stringify(k), ":");
    write((v as Record<string, unknown>)[k], out, depth + 1);
  }
  out.push("}");
}

/** `v` written out canonically (see the file's comment). Throws {@link Unmovable}. */
export function canonical(v: unknown): string {
  const out: string[] = [];
  write(v, out, 0);
  return out.join("");
}

/** One record of a moved store, or one moved `localStorage` entry. */
export interface DigestEntry {
  /** `idb:<db>/<store>`, or `ls`. */
  where: string;
  key: string;
  value: unknown;
}

/** The digest of `entries`, in whatever order they come (hex). Throws {@link Unmovable}. */
export async function digestOf(entries: readonly DigestEntry[]): Promise<string> {
  const sorted = [...entries].sort((a, b) => (a.where === b.where ? (a.key < b.key ? -1 : a.key > b.key ? 1 : 0) : a.where < b.where ? -1 : 1));
  const text = `droppen/v1 move\0${sorted.map((e) => `${JSON.stringify(e.where)} ${JSON.stringify(e.key)} ${canonical(e.value)}\n`).join("")}`;
  const sum = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return hex(new Uint8Array(sum));
}
