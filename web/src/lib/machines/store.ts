/*
 * A value that changes, and a way to hear it change: what a machine's
 * {@link MachineInfo} and a shell's list of machines are (`runtime.ts`).
 * React reads one with {@link useStore} (`useSyncExternalStore`), so a view
 * re-renders when the value is replaced and not otherwise.
 */
import { useSyncExternalStore } from "react";

export interface Store<T> {
  get(): T;
  subscribe(fn: () => void): () => void;
}

export interface WritableStore<T> extends Store<T> {
  /** Replace the value; listeners hear of it only when `equal` says it changed (default `Object.is`). */
  set(next: T): void;
}

export function createStore<T>(initial: T, equal: (a: T, b: T) => boolean = Object.is): WritableStore<T> {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    set(next) {
      if (equal(value, next)) return;
      value = next;
      // Copied first: a listener may unsubscribe as it runs.
      for (const fn of [...listeners]) {
        try {
          fn();
        } catch {
          // one listener failing must not starve the others
        }
      }
    },
  };
}

/** A store whose value never changes. */
export function constantStore<T>(value: T): Store<T> {
  return { get: () => value, subscribe: () => () => undefined };
}

/** `store`'s value, re-rendering when it is replaced. */
export function useStore<T>(store: Store<T>): T {
  return useSyncExternalStore(store.subscribe, store.get, store.get);
}

/** Two objects with the same own fields, each the same under `Object.is` (one level deep). */
export function shallowEqual<T extends object>(a: T, b: T): boolean {
  if (a === b) return true;
  const ka = Object.keys(a) as (keyof T)[];
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => Object.hasOwn(b, k) && Object.is(a[k], b[k]));
}
