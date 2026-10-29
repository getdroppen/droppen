import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Menu } from "@base-ui-components/react/menu";
import { Check, Copy, LogIn, UserPlus, UserRound } from "lucide-react";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { RegMark } from "@/components/reg-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { primary, secondary } from "@/features/cloud/ui";
import { cn } from "@/lib/cn";
import { appHost, appOrigin, appUrl } from "@/lib/hub/hosts";
import { DOWNLOAD_PATH } from "./get-droppen";
import { APP_PATH, publicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, TERMS_PATH, usePathname } from "./route";

/*
 * The public pages' frame, in the landing page's drafting-sheet language: a
 * sticky header over a 1360px framed column, hairline
 * sections with registration marks at their seams, and the one-line footer.
 * The landing page (`landing.tsx`), the download page (`download.tsx`), the
 * pricing page (`pricing.tsx`), the privacy notice (`privacy.tsx`), the
 * security design (`security/security.tsx`) and the terms of service
 * (`terms/terms.tsx`) are all drawn with these; the
 * header links the pricing page, the footer every page. A command to paste
 * (`CommandLine`) looks the same on the landing page's hero and the download
 * page.
 */

/**
 * The app's buttons at the public pages' size: `primary` and `secondary` carry
 * the app's own 13px label, and the theme's `text-base` page does not borrow it.
 * `cn` drops the narrower size in favour of the page's 14px.
 */
export const landingPrimary = cn(primary, "text-sm");
export const landingSecondary = cn(secondary, "text-sm");

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

/**
 * The caps labels' letter spacing is for Latin capitals; Chinese and Japanese
 * set without it, as their headlines do (产品, not 产 品).
 */
export const cjkTracking = "[&:lang(ja)]:tracking-normal [&:lang(zh)]:tracking-normal";

/** A section's head; `label`, the caps line over the title, only where the title does not already say what the section is. */
export function SectionHead({ label, title, lead, className }: { label?: string; title: ReactNode; lead?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-3 px-6 pt-10 pb-8 max-md:px-4 max-md:pt-8 max-md:pb-6", className)}>
      {label && <CapsLabel face="sans" className={cn(cjkTracking, "text-xs font-semibold")}>{label}</CapsLabel>}
      <h2 className="text-3xl leading-tight font-normal tracking-tight text-balance text-fg max-[359px]:text-pretty [word-break:auto-phrase] md:[&:lang(zh)]:[word-break:keep-all] max-md:text-2xl">{title}</h2>
      {lead && <p className="max-w-[38em] text-sm leading-6 text-muted">{lead}</p>}
    </div>
  );
}

