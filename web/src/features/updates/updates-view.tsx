import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSettings } from "@/app/global-feed";
import { updateVersion, useDesktopUpdate, type UpdateStatus } from "@/app/desktop-update";
import { Content } from "@/components/panel";
import { Sheet } from "@/components/sheet";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";

/*
 * The update page: what Droppen is about to install (or the newest version it
 * has), what changed in it, and the one action that restarts into it. The
 * rail's entry opens it and never restarts by itself (`app/desktop-update.tsx`).
 *
 * The notes are the release body's two "what's new" sections, English and
 * Chinese, read by the app for its update check
 * (`desktop/core/src/update.rs`, `whats_new`): the page picks the reader's
 * language and offers the other. A release without them (a snapshot) links to
 * GitHub instead.
 *
 * Nothing here is a dialog, and nothing interrupts: the app updates quietly
 * at the next quit, and the page is where the quiet entry explains itself.
 */

const releasePage = (version: string) => `https://github.com/getdroppen/droppen/releases/tag/v${version}`;

/** The whole point of the page, in the app's terms. */
export function problemLine(problem: UpdateStatus["problem"]): string {
  switch (problem) {
    case "offline":
      return "Could not check for updates: no network.";
    case "service":
      return "The update service answered with something Droppen could not read.";
    case "signature":
      return "The download did not verify, so it was thrown away.";
    case "location":
      return "Droppen cannot update where it runs from.";
    case "install":
      return "The update did not install; Droppen is as it was.";
    default:
      return "The update check failed.";
  }
}

/** Where it runs from, when that is why it cannot update. */
function placeLine(place: UpdateStatus["place"]): string | null {
  if (place === "translocated" || place === "readOnly") return "Move Droppen to Applications to update.";
  if (place === "symlinked" || place === "notABundle") return "This copy of Droppen cannot update itself.";
  return null;
}

/** The notes for the version in hand, and which language to show. */
function notesFor(status: UpdateStatus, version: string, chinese: boolean): { text: string; both: boolean; shown: "en" | "zh" } | null {
  if (!status.notes || status.notes.version !== version) return null;
  const { english, chinese: zh } = status.notes;
  if (!english && !zh) return null;
  const shown = chinese && zh ? "zh" : english ? "en" : "zh";
  return { text: shown === "zh" ? zh : english, both: !!english && !!zh, shown };
}

/** Links in a release body are read, not fetched: only the projects' own pages. */
function releaseUrl(url: string): string {
  const allowed = /^https:\/\/(?:github\.com|(?:www\.)?droppen\.app)\//i.test(url);
  return allowed ? defaultUrlTransform(url) : "";
}

const notesComponents: React.ComponentProps<typeof ReactMarkdown>["components"] = {
  h2: () => null,
  h3: ({ children }) => <h3 className="mt-4 mb-1 text-[13px] font-semibold first:mt-0">{children}</h3>,
  h4: ({ children }) => <h4 className="mt-3 mb-1 text-xs font-semibold">{children}</h4>,
  p: ({ children }) => <p className="my-1.5 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-1 list-[square] space-y-1 pl-4 marker:text-faint">{children}</ul>,
  ol: ({ children }) => <ol className="my-1 list-decimal space-y-1 pl-5 marker:font-mono marker:text-faint">{children}</ol>,
  li: ({ children }) => <li className="[&>p]:my-0">{children}</li>,
  a: ({ href, children }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="text-fg underline decoration-border-strong underline-offset-[3px] hover:decoration-fg"
    >
      {children}
    </a>
  ),
  img: () => null,
  blockquote: ({ children }) => <blockquote className="my-1.5 border-l border-border-strong pl-3 text-muted">{children}</blockquote>,
  hr: () => <hr className="my-3 border-border" />,
  code: ({ children }) => (
    <code className="bg-raised px-1 py-px font-mono text-[0.9em] text-fg shadow-[inset_0_0_0_1px_var(--color-border)]">{children}</code>
  ),
  pre: ({ children }) => <pre className="my-2 overflow-x-auto border border-border bg-raised px-2.5 py-2 font-mono text-2xs">{children}</pre>,
};

const row = "border-b border-border";
const cellLabel = "py-1 pr-4 align-top font-mono text-2xs whitespace-nowrap text-faint";
const cellValue = "py-1 font-mono text-xs break-all";

