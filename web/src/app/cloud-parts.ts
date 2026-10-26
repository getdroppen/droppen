/*
 * What of cloud mode the app's own views show: the bar above them, the
 * onboarding hints, the line over a session copy the retention window
 * trimmed. Each lives with cloud mode (`features/cloud`) and renders nothing
 * without droppen.app's shell; the cloud app hands them over here as its
 * chunk loads (`cloud-app.tsx`), before it renders the router. So the views
 * every mode shares draw them without importing them, and the local app's
 * boot never carries them: there, {@link cloudParts} is `null` and they are
 * not drawn at all, which is what they would draw.
 */
import type { CloudBar } from "@/features/cloud/cloud-bar";
import type { AddDeviceHint, SideBySideTip } from "@/features/cloud/onboarding-hints";
import type { CopyWindowNotice } from "@/features/cloud/plan";

export interface CloudParts {
  CloudBar: typeof CloudBar;
  SideBySideTip: typeof SideBySideTip;
  AddDeviceHint: typeof AddDeviceHint;
  CopyWindowNotice: typeof CopyWindowNotice;
}

let parts: CloudParts | null = null;

/** Cloud mode's parts, once its app has loaded; `null` in local mode. */
export function cloudParts(): CloudParts | null {
  return parts;
}

export function provideCloudParts(given: CloudParts): void {
  parts = given;
}
