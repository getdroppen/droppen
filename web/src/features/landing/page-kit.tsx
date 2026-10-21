import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { RegMark } from "@/components/reg-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { secondary } from "@/features/cloud/ui";
import { cn } from "@/lib/cn";
import { DOWNLOAD_PATH } from "./get-droppen";
import { APP_PATH, onPublicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_IN_PATH, usePathname } from "./route";

/*
 * The public pages' frame, in the landing page's drafting-sheet language
 * (design/DESIGN.md): a sticky header over a 1152px framed column, hairline
 * sections with registration marks at their seams, and the one-line footer.
 * The landing page (`landing.tsx`), the download page (`download.tsx`), the
 * pricing page (`pricing.tsx`), the privacy notice (`privacy.tsx`) and the
 * security design (`security/security.tsx`) are all drawn with these; the
 * header links the pricing page, the footer every page.
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

export function SectionHead({ label, title, lead, className }: { label: string; title: string; lead?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-3 px-6 pt-10 pb-8 max-md:px-4 max-md:pt-8 max-md:pb-6", className)}>
      <CapsLabel>{label}</CapsLabel>
      <h2 className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[26px]">{title}</h2>
      {lead && <p className="max-w-[38em] text-sm leading-6 text-muted">{lead}</p>}
    </div>
  );
}

export function Points({ items, className }: { items: string[]; className?: string }) {
  return (
    <ul className={cn("flex flex-col gap-2.5", className)}>
      {items.map((item) => (
        <li key={item} className="flex gap-2.5 text-xs leading-5 text-muted">
          <span className="mt-[7px] size-1.5 shrink-0 bg-fg" />
          <span>{item}</span>
        </li>
      ))}
    </ul>
  );
}

/** A link to another public page, marked when it is the page shown. */
function PageLink({ path, label, className, testId }: { path: string; label: string; className: string; testId?: string }) {
  const here = usePathname().replace(/\/+$/, "") === path;
  return (
    <a href={path} onClick={onPublicLink(path)} aria-current={here ? "page" : undefined} data-testid={testId} className={cn(className, here && "text-fg")}>
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
        <a href="/" onClick={onPublicLink("/")} className="flex items-center gap-3" aria-label="Droppen">
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
          <a href={APP_PATH} onClick={onPublicLink(APP_PATH)} className={secondary}>
            {t("Open app")}
          </a>
        ) : (
          <a href={SIGN_IN_PATH} onClick={onPublicLink(SIGN_IN_PATH)} className={secondary}>
            {t("Sign in")}
          </a>
        )}
        <RegMark className="-bottom-[5px] -left-[5px]" />
        <RegMark className="-right-[5px] -bottom-[5px]" />
      </div>
    </header>
  );
}

/** The last line of every public page: the mark, and the other public pages. */
export function FooterLine() {
  const { t } = useTranslation();
  return (
    <Section>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-6 py-5 max-md:px-4">
        <span className="flex items-center gap-2 font-mono text-2xs text-faint">
          <BrandMark size={12} />© 2026 Droppen · droppen.app
        </span>
        <a href={DOWNLOAD_PATH} onClick={onPublicLink(DOWNLOAD_PATH)} data-testid="footer-download" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Download Droppen")}
        </a>
        <PageLink path={PRICING_PATH} label={t("Pricing")} testId="footer-pricing" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={PRIVACY_PATH} label={t("Privacy notice")} testId="footer-privacy" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={SECURITY_PATH} label={t("Security")} testId="footer-security" className="font-mono text-2xs text-muted underline-offset-4 hover:text-fg hover:underline" />
      </div>
    </Section>
  );
}
