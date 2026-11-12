/*
 * The desktop app's update page (`features/updates/updates-view.tsx`) where the
 * app cannot update itself (`?fixture=updates&place=<place>&lang=<lang>`):
 * the line each place gets, which says what the app's menu item says
 * (`desktop/src-tauri/src/updater/words.rs`). The page is drawn from the
 * status alone, as the app's bridge hands it over.
 */
import { createRoot } from "react-dom/client";
import type { UpdatePlace, UpdateStatus } from "@/app/desktop-update";
import { UpdatesPage } from "@/features/updates/updates-view";
import { applyLanguage, type LanguagePref } from "@/i18n";

export async function mountUpdates(query: URLSearchParams): Promise<void> {
  const lang = (query.get("lang") as LanguagePref | null) ?? "en";
  await applyLanguage(lang);
  const status: UpdateStatus = {
    current: "0.2.140",
    state: "idle",
    release: null,
    percent: null,
    place: (query.get("place") as UpdatePlace | null) ?? "notWritable",
    lastChecked: null,
    problem: null,
    notes: null,
  };
  const noop = () => undefined;
  createRoot(document.getElementById("root")!).render(
    <div data-fixture-root="" className="flex h-full min-h-0 min-w-0 flex-col">
      <UpdatesPage status={status} dismissed={null} onDismiss={noop} onRestart={noop} onCheck={noop} serverVersion="0.2.140" language={lang} />
    </div>,
  );
}
