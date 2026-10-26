/*
 * The setup page (`features/auth/setup-view.tsx`): the first account of a
 * device whose Droppen server has none yet — made by its owner, on a page
 * opened signed in from the device itself (`droppen open`), or
 * with the one-time setup code the server keeps. Kept
 * apart from the main tables and merged under them in `lang-*.ts` (a key the
 * main tables already translate keeps that translation); keys are the English
 * source strings, as everywhere.
 *
 * The rule sentences are the server's own `hint`s for `invalid_account`, word
 * for word, so a refusal the page did not catch first reads in the reader's
 * language too.
 */

export { setupZhCN } from "./setup.zh-CN";
export { setupZhTW } from "./setup.zh-TW";
export { setupJa } from "./setup.ja";
