/*
 * Signing a device in to Droppen through the system browser (the browser
 * hand-off, docs/local-aggregator.md §4.9): "continue in your browser" on the
 * device's page (`features/link/browser-sign-in.tsx`), droppen.app's `/link`
 * answering through a tab (`features/link/link-popup.tsx`), and the device's
 * `/link/return` (`features/link/link-return.tsx`). Merged into the link
 * tables (`link.ts`); keys are the English source strings, as everywhere.
 */

export { linkHandoffZhCN } from "./link-handoff.zh-CN";
export { linkHandoffZhTW } from "./link-handoff.zh-TW";
export { linkHandoffJa } from "./link-handoff.ja";
