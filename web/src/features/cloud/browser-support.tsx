import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { compatModeActive, forgetCompatConsent } from "@/lib/e2e/compat";
import { MINIMUM_BROWSERS } from "@/lib/e2e/support";
import { cn } from "@/lib/cn";
import { type HeldSession, signOut } from "./sign-out";
import { Centered, danger, Note, Panel, quiet } from "./ui";

/** An app whose built-in browser this page seems to run in. */
export type InAppBrowser = "wechat" | "qq" | "alipay" | "douyin" | "weibo" | "webview";

/**
 * The app whose built-in browser `userAgent` names, if any: they often run
 * an old engine of their own, and the system browser is the way out.
 * `webview`: an Android app's web view (`; wv)`), whichever app it is.
 */
export function inAppBrowser(userAgent: string = globalThis.navigator?.userAgent ?? ""): InAppBrowser | null {
  if (/MicroMessenger|wxwork/i.test(userAgent)) return "wechat";
  if (/AlipayClient/i.test(userAgent)) return "alipay";
  if (/aweme|BytedanceWebview|musical_ly/i.test(userAgent)) return "douyin";
  if (/weibo/i.test(userAgent)) return "weibo";
  // The QQ app (`QQ/9.0.…`); not QQ Browser (`MQQBrowser/`), a browser of its own.
  if (/(?:^|[^A-Za-z])QQ\/\d/.test(userAgent)) return "qq";
  if (/\bAndroid\b/.test(userAgent) && /;\s*wv\)/.test(userAgent)) return "webview";
  return null;
}

const APP_NAMES: Record<Exclude<InAppBrowser, "webview">, string> = {
  wechat: "WeChat",
  qq: "QQ",
  alipay: "Alipay",
  douyin: "Douyin",
  weibo: "Weibo",
};

/**
 * This browser cannot run the end-to-end encryption (`missing`, from
 * `checkE2eSupport`): said before any key is read or made, with what to do —
 * a current browser, or, from an app's built-in browser, the system's own.
 * Nothing here retries or makes a new device: neither would ever help.
 *
 * `compat`: only X25519 / Ed25519 are missing, so compatibility mode could
 * stand in (`lib/e2e/compat.ts`). It is offered with its risk said plainly,
 * and starts only once the person ticks that they understand it and trusts
 * this browser (`onTrust`). `held`: the session a sign-out ends (signed in).
 * Exported so the screen can be previewed on its own.
 */
export function UnsupportedBrowserScreen({
  missing,
  compat = false,
  held,
  onTrust,
  userAgent,
}: {
  missing: string[];
  compat?: boolean;
  held?: HeldSession;
  onTrust?: () => void;
  userAgent?: string;
}) {
  const { t } = useTranslation();
  const [understood, setUnderstood] = useState(false);
  const app = inAppBrowser(userAgent);
  const offer = compat && onTrust !== undefined;
  return (
    <Centered>
      <Panel title={offer ? t("this browser does not support Droppen's strong key protection") : t("this browser cannot run Droppen's end-to-end encryption")}>
        <Note tone="warn">
          <p data-testid="unsupported-why" className="text-xs leading-5">
            {offer
              ? t("Droppen normally keeps this device's private keys as non-extractable browser keys. This browser cannot do that for the key types Droppen uses. Nothing was stored or changed.")
              : t("Droppen encrypts everything end to end on this device, and this browser lacks what that needs. Nothing was stored or changed.")}
          </p>
          {missing.length > 0 && (
            <div data-signal="unsupported" data-testid="unsupported-missing" className="font-mono text-2xs break-words text-faint">
              {t("missing: {{what}}", { what: missing.join(", ") })}
            </div>
          )}
        </Note>
        <p data-testid="unsupported-browsers" className="text-xs leading-5 text-muted">
          {t("Update this browser, or open this page in Chrome {{chrome}}, Edge {{edge}}, Firefox {{firefox}} or Safari {{safari}}, or newer.", MINIMUM_BROWSERS)}
        </p>
        {app && (
          <Note>
            <p data-testid="in-app-browser" className="text-xs leading-5 text-muted">
              {app === "webview"
                ? t("This looks like a browser built into an app. Open this page in the system browser instead.")
                : t("This looks like the browser built into {{app}}. Open this page in the system browser: in the app's menu (often ··· or ⋮), choose “Open in browser”.", { app: t(APP_NAMES[app]) })}
            </p>
          </Note>
        )}
        {offer && (
          <div data-testid="compat-offer" className="flex flex-col gap-2 border border-border p-3">
            <p className="text-xs leading-5 text-fg">
              {t(
                "Or use this browser in compatibility mode: the same encryption, done in JavaScript. What changes: the JavaScript curve arithmetic is not hardened against side channels as the browser's own is; one more library is loaded into the page; and this device's private keys are plain bytes in the page's memory instead of keys the browser holds, so malicious code that gets into the page could copy them.",
              )}
            </p>
            <label className="flex cursor-pointer items-start gap-2">
              <input
                type="checkbox"
                data-testid="compat-understood"
                className="mt-0.5 accent-fg"
                checked={understood}
                onChange={(e) => setUnderstood(e.target.checked)}
              />
              <span className="text-xs text-fg">{t("I understand the risk")}</span>
            </label>
            <div className="flex flex-wrap items-center gap-2">
              <button type="button" data-testid="compat-trust" className={danger} disabled={!understood} onClick={onTrust}>
                {t("trust this browser")}
              </button>
              <span className="text-2xs text-faint">{t("Remembered for this browser, which then shows “compatibility mode”.")}</span>
            </div>
          </div>
        )}
        {held && (
          <div className="flex gap-2">
            <button type="button" data-testid="unsupported-sign-out" className={quiet} onClick={() => void signOut(undefined, held)}>
              {t("sign out")}
            </button>
          </div>
        )}
      </Panel>
    </Centered>
  );
}

/**
 * While this page runs in compatibility mode: in the bar (`compact` on a
 * phone's width) and on this device's row in 「我的设备」. Tapped, it says what
 * that means and offers to stop: the agreement is withdrawn and the page
 * reloads, to ask again before the device keys are used (they stay stored).
 */
export function CompatBadge({ className, compact = false }: { className?: string; compact?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (!compatModeActive()) return null;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger data-testid="compat-mode" data-signal="warn" className={cn("shrink-0 truncate text-warn-fg underline-offset-2 hover:underline", className)}>
        {compact ? (
          <>
            <span className="md:hidden">{t("compat.")}</span>
            <span className="max-md:hidden">{t("compatibility mode")}</span>
          </>
        ) : (
          t("compatibility mode")
        )}
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-72 max-w-[calc(100vw-2rem)]">
        <div data-testid="compat-card" className="flex flex-col gap-2 text-xs leading-5 normal-case tracking-normal">
          <p className="text-fg">{t("This browser runs Droppen's encryption in JavaScript: this device's private keys are plain bytes in the page's memory, not keys the browser holds.")}</p>
          <button
            type="button"
            data-testid="compat-withdraw"
            className={cn(quiet, "self-start")}
            onClick={() => {
              forgetCompatConsent();
              location.reload();
            }}
          >
            {t("stop using compatibility mode")}
          </button>
          <p className="text-2xs text-faint">{t("Its device keys stay stored in this browser; it asks again before it uses them.")}</p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
