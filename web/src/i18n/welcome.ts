/*
 * The first-run guide of the Droppen app (`features/setup/`, `/welcome`):
 * every string of the guide as first designed, so the
 * steps built later (01 AI tools, 03 first session) only add what they invent.
 * Kept apart from the main tables and merged under them in `lang-*.ts` (a key
 * the main tables already translate keeps that translation); keys are the
 * English source strings, as everywhere.
 *
 * Generated once from a table of the guide's copy
 * (one row per string: screen, key, en, zh-CN, zh-TW, ja): the key is the
 * English text with `{name}` written as `{{name}}`; rows with no English are
 * left out; the trailing comment is the row's key in that table
 * (`where.title`). Changes from that table: the header reads "Get
 * started with Droppen" (「设置 Droppen」 read as Settings); the sign-in card's
 * "coming soon" line, step 04's menu-bar line and the resume bar's "1 step"
 * are new; nothing here changes a translation the app already has
 * (`test/setup-i18n.test.ts`), so "now" and "on" (a time, online) are left
 * out, "done" is 完成 as in the main tables and two ja strings follow the
 * app's (サインイン, 件のセッション). Where two rows share their English the
 * first one's words are kept: "Name" is 名称 (step 01's account name; step
 * 02's network sign-in name needs a key of its own). Edit by hand from here
 * on.
 */

export { welcomeZhCN } from "./welcome.zh-CN";
export { welcomeZhTW } from "./welcome.zh-TW";
export { welcomeJa } from "./welcome.ja";
