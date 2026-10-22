/**
 * Whether the app can sign this device in to Droppen through the system
 * browser (`docs/local-aggregator.md` §4.9, the browser hand-off): step 02's
 * "On my phone and other devices too" card can be chosen, and signs in from
 * the step (`where-sign-in.tsx`). Off, the card is shown disabled with a
 * "coming soon" line and "Only on this Mac" is the choice.
 *
 * On since the app that has the hand-off (0.2.125). The download page's
 * `APP_SIGN_IN_AVAILABLE` (`features/landing/get-droppen.ts`) is
 * the same fact for droppen.app's own copy: it turns on once that app is
 * published, since droppen.app serves people who may not have it yet.
 */
export const DESKTOP_SIGN_IN_READY = true;
