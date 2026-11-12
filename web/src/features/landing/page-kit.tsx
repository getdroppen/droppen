import { type AnchorHTMLAttributes, Fragment, type ReactNode, type Ref, useCallback, useEffect, useRef } from "react";
import { ArrowRightIcon, CheckIcon, CopyIcon, SignInIcon, UserPlusIcon, UserCircleIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useEventCallback, useEventListener } from "usehooks-ts";
import { BrandMark } from "@/components/brand-mark";
import { GithubMark } from "./brand-marks";
import { PrefsControls } from "@/components/prefs-controls";
import { RegMark } from "@/components/reg-mark";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { CapsLabel, CJK_TRACKING } from "@/components/ui/caps-label";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useCopy } from "@/lib/clipboard";
import { cn } from "@/lib/cn";
import { appHost, appOrigin, appUrl } from "@/lib/hub/hosts";
import { APP_PATH, DOWNLOAD_PATH, publicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, TERMS_PATH, usePathname } from "./route";

/** A body link: ink and underlined, or quieter when it sits inside a sentence. */
export function TextLink({
  quiet = false,
  nowrap = false,
  className,
  ...props
}: AnchorHTMLAttributes<HTMLAnchorElement> & { quiet?: boolean; nowrap?: boolean }) {
  return (
    <a
      {...props}
      className={cn(
        "underline underline-offset-4",
        quiet ? "text-muted hover:text-fg" : "text-fg hover:no-underline",
        nowrap && "whitespace-nowrap",
        className,
      )}
    />
  );
}

export function DocumentHead({
  label,
  title,
  lead,
  note,
  center = false,
  className,
}: {
  label?: string;
  title: ReactNode;
  lead: string;
  note?: ReactNode;
  center?: boolean;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8",
        center && "mx-auto max-w-[44rem] items-center gap-4 px-6 pt-16 text-center max-md:px-4 max-md:pt-10",
        className,
      )}
    >
      {label && <CapsLabel variant="section">{label}</CapsLabel>}
      <h1 className="max-w-[18em] text-5xl leading-display font-normal tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-3xl">{title}</h1>
      <p className={cn("max-w-[44em] text-base leading-6 text-pretty text-muted cjk-wrap", center && "max-w-[32em]")}>{lead}</p>
      {note && <p className="text-xs text-faint">{note}</p>}
    </div>
  );
}

export function MailLink({ email, testId }: { email: string; testId?: string }) {
  return (
    <TextLink href={`mailto:${email}`} data-testid={testId}>
      {email}
    </TextLink>
  );
}

export const SUBHEAD = "text-base leading-6 font-semibold text-fg";

export function ButtonLink({
  variant = "secondary",
  className,
  ...props
}: AnchorHTMLAttributes<HTMLAnchorElement> & { variant?: "primary" | "secondary" }) {
  return <a data-variant={variant} className={cn(buttonVariants({ variant, size: "lg" }), className)} {...props} />;
}

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
      <ArrowRightIcon className="shrink-0 transition-transform duration-150 group-hover:translate-x-0.5 motion-reduce:transition-none" />
    </ButtonLink>
  );
}

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
export const cjkTracking = CJK_TRACKING;

export function SectionHead({
  label,
  title,
  lead,
  action,
  compact = false,
  className,
}: {
  label?: string;
  title: ReactNode;
  lead?: ReactNode;
  action?: ReactNode;
  compact?: boolean;
  className?: string;
}) {
  const heading = (
    <h2
      className={cn(
        "text-balance text-fg cjk-wrap md:cjk-keep",
        compact
          ? "text-2xl leading-8 font-semibold tracking-tight max-md:text-xl"
          : "text-3xl leading-tight font-normal tracking-tight max-[359px]:text-pretty max-md:text-2xl",
      )}
    >
      {title}
    </h2>
  );
  return (
    <div
      className={cn(
        "flex flex-col gap-3 px-6 max-md:px-4",
        compact ? "pt-16 pb-10 max-md:pt-14 max-md:pb-8" : "pt-10 pb-8 max-md:pt-8 max-md:pb-6",
        className,
      )}
    >
      {label && <CapsLabel variant="section">{label}</CapsLabel>}
      {action ? (
        <div className="flex flex-wrap items-end justify-between gap-6">
          <div className="min-w-0">{heading}</div>
          {action}
        </div>
      ) : (
        heading
      )}
      {lead && <p className="max-w-[38em] text-sm leading-6 text-muted">{lead}</p>}
    </div>
  );
}

