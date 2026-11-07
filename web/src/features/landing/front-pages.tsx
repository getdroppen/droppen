import { lazy, Suspense } from "react";
import { SIGNUP_CONFIG, type HubApi } from "@/lib/hub/api";
import type { FrontPage } from "./route";

const LandingPage = lazy(() => import("@/features/landing/landing"));
const DownloadPage = lazy(() => import("@/features/landing/download"));
const PricingPage = lazy(() => import("@/features/landing/pricing"));
const PrivacyPage = lazy(() => import("@/features/landing/privacy"));
const SecurityPage = lazy(() => import("@/features/landing/security/security"));
const TermsPage = lazy(() => import("@/features/landing/terms/terms"));

export function FrontPageView({ page, signedIn, api }: { page: FrontPage; signedIn: boolean; api: HubApi }) {
  return (
    <Suspense fallback={<div className="h-full w-full bg-bg" />}>
      {page === "download" ? (
        <DownloadPage signedIn={signedIn} />
      ) : page === "pricing" ? (
        <PricingPage signedIn={signedIn} api={api} signup={SIGNUP_CONFIG.signup} />
      ) : page === "privacy" ? (
        <PrivacyPage signedIn={signedIn} />
      ) : page === "security" ? (
        <SecurityPage signedIn={signedIn} />
      ) : page === "terms" ? (
        <TermsPage signedIn={signedIn} />
      ) : (
        <LandingPage signedIn={signedIn} signup={SIGNUP_CONFIG.signup} />
      )}
    </Suspense>
  );
}
