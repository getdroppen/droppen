/**
 * Pure pane-tree model shared by the ephemeral route splits (panes.tsx) and
 * persistent screens (screens.tsx). The tree is plain JSON so a screen's
 * layout can round-trip through the server unchanged.
 */
import type { MachineKey } from "@/lib/machines/key";

/**
 * What a leaf shows. A session, a chat, a terminal or a folder is on one
 * machine — `machine`, written only when it is not the home's
 * (`docs/local-aggregator.md` §6.5, §6.7 M-2): absent, it is the home's,
 * which is also the machine that keeps the screen, so a layout from before
 * machines had keys (and every layout of a page with one machine) reads and
 * writes exactly as it did. An older build reading a leaf that names another
 * machine opens its id on the home, where it is not found — and says so.
 */
export type PaneContent =
  | { kind: "main" }
  /** Choosing what to show; `machine`: the device it chooses from, when not the home. */
  | { kind: "picker"; machine?: MachineKey }
  /** Saved before plugins existed; read as the tasks plugin's checklist (`plugins/pane-content.ts`). */
  | { kind: "tasks" }
  /** A plugin's pane; `state` is the plugin's, flat and ≤ 2 KB. */
  | { kind: "plugin"; plugin: string; pane: string; state?: Record<string, string | number | boolean | null> }
  | { kind: "files"; path?: string; file?: string; machine?: MachineKey }
  | { kind: "session"; source: "claude" | "codex" | "acp"; id: string; machine?: MachineKey }
  /** A session about to be started: the empty session view, which becomes the session once its first message is sent. */
  | { kind: "new-session"; machine?: MachineKey }
  | { kind: "chat"; chatKind: "claude" | "codex" | "api"; id: string; machine?: MachineKey }
  | { kind: "terminal"; id: string; machine?: MachineKey };

/** The kinds of content that are on a machine of their own (a picker: the machine it picks from). */
export type MachineContent = Extract<PaneContent, { kind: "files" | "session" | "new-session" | "chat" | "terminal" | "picker" }>;

/** Whether `content` is on a machine of its own (a session, a chat, a terminal, a folder, or a picker choosing from one). */
export function onMachine(content: PaneContent): content is MachineContent {
  return (
    content.kind === "files" ||
    content.kind === "session" ||
    content.kind === "new-session" ||
    content.kind === "chat" ||
    content.kind === "terminal" ||
    content.kind === "picker"
  );
}

/** Whether `content` uses its machine while it is shown (everything on one but a picker). */
export function usesMachine(content: PaneContent): boolean {
  return onMachine(content) && content.kind !== "picker";
}

/** The machine `content` is about: the one it names, else `home` (content that is on no machine is the home's too). */
export function paneMachine(content: PaneContent, home: MachineKey): MachineKey {
  return onMachine(content) && typeof content.machine === "string" && content.machine ? content.machine : home;
}

/**
 * `content` on `machine`, as it is stored: naming the machine only when it
 * is not `home` (see {@link PaneContent}). Content that is on no machine
 * comes back as it is.
 */
export function withMachine(content: PaneContent, machine: MachineKey, home: MachineKey): PaneContent {
  if (!onMachine(content)) return content;
  const { machine: _named, ...rest } = content;
  return (machine === home ? rest : { ...rest, machine }) as PaneContent;
}

/** Whether any leaf of a tree names its device (any content on a device other than the home). */
export function namesMachines(node: unknown): boolean {
  if (!node || typeof node !== "object") return false;
  const n = node as { type?: unknown; content?: unknown; children?: unknown };
  if (n.type === "split" && Array.isArray(n.children)) return n.children.some(namesMachines);
  const machine = (n.content as { machine?: unknown } | undefined)?.machine;
  return typeof machine === "string" && machine !== "";
}

/** Every machine a tree's leaves are about (`home` for those that name none), the `main` leaf as `main`. */
export function treeMachines(node: PaneNode, home: MachineKey, main: MachineKey = home): Set<MachineKey> {
  const out = new Set<MachineKey>();
  const walk = (n: PaneNode) => {
    if (n.type === "split") {
      walk(n.children[0]);
      walk(n.children[1]);
    } else if (n.content.kind === "main") out.add(main);
    else if (usesMachine(n.content)) out.add(paneMachine(n.content, home));
  };
  walk(node);
  return out;
}

export type SplitDirection = "row" | "column";

export interface LeafNode {
  type: "leaf";
  id: string;
  content: PaneContent;
}

export interface SplitNode {
  type: "split";
  id: string;
  direction: SplitDirection;
  /** Share of the first child, 0..1. */
  ratio: number;
  children: [PaneNode, PaneNode];
}

export type PaneNode = LeafNode | SplitNode;

export const MAX_LEAVES = 6;
export const MIN_RATIO = 0.1;

/** Random ids: persisted trees must not collide with ids minted after a reload. */
export function newPaneId(prefix: string): string {
  return `${prefix}-${crypto.randomUUID().slice(0, 8)}`;
}

export function countLeaves(node: PaneNode): number {
  return node.type === "leaf" ? 1 : countLeaves(node.children[0]) + countLeaves(node.children[1]);
}