/** A list of points; `large`: the landing page's reading size (14px), else the 12px of the denser pages. */
export function Points({ items, className, render, large }: { items: string[]; className?: string; render?: (item: string) => ReactNode; large?: boolean }) {
  return (
    <ul className={cn("flex flex-col gap-2.5", className)}>
      {items.map((item) => (
        <li key={item} className={cn("flex gap-2.5 text-muted", large ? "text-sm leading-6 text-pretty" : "text-xs leading-5")}>
          <span className={cn("size-1.5 shrink-0 bg-fg", large ? "mt-[9px]" : "mt-[7px]")} />
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
        className={cn(primaryCopy ? landingPrimary : landingSecondary, "h-auto shrink-0 rounded-none border-0 max-sm:h-9 sm:border-l sm:border-border-strong")}
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
    <a {...publicLink(path)} aria-current={here ? "page" : undefined} data-testid={testId} className={cn(className, here && "text-fg font-medium")}>
      {label}
    </a>
  );
}

/** GitHub's mark (Octicons' mark-github, MIT), for the header's link to the client's source. */
function GitHubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" className={className} fill="currentColor">
      <path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </svg>
  );
}

/** Popup styling matching LanguageMenu's public look */
const MENU_POPUP =
  "popup-motion max-h-[min(var(--available-height),20rem)] min-w-[10rem] max-w-[20rem] overflow-y-auto border border-border-strong bg-bg py-1.5 outline-none";
const MENU_ITEM =
  "flex min-h-8 w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-sm text-fg outline-none pointer-coarse:min-h-11 data-[highlighted]:bg-hover";

/**
 * The public pages' header: the mark (home), the site's other pages (download,
 * pricing, security; from 768px, below which the footer has them), the
 * client's source on GitHub (from 640px), language and theme, and the way in
 * (or back into the app).
 */
export function PublicHeader({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  const pages = [
    { path: DOWNLOAD_PATH, label: t("Download"), testId: "header-download" },
    { path: PRICING_PATH, label: t("Pricing"), testId: "header-pricing" },
    { path: SECURITY_PATH, label: t("Security"), testId: "header-security" },
  ];
  return (
    <header className="sticky top-0 z-40 h-14 shrink-0 border-b border-border bg-bg md:px-8">
      <div className="relative mx-auto flex h-full max-w-[1360px] items-center gap-3 px-6 max-md:px-4 md:border-x md:border-border">
        <a {...publicLink("/")} className="flex items-center gap-3" aria-label="Droppen">
          <BrandMark size={20} />
          <span className="font-mono text-base font-medium text-fg">Droppen</span>
        </a>
        <nav aria-label={t("Site navigation")} className="ml-6 flex items-center gap-6 max-md:hidden">
          {pages.map((page) => (
            <PageLink
              key={page.path}
              path={page.path}
              label={page.label}
              testId={page.testId}
              className="text-base whitespace-nowrap text-muted transition-colors hover:text-fg"
            />
          ))}
        </nav>
        <span className="flex-1" />
        <a
          href={SOURCE_URL}
          data-testid="header-source"
          aria-label={t("Client source code")}
          title={t("Client source code")}
          className="flex size-9 shrink-0 items-center justify-center border border-border-strong bg-bg text-muted hover:bg-hover hover:text-fg max-sm:hidden"
        >
          <GitHubMark className="size-4" />
        </a>
        <PrefsControls />
        <Menu.Root>
          <Menu.Trigger
            aria-label={signedIn ? t("Open app") : t("Sign in")}
            title={signedIn ? t("Open app") : t("Sign in")}
            className="flex size-9 shrink-0 items-center justify-center border border-border-strong bg-bg text-muted transition-colors hover:bg-hover hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg"
          >
            <UserRound className="size-3.5" />
          </Menu.Trigger>
          <Menu.Portal keepMounted>
            <Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-50 outline-none">
              <Menu.Popup className={MENU_POPUP}>
                {signedIn ? (
                  <Menu.Item
                    closeOnClick
                    render={<a {...publicLink(APP_PATH)} />}
                    className={MENU_ITEM}
                  >
                    <span className="flex size-3.5 shrink-0 items-center justify-center">
                      <LogIn className="size-3.5" />
                    </span>
                    <span>{t("Open app")}</span>
                  </Menu.Item>
                ) : (
                  <>
                    <Menu.Item
                      closeOnClick
                      render={<a {...publicLink(SIGN_IN_PATH)} />}
                      className={MENU_ITEM}
                    >
                      <span className="flex size-3.5 shrink-0 items-center justify-center">
                        <LogIn className="size-3.5" />
                      </span>
                      <span>{t("Sign in")}</span>
                    </Menu.Item>
                    <Menu.Item
                      closeOnClick
                      render={<a {...publicLink(SIGN_UP_PATH)} />}
                      className={MENU_ITEM}
                    >
                      <span className="flex size-3.5 shrink-0 items-center justify-center">
                        <UserPlus className="size-3.5" />
                      </span>
                      <span>{t("Create a free account")}</span>
                    </Menu.Item>
                  </>
                )}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
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
        <span className="flex items-center gap-2 text-xs text-faint">
          <BrandMark size={12} />© 2026 Droppen · www.droppen.app
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
        <a {...publicLink(DOWNLOAD_PATH)} data-testid="footer-download" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Download Droppen")}
        </a>
        <PageLink path={PRICING_PATH} label={t("Pricing")} testId="footer-pricing" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={PRIVACY_PATH} label={t("Privacy notice")} testId="footer-privacy" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={SECURITY_PATH} label={t("Security")} testId="footer-security" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        <PageLink path={TERMS_PATH} label={t("Terms of Service")} testId="footer-terms" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline" />
        {/* Who to write to: the terms' contact part, its addresses for the account and for security. */}
        <a {...publicLink(`${TERMS_PATH}#contact`)} data-testid="footer-contact" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Contact")}
        </a>
        <a href={SOURCE_URL} data-testid="footer-source" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Client source code")}
        </a>
        {/* Every build writes it (web/build/third-party-notices.mjs): the licences of the packages in this page. */}
        <a href={NOTICES_PATH} data-testid="footer-licences" className="text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
          {t("Third-party notices")}
        </a>
      </div>
    </Section>
  );
}
