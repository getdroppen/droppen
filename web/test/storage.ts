// A `Storage` kept in memory: the page's `localStorage`/`sessionStorage` in the
// tests, and the small store a preference reader is handed. Every test file
// used to carry its own copy; `values` is public because a test sometimes
// reads the whole store (`values.has`, `values.clear`).
export class MemoryStorage implements Storage {
  readonly values = new Map<string, string>();

  constructor(initial: Record<string, string> = {}) {
    for (const [key, value] of Object.entries(initial)) this.values.set(key, value);
  }

  get length(): number {
    return this.values.size;
  }

  key(index: number): string | null {
    return [...this.values.keys()][index] ?? null;
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, String(value));
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }

  clear(): void {
    this.values.clear();
  }

  /** Everything kept, key-sorted, for a test that pins a whole store. */
  snapshot(): Record<string, string> {
    return Object.fromEntries([...this.values].sort(([a], [b]) => a.localeCompare(b)));
  }
}
