import { lazy, Suspense } from "react";
import type { HubApi } from "@/lib/hub/api";
import type { FrontPage } from "./route";
import { useSignupMode } from "./signup-mode";

/** The landing page (signed out, at `/`): its own chunk, so the app's boot never carries it. */
const LandingPage = lazy(() => import("@/features/landing/landing"));
/** The download page (`/download`, signed in or not): its own chunk too. */
const DownloadPage = lazy(() => import("@/features/landing/download"));
/** The pricing page with the waitlist (`/pricing`) and the privacy notice (`/privacy`), signed in or not: chunks of their own. */
const PricingPage = lazy(() => import("@/features/landing/pricing"));
const PrivacyPage = lazy(() => import("@/features/landing/privacy"));
/** How the encryption works (`/security`), signed in or not: its own chunk, with its own words. */
const SecurityPage = lazy(() => import("@/features/landing/security/security"));
/** The terms of service (`/terms`), signed in or not: its own chunk, with its own words, too. */
const TermsPage = lazy(() => import("@/features/landing/terms/terms"));

/** One of the pages that are pages of their own, signed in or not. */
export function FrontPageView({ page, signedIn, api }: { page: FrontPage; signedIn: boolean; api: HubApi }) {
  // Sign-up is open, as `SIGNUP_CONFIG` says: the pages have nothing to ask the hub.
  const signup = useSignupMode();
  return (
    <Suspense fallback={<div className="h-full w-full bg-bg" />}>
      {page === "download" ? (
        <DownloadPage signedIn={signedIn} signup={signup} />
      ) : page === "pricing" ? (
        <PricingPage signedIn={signedIn} api={api} signup={signup} />
      ) : page === "privacy" ? (
        <PrivacyPage signedIn={signedIn} />
      ) : page === "security" ? (
        <SecurityPage signedIn={signedIn} />
      ) : page === "terms" ? (
        <TermsPage signedIn={signedIn} />
      ) : (
        <LandingPage signedIn={signedIn} signup={signup} />
      )}
    </Suspense>
  );
}
