import { type AnchorHTMLAttributes, type ReactNode, type Ref, useCallback, useEffect, useRef } from "react";
import { ArrowRight, Check, Copy, LogIn, UserPlus, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useDocumentTitle, useEventListener } from "usehooks-ts";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { RegMark } from "@/components/reg-mark";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
import { Button, buttonVariants } from "@/components/ui/button";
import { CapsLabel } from "@/components/ui/caps-label";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useCopy } from "@/lib/clipboard";
import { cn } from "@/lib/cn";
import { appHost, appOrigin, appUrl } from "@/lib/hub/hosts";
import { DOWNLOAD_PATH } from "./get-droppen";
import { APP_PATH, publicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, TERMS_PATH, usePathname } from "./route";

/* The public pages' frame: a sticky header over a 1360px column, hairline sections, and the footer. */

const landingAction = "flex h-auto min-h-9 justify-center py-1.5 text-sm";

const landingPrimary = cn(buttonVariants({ variant: "primary", size: "lg" }), landingAction);
const landingSecondary = cn(buttonVariants({ variant: "secondary", size: "lg" }), landingAction);

export function ButtonLink({
  variant = "secondary",
  className,
  ...props
}: AnchorHTMLAttributes<HTMLAnchorElement> & { variant?: "primary" | "secondary" }) {
  return <a data-variant={variant} className={cn(buttonVariants({ variant, size: "lg" }), className)} {...props} />;
}

/** A public page's call to action: its label, then an arrow that steps right on hover. */
export function ArrowLink({
  variant = "secondary",
  children,
  className,
  ...props
}: AnchorHTMLAttributes<HTMLAnchorElement> & { variant?: "primary" | "secondary" }) {
  return (
    <ButtonLink
      {...props}
      variant={variant}
      className={cn(
        "group flex h-10 justify-between gap-3 px-4 text-sm font-medium transition-colors hover:border-fg [&_svg]:size-4",
        className,
      )}
    >
      <span>{children}</span>
      <ArrowRight className="shrink-0 transition-transform duration-150 group-hover:translate-x-0.5 motion-reduce:transition-none" />
    </ButtonLink>
  );
}

/** Registration marks where a section line meets the frame. */
function Seam({ side = "both" }: { side?: "both" | "left" | "right" }) {
  return (
    <>
      {side !== "right" && <RegMark className="-top-[5px] -left-[5px]" />}
      {side !== "left" && <RegMark className="-top-[5px] -right-[5px]" />}
    </>
  );
}

export function Section({
  id,
  children,
  className,
  seam = "both",
  rule = true,
}: {
  id?: string;
  children: ReactNode;
  className?: string;
  seam?: "both" | "left" | "right" | false;
  rule?: boolean;
}) {
  return (
    <section id={id} className={cn("relative scroll-mt-14", rule && "border-t border-border", className)}>
      {seam !== false && <Seam side={seam} />}
      {children}
    </section>
  );
}

/** Latin capitals are tracked; Chinese and Japanese are not. */
export const cjkTracking = "[&:lang(ja)]:tracking-normal [&:lang(zh)]:tracking-normal";

