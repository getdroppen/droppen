import { Sheet } from "@/components/sheet";
import { DroppenSection } from "@/features/link/link-settings";
import { SecuritySection } from "@/features/settings/security-section";
import { isCloud } from "@/lib/hub/config";

/**
 * Identity, in one place: this device's local login (who may use it, its
 * password, the sessions holding a token) and — on the computer's own page,
 * never on droppen.app — its Droppen link. These were two tabs that each told
 * the reader to go to the other one; the login comes first because the link
 * rests on it (`features/link/link-settings.tsx`). Inside the Droppen desktop
 * app the link signs in through the system browser
 * (`docs/local-aggregator.md` §4.9, `features/link/browser-sign-in.tsx`).
 *
 * Its own module rather than a part of `settings-view.tsx` so the render tests
 * can mount it: that page imports the plugins section, whose virtual module
 * only the Vite build provides.
 */
export function AccessSection() {
  return (
    <Sheet>
      <SecuritySection embedded />
      {!isCloud() && <DroppenSection embedded />}
    </Sheet>
  );
}
