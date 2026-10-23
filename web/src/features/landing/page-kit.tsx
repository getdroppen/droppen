import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy } from "lucide-react";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { RegMark } from "@/components/reg-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { primary, secondary } from "@/features/cloud/ui";
import { cn } from "@/lib/cn";
import { appHost, appOrigin, appUrl } from "@/lib/hub/hosts";
import { DOWNLOAD_PATH } from "./get-droppen";
import { APP_PATH, publicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_IN_PATH, TERMS_PATH, usePathname } from "./route";

/*
 * The public pages' frame, in the landing page's drafting-sheet language: a
 * sticky header over a 1152px framed column, hairline
 * sections with registration marks at their seams, and the one-line footer.
 * The landing page (`landing.tsx`), the download page (`download.tsx`), the
 * pricing page (`pricing.tsx`), the privacy notice (`privacy.tsx`), the
 * security design (`security/security.tsx`) and the terms of service
 * (`terms/terms.tsx`) are all drawn with these; the
 * header links the pricing page, the footer every page. A command to paste
 * (`CommandLine`) looks the same on the landing page's hero and the download
 * page.
 */

/** The two marks where a section's top line meets the frame. */
export function Seam() {
  return (
    <>
      <RegMark className="-top-[5px] -left-[5px]" />
      <RegMark className="-top-[5px] -right-[5px]" />
    </>
  );
}

export function Section({ id, children, className }: { id?: string; children: ReactNode; className?: string }) {
  return (
    <section id={id} className={cn("relative scroll-mt-14 border-t border-border", className)}>
      <Seam />
      {children}
    </section>
  );
}

export function SectionHead({ label, title, lead, className }: { label: string; title: ReactNode; lead?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-3 px-6 pt-10 pb-8 max-md:px-4 max-md:pt-8 max-md:pb-6", className)}>
      <CapsLabel>{label}</CapsLabel>
      <h2 className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[26px]">{title}</h2>
      {lead && <p className="max-w-[38em] text-sm leading-6 text-muted">{lead}</p>}
    </div>
  );
}

export function Points({ items, className, render }: { items: string[]; className?: string; render?: (item: string) => ReactNode }) {
  return (
    <ul className={cn("flex flex-col gap-2.5", className)}>
      {items.map((item) => (
        <li key={item} className="flex gap-2.5 text-xs leading-5 text-muted">
          <span className="mt-[7px] size-1.5 shrink-0 bg-fg" />
          <span>{render ? render(item) : item}</span>
        </li>
      ))}
    </ul>
  );
}