export function firstLeafId(node: PaneNode): string {
  return node.type === "leaf" ? node.id : firstLeafId(node.children[0]);
}

export function hasLeaf(node: PaneNode, id: string): boolean {
  if (node.type === "leaf") return node.id === id;
  return hasLeaf(node.children[0], id) || hasLeaf(node.children[1], id);
}

/** Replace the target leaf with a split of [target, fresh picker leaf]. */
export function splitLeaf(
  node: PaneNode,
  targetId: string,
  direction: SplitDirection,
  newLeafId: string,
  splitId = newPaneId("split"),
): PaneNode {
  if (node.type === "leaf") {
    if (node.id !== targetId) return node;
    return {
      type: "split",
      id: splitId,
      direction,
      ratio: 0.5,
      children: [node, { type: "leaf", id: newLeafId, content: { kind: "picker" } }],
    };
  }
  return {
    ...node,
    children: [
      splitLeaf(node.children[0], targetId, direction, newLeafId, splitId),
      splitLeaf(node.children[1], targetId, direction, newLeafId, splitId),
    ],
  };
}

/** Remove a leaf; its sibling takes the parent split's place. */
export function removeLeaf(node: PaneNode, targetId: string): PaneNode {
  if (node.type === "leaf") return node;
  const [a, b] = node.children;
  if (a.type === "leaf" && a.id === targetId) return b;
  if (b.type === "leaf" && b.id === targetId) return a;
  return { ...node, children: [removeLeaf(a, targetId), removeLeaf(b, targetId)] };
}

export function updateLeafContent(node: PaneNode, id: string, content: PaneContent): PaneNode {
  if (node.type === "leaf") return node.id === id ? { ...node, content } : node;
  return {
    ...node,
    children: [
      updateLeafContent(node.children[0], id, content),
      updateLeafContent(node.children[1], id, content),
    ],
  };
}

export function updateRatio(node: PaneNode, splitId: string, ratio: number): PaneNode {
  if (node.type === "leaf") return node;
  if (node.id === splitId) {
    return { ...node, ratio: Math.min(1 - MIN_RATIO, Math.max(MIN_RATIO, ratio)) };
  }
  return {
    ...node,
    children: [updateRatio(node.children[0], splitId, ratio), updateRatio(node.children[1], splitId, ratio)],
  };
}

/**
 * The operations a rendered pane tree needs, regardless of whether the tree
 * lives in ephemeral React state or in a persisted screen.
 */
export interface PaneTreeApi {
  root: PaneNode;
  hasSplit: boolean;
  focusedId: string;
  setFocused: (id: string) => void;
  splitFocused: (direction: SplitDirection) => void;
  closePane: (id: string) => void;
  setPaneContent: (id: string, content: PaneContent) => void;
  resizeSplit: (splitId: string, ratio: number) => void;
}

export interface PaneTreeState {
  root: PaneNode;
  focusedId: string;
}

export type PaneAction =
  | { type: "focus"; id: string }
  | { type: "split"; direction: SplitDirection; leafId: string; splitId: string }
  | { type: "close"; id: string }
  | { type: "content"; id: string; content: PaneContent }
  | { type: "resize"; id: string; ratio: number };

/** A route layout protects its main pane; a screen clears its last leaf. */
export function reducePaneTree(state: PaneTreeState, action: PaneAction, protectedId?: string): PaneTreeState {
  const { root, focusedId } = state;
  switch (action.type) {
    case "focus":
      return action.id === focusedId || !hasLeaf(root, action.id) ? state : { ...state, focusedId: action.id };
    case "split":
      if (countLeaves(root) >= MAX_LEAVES || !hasLeaf(root, focusedId)) return state;
      return { root: splitLeaf(root, focusedId, action.direction, action.leafId, action.splitId), focusedId: action.leafId };
    case "close": {
      if (action.id === protectedId || !hasLeaf(root, action.id)) return state;
      if (root.type === "leaf") {
        return root.content.kind === "picker" ? state : { root: { ...root, content: { kind: "picker" } }, focusedId: root.id };
      }
      const next = removeLeaf(root, action.id);
      return { root: next, focusedId: hasLeaf(next, focusedId) ? focusedId : firstLeafId(next) };
    }
    case "content":
      return { ...state, root: updateLeafContent(root, action.id, action.content) };
    case "resize":
      return { ...state, root: updateRatio(root, action.id, action.ratio) };
  }
}

/** The adapters only choose where state is stored; operations have one owner. */
export function paneTreeApi(state: PaneTreeState, dispatch: (action: PaneAction) => void): PaneTreeApi {
  return {
    ...state,
    hasSplit: state.root.type !== "leaf",
    setFocused: (id) => dispatch({ type: "focus", id }),
    splitFocused: (direction) => dispatch({ type: "split", direction, leafId: newPaneId("pane"), splitId: newPaneId("split") }),
    closePane: (id) => dispatch({ type: "close", id }),
    setPaneContent: (id, content) => dispatch({ type: "content", id, content }),
    resizeSplit: (id, ratio) => dispatch({ type: "resize", id, ratio }),
  };
}
