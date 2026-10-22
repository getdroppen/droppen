/*
 * Byte plumbing for the end-to-end layer: the same big-endian, length-prefixed
 * encoding the Rust crate uses (`server-rs/crates/rh-proto/src/codec.rs`), a
 * bounds-checked reader, and the conversions WebCrypto needs.
 *
 * `Bytes` is pinned to `ArrayBuffer` (not `SharedArrayBuffer`) because that is
 * what WebCrypto's `BufferSource` accepts; every buffer here is made with
 * `new Uint8Array(n)` or from a WebCrypto result, so the pin costs nothing.
 */

export type Bytes = Uint8Array<ArrayBuffer>;

export class DecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DecodeError";
  }
}

export function bytes(n: number): Bytes {
  return new Uint8Array(new ArrayBuffer(n));
}

export function concat(...parts: Uint8Array[]): Bytes {
  const out = bytes(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export function copy(data: Uint8Array): Bytes {
  const out = bytes(data.length);
  out.set(data);
  return out;
}

export function equal(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}

const encoder = new TextEncoder();
/**
 * Exactly what Rust's `String::from_utf8` takes: invalid or non-canonical
 * UTF-8 (overlong forms, surrogates, past U+10FFFF, cut short) is refused,
 * and a leading byte-order mark is kept as U+FEFF — not dropped, as a
 * `TextDecoder` does by default, which let a hub put one into a signed
 * string that this side then verified as the original and Rust refused.
 * So `utf8(fromUtf8(b))` is `b`.
 */
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export function utf8(text: string): Bytes {
  return copy(encoder.encode(text));
}

/** `data` as text, decoded as Rust decodes it (see `decoder`), or a `DecodeError`. */
export function fromUtf8(data: Uint8Array): string {
  try {
    return decoder.decode(data);
  } catch {
    throw new DecodeError("a string field is not valid UTF-8");
  }
}

export function hex(data: Uint8Array): string {
  let out = "";
  for (const b of data) out += b.toString(16).padStart(2, "0");
  return out;
}

export function fromHex(text: string): Bytes {
  if (text.length % 2 !== 0 || /[^0-9a-f]/i.test(text)) throw new DecodeError("not hex");
  const out = bytes(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function fits(v: number, max: number, what: string): number {
  if (!Number.isInteger(v) || v < 0 || v > max) throw new DecodeError(`${v} does not fit a ${what}`);
  return v;
}

/** Appends big-endian fields; the encoder half of {@link Reader}. */
export class Writer {
  private parts: Uint8Array[] = [];

  // Every fixed-width field throws on a value it cannot hold, never wraps or masks: a count or length that does not fit
  // must not encode as a smaller one (Rust's encoders refuse the same).
  u8(v: number): this {
    this.parts.push(Uint8Array.of(fits(v, 0xff, "u8")));
    return this;
  }

  u16(v: number): this {
    const b = bytes(2);
    new DataView(b.buffer).setUint16(0, fits(v, 0xffff, "u16"));
    this.parts.push(b);
    return this;
  }

  u32(v: number): this {
    const b = bytes(4);
    new DataView(b.buffer).setUint32(0, fits(v, 0xffffffff, "u32"));
    this.parts.push(b);
    return this;
  }

  /** A u64 from a safe integer (seqs, epochs, milliseconds: never near 2^53). */
  u64(v: number | bigint): this {
    const b = bytes(8);
    new DataView(b.buffer).setBigUint64(0, BigInt(v));
    this.parts.push(b);
    return this;
  }

  raw(data: Uint8Array): this {
    this.parts.push(data);
    return this;
  }

  /** `u16` length + UTF-8. */
  str16(text: string): this {
    const b = utf8(text);
    if (b.length > 0xffff) throw new DecodeError("string too long");
    return this.u16(b.length).raw(b);
  }

  /** `u8` length + UTF-8. */
  str8(text: string): this {
    const b = utf8(text);
    if (b.length > 0xff) throw new DecodeError("string too long");
    return this.u8(b.length).raw(b);
  }

  finish(): Bytes {
    return concat(...this.parts);
  }
}

/** A cursor that refuses to read past the end, or to finish with bytes left over. */
export class Reader {
  private pos = 0;
  private readonly view: DataView;

  constructor(private readonly buf: Uint8Array) {
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  get remaining(): number {
    return this.buf.length - this.pos;
  }

  bytes(n: number): Bytes {
    if (this.remaining < n) throw new DecodeError("truncated input");
    const out = copy(this.buf.subarray(this.pos, this.pos + n));
    this.pos += n;
    return out;
  }

  u8(): number {
    if (this.remaining < 1) throw new DecodeError("truncated input");
    return this.view.getUint8(this.pos++);
  }

  u16(): number {
    if (this.remaining < 2) throw new DecodeError("truncated input");
    const v = this.view.getUint16(this.pos);
    this.pos += 2;
    return v;
  }

  u32(): number {
    if (this.remaining < 4) throw new DecodeError("truncated input");
    const v = this.view.getUint32(this.pos);
    this.pos += 4;
    return v;
  }

  /** A u64 as a number; refuses values past 2^53, which no field here reaches. */
  u64(): number {
    if (this.remaining < 8) throw new DecodeError("truncated input");
    const v = this.view.getBigUint64(this.pos);
    this.pos += 8;
    if (v > BigInt(Number.MAX_SAFE_INTEGER)) throw new DecodeError("u64 field beyond 2^53");
    return Number(v);
  }

  str16(what: string, max: number): string {
    const len = this.u16();
    if (len > max) throw new DecodeError(`${what} is ${len} bytes, more than ${max}`);
    return fromUtf8(this.bytes(len));
  }

  str8(what: string, max: number): string {
    const len = this.u8();
    if (len > max) throw new DecodeError(`${what} is ${len} bytes, more than ${max}`);
    return fromUtf8(this.bytes(len));
  }

  rest(): Bytes {
    return this.bytes(this.remaining);
  }

  finish(): void {
    if (this.remaining !== 0) throw new DecodeError(`${this.remaining} trailing byte(s) after a complete value`);
  }
}

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** RFC 4648 base32, upper case, no padding (enrollment codes, fingerprints). */
export function base32(data: Uint8Array): string {
  let out = "";
  let buffer = 0;
  let bits = 0;
  for (const byte of data) {
    buffer = (buffer << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += BASE32[(buffer >> bits) & 31];
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits > 0) out += BASE32[(buffer << (5 - bits)) & 31];
  return out;
}

/** Case-insensitive; refuses characters outside the alphabet and non-zero trailing bits. */
export function fromBase32(text: string): Bytes | null {
  const out: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const c of text.toUpperCase()) {
    const v = BASE32.indexOf(c);
    if (v < 0) return null;
    buffer = (buffer << 5) | v;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 0xff);
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits >= 5 || buffer !== 0) return null;
  return copy(Uint8Array.from(out));
}