export function SectionHead({ label, title, lead, className }: { label?: string; title: ReactNode; lead?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-3 px-6 pt-10 pb-8 max-md:px-4 max-md:pt-8 max-md:pb-6", className)}>
      {label && <CapsLabel variant="section">{label}</CapsLabel>}
      <h2 className="text-3xl leading-tight font-normal tracking-tight text-balance text-fg max-[359px]:text-pretty cjk-wrap md:cjk-keep max-md:text-2xl">{title}</h2>
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

/** A command on the raised paper, with its copy button. */
export function CommandLine({ command, label, testId, primaryCopy = false }: { command: string; label: string; testId?: string; primaryCopy?: boolean }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  return (
    <div className="@container/command">
      <div className="flex min-w-0 items-stretch border border-border-strong bg-raised @max-[30rem]/command:flex-col">
        <code data-testid={testId} className="min-w-0 flex-1 px-3 py-2.5 font-mono text-xs leading-5 whitespace-pre-wrap text-fg select-all [overflow-wrap:anywhere]">
          <span aria-hidden="true" className="mr-2 text-faint select-none">$</span>
          {command}
        </code>
        <Button
          variant={primaryCopy ? "primary" : "secondary"}
          data-testid={testId ? `${testId}-copy` : undefined}
          aria-label={copied ? t("copied") : label}
          onClick={() => copy(command)}
          className={cn(
            primaryCopy ? landingPrimary : landingSecondary,
            "[&_svg]:size-3.5 h-auto min-w-[11rem] shrink-0 justify-center rounded-none border-0 @max-[30rem]/command:h-9 @max-[30rem]/command:border-t @max-[30rem]/command:border-border-strong @min-[30rem]/command:border-l @min-[30rem]/command:border-border-strong",
          )}
        >
          {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
          {copied ? t("copied") : label}
        </Button>
      </div>
    </div>
  );
}

/** The page's title, description and language. */
export function usePageMeta(title: string, description?: string) {
  const { i18n: active } = useTranslation();
  useDocumentTitle(title, { preserveTitleOnUnmount: false });
  useEffect(() => {
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const previous = meta?.getAttribute("content") ?? null;
    if (description) meta?.setAttribute("content", description);
    return () => {
      if (meta && previous !== null) meta.setAttribute("content", previous);
    };
  }, [description, active.language]);
}

/** One options object, so `useEventListener` keeps its listener attached (`{ passive: true }`). */
const PASSIVE = { passive: true } as const;

/**
 * Arrive at the fragment in the address (`/pricing#waitlist` from the landing
 * page, `/terms#contact` from any footer): the page was not there to scroll to
 * yet, so scroll once the frame is up, and again once the webfonts settle —
 * they swap in after that first scroll and move its target with them. The
 * reader's own moving of the page (a wheel, a key, a touch, a press) ends the
 * correction, and so does a newer fragment; a link can also change the
 * fragment in place, which scrolls to the one it names.
 *
 * `fallback` names the section a fragment-less deep link should land on (the
 * download page's old `?os=` links); the pages that only ever need the
 * address's own fragment leave it out.
 */
export function useHashScroll(fallback?: (hash: string) => string): void {
  const map = useRef(fallback);
  const live = useRef(false);
  const frame = useRef(0);

  /** Scroll to the fragment the address (or `map`) names, if it is still the one there. */
  const start = useCallback(() => {
    if (typeof location === "undefined" || typeof requestAnimationFrame === "undefined") return;
    cancelAnimationFrame(frame.current);
    live.current = true;
    const hash = location.hash.slice(1);
    const id = hash || map.current?.(hash) || "";
    if (!id) return;
    const to = () => {
      if (live.current && location.hash.slice(1) === hash) document.getElementById(id)?.scrollIntoView();
    };
    frame.current = requestAnimationFrame(to);
    void document.fonts?.ready.then(() => {
      if (live.current) to();
    });
  }, []);

  useEffect(() => {
    map.current = fallback;
  }, [fallback]);

  useEffect(() => {
    start();
    return () => {
      live.current = false;
      cancelAnimationFrame(frame.current);
    };
  }, [start]);

  /** The reader moving the page wins over a correction still in flight. */
  const stop = useCallback(() => {
    live.current = false;
    cancelAnimationFrame(frame.current);
  }, []);

  useEventListener("hashchange", start);
  useEventListener("pointerdown", stop, undefined, PASSIVE);
  useEventListener("wheel", stop, undefined, PASSIVE);
  useEventListener("touchstart", stop, undefined, PASSIVE);
  useEventListener("keydown", stop, undefined, PASSIVE);
}

/** One accordion of questions, one open at a time; answers stay mounted for search and no-script readers. */
export function FaqAccordion({
  items,
  className,
  itemClassName,
  triggerClassName,
  panelClassName,
  answerClassName,
  value,
  onValueChange,
  testId,
}: {
  items: { q: string; a: ReactNode; value?: string; id?: string; testId?: string }[];
  className?: string;
  itemClassName?: string;
  triggerClassName?: string;
  panelClassName?: string;
  answerClassName?: string;
  value?: (string | null)[];
  onValueChange?: (value: (string | null)[]) => void;
  testId?: string;
}) {
  return (
    <div data-testid={testId} className={className}>
      <Accordion multiple={false} keepMounted value={value} onValueChange={onValueChange}>
        {items.map((item) => (
          <AccordionItem
            key={item.value ?? item.q}
            value={item.value ?? item.q}
            id={item.id}
            data-testid={item.testId ?? "faq-item"}
            className={cn("group", itemClassName ?? "last:border-b")}
          >
            <AccordionTrigger className={triggerClassName}>
              <span className="text-base leading-6 font-medium text-fg">{item.q}</span>
            </AccordionTrigger>
            <AccordionContent className={cn("p-0", panelClassName)}>
              {typeof item.a === "string" ? <p className={cn("max-w-prose leading-6 text-muted", answerClassName ?? "text-base")}>{item.a}</p> : item.a}
            </AccordionContent>
          </AccordionItem>
        ))}
      </Accordion>
    </div>
  );
}

function PageLink({ path, label, className, testId }: { path: string; label: string; className: string; testId?: string }) {
  const here = usePathname().replace(/\/+$/, "") === path;
  return (
    <a {...publicLink(path)} aria-current={here ? "page" : undefined} data-testid={testId} className={cn("inline-grid", className, here && "text-fg")}>
      <span aria-hidden="true" className="invisible col-start-1 row-start-1 font-semibold">
        {label}
      </span>
      <span className={cn("col-start-1 row-start-1", here && "font-semibold")}>{label}</span>
    </a>
  );
}

/** GitHub's mark (Octicons' mark-github, MIT). */
function GitHubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" className={className} fill="currentColor">
      <path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </svg>
  );
}

const iconControl = cn(
  buttonVariants({ variant: "secondary" }),
  "flex size-9 p-0 text-muted transition-colors hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg",
);

function MenuLink({ path, icon: Icon, children }: { path: string; icon: typeof LogIn; children: ReactNode }) {
  return (
    <DropdownMenuItem render={<a {...publicLink(path)} />} className="gap-2 text-sm">
      <span className="flex size-3.5 shrink-0 items-center justify-center">
        <Icon className="size-3.5" />
      </span>
      <span>{children}</span>
    </DropdownMenuItem>
  );
}

function AccountMenu({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger aria-label={signedIn ? t("Open app") : t("Sign in")} title={signedIn ? t("Open app") : t("Sign in")} className={iconControl}>
        <UserRound className="size-3.5" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="bottom" align="end" sideOffset={4}>
        {signedIn ? (
          <MenuLink path={APP_PATH} icon={LogIn}>
            {t("Open app")}
          </MenuLink>
        ) : (
          <>
            <MenuLink path={SIGN_IN_PATH} icon={LogIn}>
              {t("Sign in")}
            </MenuLink>
            <MenuLink path={SIGN_UP_PATH} icon={UserPlus}>
              {t("Create a free account")}
            </MenuLink>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The public pages' header. */
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
          className={cn(buttonVariants({ variant: "secondary" }), "flex size-9 p-0 text-muted transition-colors hover:text-fg [&_svg]:size-4 max-sm:hidden")}
        >
          <GitHubMark className="size-4" />
        </a>
        <PrefsControls />
        <AccountMenu signedIn={signedIn} />
        <RegMark className="-bottom-[5px] -left-[5px]" />
        <RegMark className="-right-[5px] -bottom-[5px]" />
      </div>
    </header>
  );
}

/** The licences of the npm packages in this build, beside `index.html` in every build. */
export const NOTICES_PATH = "/THIRD_PARTY_NOTICES.txt";

export const SOURCE_URL = "https://github.com/getdroppen/droppen";

export const SECURITY_EMAIL = "security@droppen.app";

/** The last line of every public page. */
const FOOTER_WIDE = "mx-auto w-full max-w-[72rem]";

function FooterHeading({ children }: { children: ReactNode }) {
  return <h4 className="font-mono text-xs tracking-label uppercase text-muted">{children}</h4>;
}

/** The footer every public page ends with; `home` keeps its in-page links on the landing page. */
export function PublicFooter({ home = false }: { home?: boolean }) {
  const { t } = useTranslation();
  const item = "text-xs leading-5 text-muted transition-colors hover:text-fg";
  return (
    <Section rule={!home} seam={home ? false : "both"}>
      <div className={cn(FOOTER_WIDE, "grid gap-9 px-6 py-11 max-md:px-4 max-md:py-8 sm:grid-cols-2 lg:grid-cols-[2.1fr_repeat(3,1fr)]")}>
        <div>
          <a {...publicLink("/")} className="inline-flex items-center gap-2.5" aria-label="Droppen">
            <BrandMark size={18} />
            <span className="font-mono text-base font-medium text-fg">Droppen</span>
          </a>
          <p className="mt-3 max-w-[17em] text-xs leading-5 text-muted">{t("Your AI, from your phone. End-to-end encrypted, open-source client.")}</p>
        </div>
        <div>
          <FooterHeading>{t("Product")}</FooterHeading>
          <ul className="mt-3 flex flex-col gap-2">
            <li><a {...(home ? { href: "#tour" } : publicLink("/#tour"))} className={item}>{t("Interface")}</a></li>
            <li><a {...publicLink(DOWNLOAD_PATH)} data-testid="footer-download" className={item}>{t("Download Droppen")}</a></li>
            <li><a {...publicLink(PRICING_PATH)} data-testid="footer-pricing" className={item}>{t("Pricing")}</a></li>
            <li><a {...publicLink(SECURITY_PATH)} data-testid="footer-security" className={item}><span>{t("Security")}</span></a></li>
          </ul>
        </div>
        <div>
          <FooterHeading>{t("Resources")}</FooterHeading>
          <ul className="mt-3 flex flex-col gap-2">
            <li><a href={SOURCE_URL} data-testid="footer-source" className={item}>{t("Client source code")}</a></li>
            <li><a {...(home ? { href: "#faq" } : publicLink("/#faq"))} className={item}>{t("FAQ")}</a></li>
            <li><a {...publicLink(SECURITY_PATH)} className={item}>{t("Encryption design")}</a></li>
          </ul>
        </div>
        <div>
          <FooterHeading>{t("Legal")}</FooterHeading>
          <ul className="mt-3 flex flex-col gap-2">
            <li><a {...publicLink(PRIVACY_PATH)} data-testid="footer-privacy" className={item}><span>{t("Privacy notice")}</span></a></li>
            <li><a {...publicLink(TERMS_PATH)} data-testid="footer-terms" className={item}><span>{t("Terms of Service")}</span></a></li>
            <li><a href={NOTICES_PATH} data-testid="footer-licences" className={item}>{t("Third-party notices")}</a></li>
            <li><a {...publicLink(`${TERMS_PATH}#contact`)} data-testid="footer-contact" className={item}>{t("Contact")}</a></li>
          </ul>
        </div>
      </div>
      <div className={cn(FOOTER_WIDE, "flex flex-wrap items-center justify-between gap-3 border-t border-border px-6 py-4 text-xs text-faint max-md:px-4")}>
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono">
          <span className="flex items-center gap-2"><BrandMark size={12} />© 2026 Droppen</span>
          <span>AGPL-3.0-only</span>
          <span>www.droppen.app</span>
          {appOrigin() && (
            <a href={appUrl("/")} data-testid="footer-app" className="underline-offset-4 hover:text-fg hover:underline">
              {appHost()}
            </a>
          )}
        </span>
        <a href="#top" className="underline-offset-4 hover:text-fg hover:underline">{t("Back to top")}</a>
      </div>
    </Section>
  );
}

/** The frame every public page shares: the scrolling page, the header, the 1360px column and the footer. */
export function PublicLayout({
  testId,
  signedIn = false,
  scrollerRef,
  mainTestId,
  mainClassName,
  mainData,
  home = false,
  hashFallback,
  children,
}: {
  testId: string;
  signedIn?: boolean;
  scrollerRef?: Ref<HTMLDivElement>;
  mainTestId?: string;
  mainClassName?: string;
  mainData?: Record<string, string | undefined>;
  home?: boolean;
  /** Names the section a fragment-less deep link lands on (the download page's old `?os=` links). */
  hashFallback?: (hash: string) => string;
  children: ReactNode;
}) {
  useHashScroll(hashFallback);
  return (
    <div ref={scrollerRef} data-testid={testId} className="h-full w-full overflow-x-hidden overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main id="top" data-testid={mainTestId} {...mainData} className={cn("mx-auto max-w-[1360px] md:border-x md:border-border", mainClassName)}>
          {children}
          <PublicFooter home={home} />
        </main>
      </div>
    </div>
  );
}
