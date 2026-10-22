// A browser whose WebCrypto predates some algorithms, made out of node's: every
// call naming one of `names` fails as Chromium's does ("NotSupportedError:
// Algorithm: Unrecognized name"), everything else is node's own. For the
// tests of the unsupported-browser screen and of compatibility mode.

const ALG_AT: Record<string, number> = { generateKey: 0, importKey: 2, sign: 0, verify: 0, deriveBits: 0, deriveKey: 0 };

/** The error an old Chromium gives for an algorithm it does not know. */
export function unrecognized(): DOMException {
  return new DOMException("Failed to execute 'generateKey' on 'SubtleCrypto': Algorithm: Unrecognized name", "NotSupportedError");
}

const nameOf = (alg: unknown) => (typeof alg === "string" ? alg : (alg as { name?: string } | null)?.name ?? "").toLowerCase();

/**
 * Take `names` (case-insensitive) away from `subtle`, as a browser that never
 * had them; `when` narrows it to some calls (`(method, args) => boolean`).
 * Gives back the way to restore it.
 */
export function removeNative(
  names: string[] = ["Ed25519", "X25519"],
  when: (method: string, args: unknown[]) => boolean = () => true,
  subtle: SubtleCrypto = globalThis.crypto.subtle,
): () => void {
  const lower = names.map((n) => n.toLowerCase());
  const before = new Map<string, PropertyDescriptor | undefined>();
  for (const [method, at] of Object.entries(ALG_AT)) {
    before.set(method, Object.getOwnPropertyDescriptor(subtle, method));
    const own = (subtle as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>)[method]!.bind(subtle);
    Object.defineProperty(subtle, method, {
      configurable: true,
      writable: true,
      value: (...args: unknown[]) => (lower.includes(nameOf(args[at])) && when(method, args) ? Promise.reject(unrecognized()) : own(...args)),
    });
  }
  return () => {
    for (const [method, d] of before) {
      if (d) Object.defineProperty(subtle, method, d);
      else delete (subtle as unknown as Record<string, unknown>)[method];
    }
  };
}