/** The plan page's and the landing page's one "Recommended" mark. */
export function RecommendedBadge({ children }: { children: ReactNode }) {
  return (
    <Badge className={cn("h-[18px] bg-fg px-1.5 font-sans text-xs font-medium tracking-label text-bg uppercase shadow-none", cjkTracking)}>
      {children}
    </Badge>
  );
}

function Points({ items, className, render }: { items: string[]; className?: string; render?: (item: string) => ReactNode }) {
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

export function CommandLine({ command, testId, primaryCopy = false }: { command: string; testId?: string; primaryCopy?: boolean }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  const label = t("copy the command");
  return (
    <div className="@container/command">
      <div className="flex min-w-0 items-stretch border border-border-strong bg-raised @max-[30rem]/command:flex-col">
        <code data-testid={testId} className="min-w-0 flex-1 px-3 py-2.5 font-mono text-xs leading-5 whitespace-pre-wrap text-fg select-all [overflow-wrap:anywhere]">
          <span aria-hidden="true" className="mr-2 text-faint select-none">$</span>
          {command}
        </code>
        <Button
          variant={primaryCopy ? "primary" : "secondary"}
          size="lg"
          data-testid={testId ? `${testId}-copy` : undefined}
          aria-label={copied ? t("copied") : label}
          onClick={() => copy(command)}
          className={cn(
            "flex h-auto min-h-9 justify-center py-1.5 text-sm",
            "[&_svg]:size-3.5 min-w-[11rem] shrink-0 rounded-none border-0 @max-[30rem]/command:h-9 @max-[30rem]/command:border-t @max-[30rem]/command:border-border-strong @min-[30rem]/command:border-l @min-[30rem]/command:border-border-strong",
          )}
        >
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
          {copied ? t("copied") : label}
        </Button>
      </div>
    </div>
  );
}

export function usePageMeta(title: string, description?: string) {
  const { i18n: active } = useTranslation();
  // Set the title directly rather than through usehooks-ts's useDocumentTitle: that
  // hook restores the title it found on mount when this page unmounts, and on a
  // client-side move to the next public page that restore lands after the new
  // page set its own, leaving every route titled "Droppen".
  useEffect(() => {
    document.title = title;
  }, [title]);
  useEffect(() => {
    document.documentElement.lang = active.language;
    if (description) document.querySelector('meta[name="description"]')?.setAttribute("content", description);
  }, [description, active.language]);
}

/**
 * Listeners that never block the reader's own gesture, shared so each stays one stable object
 * (`useEventListener` re-attaches whenever its options object changes identity).
 */
export const PASSIVE = { passive: true } as const;
export const CAPTURE = { capture: true } as const;

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
function useHashScroll(fallback?: (hash: string) => string): void {
  const map = useEventCallback((hash: string) => fallback?.(hash) ?? "");
  const live = useRef(false);
  const frame = useRef(0);

  const start = useCallback(() => {
    if (typeof location === "undefined" || typeof requestAnimationFrame === "undefined") return;
    cancelAnimationFrame(frame.current);
    live.current = true;
    const hash = location.hash.slice(1);
    const id = hash || map(hash);
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
    start();
    return () => {
      live.current = false;
      cancelAnimationFrame(frame.current);
    };
  }, [start]);

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

/** Answers stay mounted for search and no-script readers. */
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
    <Card data-testid={testId} className={cn("gap-0", className)}>
      {/* The rows set their own inset (`triggerClassName`), so the card's body carries none. */}
      <CardContent className="px-0">
        <Accordion multiple={false} keepMounted value={value} onValueChange={onValueChange}>
          {items.map((item) => (
            <AccordionItem
              key={item.value ?? item.q}
              value={item.value ?? item.q}
              id={item.id}
              data-testid={item.testId ?? "faq-item"}
              className={cn("group border-b border-border last:border-b-0", itemClassName)}
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
      </CardContent>
    </Card>
  );
}

/**
 * A value still to be supplied by the owner: `[IN CAPITALS]`. The notice shows one marked
 * ({@link marked}); each is listed in `web/owner-placeholders.txt`, and a release refuses a
 * web build that still holds one (`scripts/release-text-check.sh`).
 */
export const PLACEHOLDER = /\[[A-Z][A-Z ]*\]/;

export function marked(text: string): ReactNode {
  const parts = text.split(new RegExp(`(${PLACEHOLDER.source})`));
  if (parts.length === 1) return text;
  return parts.map((part, i) =>
    i % 2 === 1 ? (
      <mark key={i} data-placeholder="" className="bg-warn/25 px-0.5 font-mono text-warn-fg">
        {part}
      </mark>
    ) : (
      <Fragment key={i}>{part}</Fragment>
    ),
  );
}

export function Pair({ left, right }: { left: ReactNode; right: ReactNode }) {
  return (
    <div className="grid border-t border-border md:grid-cols-2">
      {left}
      <div className="max-md:border-t max-md:border-border md:border-l md:border-border">{right}</div>
    </div>
  );
}

export function Block({ title, items }: { title: string; items: string[] }) {
  return (
    <div className="flex min-w-0 flex-col gap-3 px-6 py-6 max-md:px-4">
      <span className="text-sm leading-5 font-semibold text-fg">{title}</span>
      <Points items={items} render={marked} />
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

const iconControl = cn(
  buttonVariants({ variant: "secondary" }),
  "flex size-9 p-0 text-muted transition-colors hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg",
);

function MenuLink({ path, icon: Icon, children }: { path: string; icon: typeof SignInIcon; children: ReactNode }) {
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
        <UserCircleIcon className="size-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="bottom" align="end" sideOffset={4}>
        {signedIn ? (
          <MenuLink path={APP_PATH} icon={SignInIcon}>
            {t("Open app")}
          </MenuLink>
        ) : (
          <>
            <MenuLink path={SIGN_IN_PATH} icon={SignInIcon}>
              {t("Sign in")}
            </MenuLink>
            <MenuLink path={SIGN_UP_PATH} icon={UserPlusIcon}>
              {t("Sign up")}
            </MenuLink>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

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
        <div className="flex items-center gap-2">
          <a
            href={SOURCE_URL}
            data-testid="header-source"
            aria-label={t("Client source code")}
            title={t("Client source code")}
            className={cn(buttonVariants({ variant: "secondary" }), "flex size-9 p-0 text-muted transition-colors hover:text-fg [&_svg]:size-4 max-sm:hidden")}
          >
            <GithubMark className="size-4" />
          </a>
          <PrefsControls />
          <AccountMenu signedIn={signedIn} />
        </div>
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

/** The privacy contact, for removals and requests about data (the owner's word, 2026-10-23). */
export const PRIVACY_EMAIL = "privacy@droppen.app";

const PAGE_WIDE = "mx-auto w-full max-w-[72rem]";

/** A public page's section body: the page-wide column with its bottom room. */
export function SectionBody({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn(PAGE_WIDE, "pb-24 max-md:pb-16", className)}>{children}</div>;
}

function FooterHeading({ children }: { children: ReactNode }) {
  return <h3 className="font-mono text-xs font-semibold tracking-label uppercase text-fg">{children}</h3>;
}

function FooterList({ heading, children }: { heading: string; children: ReactNode }) {
  return (
    <div className="max-sm:col-span-2">
      <FooterHeading>{heading}</FooterHeading>
      <ul className="mt-1.5 flex flex-col gap-2 max-sm:flex-row max-sm:flex-wrap max-sm:gap-x-4 max-sm:gap-y-1.5">{children}</ul>
    </div>
  );
}

export function PublicFooter({ home = false }: { home?: boolean }) {
  const { t } = useTranslation();
  const item = "text-xs leading-5 text-muted transition-colors hover:text-fg";
  return (
    <Section rule={!home} seam={home ? false : "both"}>
      {/* Wide, three columns stand beside the name. Narrower — a phone above all — they are
          two or three lines of links each, and stacking them would make a page of its own:
          they fold into two cells whose links wrap in a row, and the name stands over both. */}
      <div
        className={cn(
          PAGE_WIDE,
          "grid gap-x-9 gap-y-10 px-6 py-11 max-sm:px-6 max-sm:py-8 max-md:px-4 max-md:py-7",
          "sm:grid-cols-2 lg:grid-cols-[2.1fr_repeat(3,1fr)]",
          "max-sm:grid-cols-2 max-sm:gap-x-4 max-sm:gap-y-10",
        )}
      >
        <div className="sm:max-lg:col-span-2 max-sm:col-span-2">
          <a {...publicLink("/")} className="inline-flex items-center gap-2.5" aria-label="Droppen">
            <BrandMark size={18} />
            <span className="font-mono text-base font-medium text-fg">Droppen</span>
          </a>
          <p className="mt-3 max-w-[17em] text-xs leading-5 text-muted">{t("Your AI, from your phone. End-to-end encrypted, open-source client.")}</p>
        </div>
        <FooterList heading={t("Product")}>
          <li><a {...(home ? { href: "#tour" } : publicLink("/#tour"))} className={item}>{t("Interface")}</a></li>
          <li><a {...publicLink(DOWNLOAD_PATH)} data-testid="footer-download" className={item}>{t("Download Droppen")}</a></li>
          <li><a {...publicLink(PRICING_PATH)} data-testid="footer-pricing" className={item}>{t("Pricing")}</a></li>
          <li><a {...publicLink(SECURITY_PATH)} data-testid="footer-security" className={item}><span>{t("Security")}</span></a></li>
        </FooterList>
        <FooterList heading={t("Resources")}>
          <li><a href={SOURCE_URL} data-testid="footer-source" className={item}>{t("Client source code")}</a></li>
          <li><a {...(home ? { href: "#faq" } : publicLink("/#faq"))} className={item}>{t("FAQ")}</a></li>
          <li><a {...publicLink(SECURITY_PATH)} className={item}>{t("Encryption design")}</a></li>
        </FooterList>
        <FooterList heading={t("Legal")}>
          <li><a {...publicLink(PRIVACY_PATH)} data-testid="footer-privacy" className={item}><span>{t("Privacy notice")}</span></a></li>
          <li><a {...publicLink(TERMS_PATH)} data-testid="footer-terms" className={item}><span>{t("Terms of Service")}</span></a></li>
          <li><a href={NOTICES_PATH} data-testid="footer-licences" className={item}>{t("Third-party notices")}</a></li>
          <li><a {...publicLink(`${TERMS_PATH}#contact`)} data-testid="footer-contact" className={item}>{t("Contact")}</a></li>
        </FooterList>
      </div>
      <div className={cn(PAGE_WIDE, "flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t border-border px-6 py-4 text-xs text-faint max-md:px-4")}>
        <span className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
          <span className="flex items-center gap-2"><BrandMark size={12} />© 2026 Droppen</span>
          <span>AGPL-3.0-only</span>
          {appOrigin() && (
            <a href={appUrl("/")} data-testid="footer-app" className="underline-offset-4 hover:text-fg hover:underline">
              {appHost()}
            </a>
          )}
        </span>
        <a
          href="#top"
          onClick={(event) => {
            const scroller = event.currentTarget.closest<HTMLElement>("[data-scroller]");
            if (!scroller) return;
            event.preventDefault();
            scroller.scrollTo({ top: 0 });
            history.replaceState(history.state, "", `${location.pathname}${location.search}`);
          }}
          className="ml-auto underline-offset-4 hover:text-fg hover:underline"
        >
          {t("Back to top")}
        </a>
      </div>
    </Section>
  );
}

export function PublicLayout({
  testId,
  signedIn = false,
  scrollerRef,
  mainTestId,
  mainData,
  home = false,
  hashFallback,
  children,
}: {
  testId: string;
  signedIn?: boolean;
  scrollerRef?: Ref<HTMLDivElement>;
  mainTestId?: string;
  mainData?: Record<string, string | undefined>;
  home?: boolean;
  hashFallback?: (hash: string) => string;
  children: ReactNode;
}) {
  useHashScroll(hashFallback);
  return (
    <div ref={scrollerRef} data-testid={testId} data-scroller="" className="h-full w-full overflow-x-hidden overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main id="top" data-testid={mainTestId} {...mainData} className="mx-auto max-w-[1360px] md:border-x md:border-border">
          {children}
          <PublicFooter home={home} />
        </main>
      </div>
    </div>
  );
}
