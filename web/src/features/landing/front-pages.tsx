import { lazy, Suspense } from "react";
import type { HubApi } from "@/lib/hub/api";
import type { FrontPage } from "./route";

/** The landing page (signed out, at `/`): its own chunk, so the app's boot never carries it. */
const LandingPage = lazy(() => import("@/features/landing/landing"));
/** The download page (`/download`, signed in or not): its own chunk too. */
const DownloadPage = lazy(() => import("@/features/landing/download"));
/** The pricing page with the waitlist (`/pricing`) and the privacy notice (`/privacy`), signed in or not: chunks of their own. */
const PricingPage = lazy(() => import("@/features/landing/pricing"));
const PrivacyPage = lazy(() => import("@/features/landing/privacy"));
/** How the encryption works (`/security`), signed in or not: its own chunk, with its own words. */
const SecurityPage = lazy(() => import("@/features/landing/security/security"));

/** One of the pages that are pages of their own, signed in or not. */
export function FrontPageView({ page, signedIn, api }: { page: FrontPage; signedIn: boolean; api: HubApi }) {
  return (
    <Suspense fallback={<div className="h-full w-full bg-bg" />}>
      {page === "download" ? (
        <DownloadPage signedIn={signedIn} />
      ) : page === "pricing" ? (
        <PricingPage signedIn={signedIn} api={api} />
      ) : page === "privacy" ? (
        <PrivacyPage signedIn={signedIn} />
      ) : page === "security" ? (
        <SecurityPage signedIn={signedIn} />
      ) : (
        <LandingPage signedIn={signedIn} />
      )}
    </Suspense>
  );
}