/** A command, as the page shows it: mono on the raised paper, selectable whole, with its copy button. */
export function CommandLine({ command, label, testId, primaryCopy = false }: { command: string; label: string; testId?: string; primaryCopy?: boolean }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex min-w-0 items-stretch border border-border-strong bg-raised max-sm:flex-col">
      <code data-testid={testId} className="min-w-0 flex-1 px-3 py-2.5 font-mono text-xs leading-5 whitespace-pre-wrap text-fg select-all [overflow-wrap:anywhere]">
        <span aria-hidden="true" className="mr-2 text-faint select-none">$</span>
        {command}
      </code>
      <button
        type="button"
        data-variant={primaryCopy ? "primary" : undefined}
        data-testid={testId ? `${testId}-copy` : undefined}
        aria-label={label}
        onClick={() => {
          void navigator.clipboard?.writeText(command).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
        className={cn(primaryCopy ? primary : secondary, "h-auto shrink-0 rounded-none border-0 max-sm:h-9 sm:border-l sm:border-border-strong")}
      >
        {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
        {copied ? t("copied") : label}
      </button>
    </div>
  );
}

/** A link to another public page, marked when it is the page shown. */
function PageLink({ path, label, className, testId }: { path: string; label: string; className: string; testId?: string }) {
  const here = usePathname().replace(/\/+$/, "") === path;
  return (
    <a {...publicLink(path)} aria-current={here ? "page" : undefined} data-testid={testId} className={cn(className, here && "text-fg")}>
      {label}
    </a>
  );
}

/**
 * The public pages' header: the mark (home), the page's own anchors, pricing
 * (from 640px: below, the footer has it), language and theme, and the way in
 * (or back into the app).
 */
export function PublicHeader({ signedIn, nav = [] }: { signedIn: boolean; nav?: { href: string; label: string }[] }) {
  const { t } = useTranslation();
  return (
    <header className="sticky top-0 z-40 h-14 shrink-0 border-b border-border bg-bg md:px-8">
      <div className="relative mx-auto flex h-full max-w-[1152px] items-center gap-3 px-6 max-md:px-4 md:border-x md:border-border">
        <a {...publicLink("/")} className="flex items-center gap-3" aria-label="Droppen">
          <BrandMark size={20} />
          <span className="font-mono text-sm font-medium text-fg">Droppen</span>
        </a>
        {nav.length > 0 && (
          <nav aria-label={t("On this page")} className="ml-6 flex items-center gap-5 max-lg:hidden">
            {nav.map((n) => (
              <a key={n.href} href={n.href} className="label-caps text-2xs text-muted hover:text-fg">
                {n.label}
              </a>
            ))}
          </nav>
        )}
        <span className="flex-1" />
        <PageLink path={PRICING_PATH} label={t("Pricing")} testId="header-pricing" className="label-caps mr-1 text-2xs whitespace-nowrap text-muted hover:text-fg max-sm:hidden" />
        <PrefsControls />
        {signedIn ? (
          <a {...publicLink(APP_PATH)} className={secondary}>
            {t("Open app")}
          </a>
        ) : (
          <a {...publicLink(SIGN_IN_PATH)} className={secondary}>
            {t("Sign in")}
          </a>
        )}
        <RegMark className="-bottom-[5px] -left-[5px]" />
        <RegMark className="-right-[5px] -bottom-[5px]" />
      </div>
    </header>
  );
}

/** The licences of the npm packages in this build, beside `index.html` in every build. */
export const NOTICES_PATH = "/THIRD_PARTY_NOTICES.txt";

/** Where the source of this web app (AGPL-3.0-only) and of the local program is. */
export const SOURCE_URL = "https://github.com/getdroppen/droppen";

/** Where to report a vulnerability, besides GitHub's private vulnerability reporting (SECURITY.md). */
export const SECURITY_EMAIL = "security@droppen.app";

/** The last line of every public page: the mark, the other public pages, the source and the licences. */
export function FooterLine() {
  const { t } = useTranslation();
  return (
    <Section>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-6 py-5 max-md:px-4">
        <span className="flex items-center gap-2 font-mono text-2xs text-faint">
          <BrandMark size={12} />© 2026 Droppen · droppen.app
          {/* The app, where the hub keeps it on a host of its own (my.droppen.app): a link there from these pages. */}
          {appOrigin() && (
            <>
              {" · "}
              <a href={appUrl("/")} data-testid="footer-app" className="text-faint underline-offset-4 hover:text-fg hover:underline">
                {appHost()}
              </a>
            </>
          )}
        </span>
        <a {...publicLink(DOWNLOAD_PATH)} data-testid="footer-download" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Download Droppen")}
        </a>
        <PageLink path={PRICING_PATH} label={t("Pricing")} testId="footer-pricing" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={PRIVACY_PATH} label={t("Privacy notice")} testId="footer-privacy" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={SECURITY_PATH} label={t("Security")} testId="footer-security" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={TERMS_PATH} label={t("Terms of Service")} testId="footer-terms" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <a href={SOURCE_URL} data-testid="footer-source" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Source code")}
        </a>
        {/* Every build writes it (web/build/third-party-notices.mjs): the licences of the packages in this page. */}
        <a href={NOTICES_PATH} data-testid="footer-licences" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Third-party licences")}
        </a>
      </div>
    </Section>
  );
}