export interface UpdatesPageProps {
  status: UpdateStatus | null;
  dismissed: string | null;
  onDismiss: (version: string) => void;
  onRestart: () => void;
  onCheck: () => void;
  serverVersion?: string | null;
  /** The reader's language, for which of the two notes to show first. */
  language?: string;
}

/** The page itself, given the app's status; `UpdatesView` wires the bridge. */
export function UpdatesPage({ status, dismissed, onDismiss, onRestart, onCheck, serverVersion = null, language = "en" }: UpdatesPageProps) {
  const { t } = useTranslation();
  // The reader's language decides until they choose for this visit — the
  // page can be open while the app's language preference is still loading.
  const [chosen, setChosen] = useState<"en" | "zh" | null>(null);
  const wantsChinese = chosen ? chosen === "zh" : language.toLowerCase().startsWith("zh");

  if (!status) {
    return (
      <Content>
        <header className="flex h-8 shrink-0 items-center border-b border-border bg-panel px-3">
          <span className="font-mono text-xs">{t("Updates")}</span>
        </header>
        <Sheet>
          <section className="p-3">
            <p className="max-w-xl text-xs text-muted">{t("Updates are handled by the Droppen app.")}</p>
          </section>
        </Sheet>
      </Content>
    );
  }

  const version = updateVersion(status);
  const target = ["downloading", "ready", "installing", "restarting"].includes(status.state) ? status.release?.version ?? null : null;
  const ready = status.state === "ready" && status.release;
  const downloading = status.state === "downloading" && status.release;
  const checking = status.state === "checking";
  const canUpdate = status.place === "updatable";
  const cannot = placeLine(status.place);
  const notes = notesFor(status, version, wantsChinese);
  const release = releasePage(version);
  const checked = status.lastChecked
    ? formatRelative(status.lastChecked) === "now"
      ? t("checked just now")
      : t("checked {{when}} ago", { when: formatRelative(status.lastChecked) })
    : t("not checked yet");

  return (
    <Content>
      <header className="flex h-8 shrink-0 items-center justify-between gap-2 border-b border-border bg-panel px-3">
        <span className="font-mono text-xs">{t("Updates")}</span>
        <span className="font-mono text-2xs text-faint">{status.current}</span>
      </header>
      <Sheet>
        <section className="border-b border-border p-3" data-testid="updates-state" data-state={status.state}>
          <div className="flex flex-wrap items-baseline gap-2 font-mono text-xl text-fg tabular-nums">
            <span>{status.current}</span>
            {target && target !== status.current && (
              <>
                <span className="text-faint">→</span>
                <span data-testid="updates-target">{target}</span>
              </>
            )}
          </div>

          {ready ? (
            <>
              <p className="mt-1 text-[13px] font-semibold text-fg" data-testid="updates-ready">
                {t("Droppen {{version}} is ready", { version: ready.version })}
              </p>
              <p className="mt-0.5 text-xs text-muted">{t("Downloaded and verified.")}</p>
              <div className="mt-3 flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  data-variant="primary"
                  data-testid="updates-restart"
                  onClick={onRestart}
                  className="flex h-7 items-center justify-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85"
                >
                  {t("Restart to update")}
                </button>
                {dismissed !== ready.version && (
                  <button
                    type="button"
                    data-testid="updates-later"
                    onClick={() => onDismiss(ready.version)}
                    className="flex h-7 items-center justify-center rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
                  >
                    {t("Later")}
                  </button>
                )}
              </div>
              <p className="mt-2 max-w-xl text-2xs text-faint">
                {t("It installs the next time you quit Droppen.")} {t("Sessions keep running while Droppen restarts.")}
              </p>
            </>
          ) : downloading ? (
            <>
              <p className="mt-1 text-[13px] font-semibold text-fg">{t("Downloading Droppen {{version}}", { version: downloading.version })}</p>
              <p className="mt-0.5 text-xs text-muted">
                {status.percent != null ? `${status.percent}% · ` : ""}
                {t("It keeps going in the background.")}
              </p>
              {status.percent != null && (
                <div
                  role="progressbar"
                  aria-label={t("Downloading Droppen {{version}}", { version: downloading.version })}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={status.percent}
                  className="mt-3 h-1 max-w-xl bg-border"
                >
                  <div className="h-full bg-fg" style={{ width: `${status.percent}%` }} />
                </div>
              )}
            </>
          ) : status.state === "installing" || status.state === "restarting" ? (
            <p className="mt-1 flex items-center gap-2 text-[13px] font-semibold text-fg">
              <Spinner />
              {status.state === "installing" ? t("Installing…") : t("Restarting…")}
            </p>
          ) : (
            <>
              <p className="mt-1 text-[13px] font-semibold text-fg" data-testid="updates-current">
                {status.lastChecked ? t("You're on the latest version, {{version}}.", { version: status.current }) : `Droppen ${status.current}`}
              </p>
              {checking && (
                <p className="mt-0.5 flex items-center gap-2 text-xs text-muted">
                  <Spinner />
                  {t("Checking for updates…")}
                </p>
              )}
            </>
          )}

          {cannot && (
            <p data-testid="updates-place" className="mt-2 max-w-xl text-xs text-muted">
              {t(cannot)}
            </p>
          )}
          {status.problem && (
            <p data-signal="failed" data-testid="updates-problem" className="mt-2 max-w-xl font-mono text-2xs break-words text-danger">
              {t(problemLine(status.problem))}
            </p>
          )}
          {canUpdate && !ready && !downloading && !checking && status.state !== "installing" && status.state !== "restarting" && (
            <div className="mt-3 flex items-center gap-2">
              <button
                type="button"
                data-testid="updates-check"
                disabled={checking}
                onClick={onCheck}
                className="flex h-7 items-center justify-center rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
              >
                {status.problem ? t("Try again") : t("Check now")}
              </button>
            </div>
          )}
        </section>

        <section className="border-b border-border p-3" data-testid="updates-notes">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <h2 data-label="" className="label-caps flex h-6 items-center whitespace-nowrap text-2xs text-fg">
              {target && target !== status.current ? t("What's new in {{version}}", { version }) : t("What changed in {{version}}", { version })}
            </h2>
            <div className="flex items-center gap-3">
              {notes?.both && (
                <Tabs value={notes.shown} onValueChange={(value) => setChosen(value === "zh" ? "zh" : "en")}>
                  <TabsList>
                    <TabsTab value="en">English</TabsTab>
                    <TabsTab value="zh">中文</TabsTab>
                  </TabsList>
                </Tabs>
              )}
              <a
                href={release}
                target="_blank"
                rel="noreferrer"
                className="font-mono text-2xs text-faint underline-offset-2 hover:text-fg hover:underline"
              >
                {t("Read the release notes on GitHub")}
              </a>
            </div>
          </div>
          {notes ? (
            <div className="max-w-[48em]" data-testid="updates-notes-body">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={notesComponents} urlTransform={releaseUrl}>
                {notes.text}
              </ReactMarkdown>
            </div>
          ) : (
            <p className="max-w-xl text-xs text-muted">{t("No notes for this release are available here.")}</p>
          )}
        </section>

        <section className="p-3" data-testid="updates-facts">
          <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
            {t("Release")}
          </h2>
          <table className="w-full max-w-xl">
            <tbody>
              <tr className={row}>
                <td className={cellLabel}>{t("App")}</td>
                <td className={cn(cellValue, "text-fg")}>{status.current}</td>
              </tr>
              {serverVersion && (
                <tr className={row}>
                  <td className={cellLabel}>{t("Server")}</td>
                  <td className={cellValue}>{serverVersion}</td>
                </tr>
              )}
              <tr className={row}>
                <td className={cellLabel}>{t("Checked")}</td>
                <td className={cellValue}>{checked}</td>
              </tr>
              <tr>
                <td className={cellLabel}>{t("Release page")}</td>
                <td className={cellValue}>
                  <a href={release} target="_blank" rel="noreferrer" className="underline decoration-border-strong underline-offset-[3px] hover:decoration-fg">
                    v{version}
                  </a>
                </td>
              </tr>
            </tbody>
          </table>
        </section>
      </Sheet>
    </Content>
  );
}

/** The route: the bridge's status, kept live by `droppen:update`. */
export function UpdatesView() {
  const { i18n } = useTranslation();
  const { status, dismissed, dismiss, restart, check } = useDesktopUpdate();
  const settings = useFeedSettings();
  const asked = useRef(false);
  // A page opened before the app's first check: ask once, so it has something
  // to show rather than waiting up to six hours.
  useEffect(() => {
    if (asked.current || !status || status.place !== "updatable" || status.lastChecked || status.notes) return;
    asked.current = true;
    check();
  }, [status, check]);
  return (
    <UpdatesPage
      status={status}
      dismissed={dismissed}
      onDismiss={dismiss}
      onRestart={restart}
      onCheck={check}
      serverVersion={settings?.serverVersion ?? null}
      language={i18n.language}
    />
  );
}
