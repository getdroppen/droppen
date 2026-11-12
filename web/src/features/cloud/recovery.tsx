import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleNotchIcon } from "@phosphor-icons/react";
import { mnemonicCompletions } from "@/lib/e2e";
import { formatExactTime } from "@/lib/format";
import { AccountError, type CloudAccount } from "@/lib/hub/account";
import { challengeAnswers, checkPhrase, type PhraseCheck, pickChallenge } from "@/lib/hub/recovery-words";
import { cn } from "@/lib/cn";
import { Centered, ErrorLine, errorText, field, Panel, primary, quiet } from "./ui";

/*
 * The recovery words, as a person meets them (`lib/hub/recovery-words.ts`):
 *
 * - shown once (at genesis, or when new words replace cleared ones), then
 *   "I have written them down" and a few of them typed back before the page
 *   goes on;
 * - typed in to come back on a new browser, word by word with suggestions,
 *   saying which word is unknown or that the words do not check out;
 * - the recovered device waiting: for another device's approval, or for the
 *   delay after which the words (asked for again) activate it.
 */

/** Show the words, then ask a few back. */
export function RecoveryWords({ words, title: titled, onDone }: { words: string[]; title?: string; onDone: () => void }) {
  const { t } = useTranslation();
  const title = titled ?? t("your recovery words");
  const [step, setStep] = useState<"show" | "check">("show");
  const positions = useMemo(() => pickChallenge(words.length), [words]);
  const [answers, setAnswers] = useState<string[]>(() => positions.map(() => ""));
  const right = challengeAnswers(words, positions, answers);
  const passed = right.every(Boolean);

  if (step === "show") {
    return (
      <Centered>
        <Panel title={title}>
          <p className="text-xs leading-5 text-muted">
            {t("Write these words down, in order, and keep them somewhere safe. They are shown only now; with them you can get back in if you lose every device.")}
          </p>
          {/* A real sequence, so numbered 01–12 in hairline cells, read across. */}
          <ol data-testid="recovery-words" className="grid grid-cols-3 border-t border-l border-border font-mono text-xs text-fg select-all max-sm:grid-cols-2">
            {words.map((w, i) => (
              <li key={i} className="flex items-baseline gap-2 border-r border-b border-border px-2.5 py-2">
                <span className="w-5 text-2xs tabular-nums text-faint">{String(i + 1).padStart(2, "0")}</span>
                <span>{w}</span>
              </li>
            ))}
          </ol>
          <button type="button" data-variant="primary" className={primary} onClick={() => setStep("check")}>
            {t("I have written them down")}
          </button>
        </Panel>
      </Centered>
    );
  }
  return (
    <Centered>
      <form
        className="w-full max-w-[26rem]"
        onSubmit={(e) => {
          e.preventDefault();
          if (passed) onDone();
        }}
      >
        <Panel title={title}>
          <p className="text-xs leading-5 text-muted">{t("To check you have them, type these words from your list:")}</p>
          {positions.map((p, i) => (
            <label key={p} className="flex items-center gap-2">
              <span className="w-16 shrink-0 font-mono text-2xs text-faint">{t("word {{n}}", { n: p + 1 })}</span>
              <input
                value={answers[i]}
                onChange={(e) => setAnswers((a) => a.map((x, j) => (j === i ? e.target.value : x)))}
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus={i === 0}
                aria-invalid={answers[i]!.trim() !== "" && !right[i]}
                className={cn(field, answers[i]!.trim() !== "" && !right[i] && "border-danger")}
              />
            </label>
          ))}
          <button type="submit" disabled={!passed} data-variant="primary" className={primary}>
            {t("continue")}
          </button>
          <button type="button" className={quiet} onClick={() => setStep("show")}>
            {t("show the words again")}
          </button>
        </Panel>
      </form>
    </Centered>
  );
}

/** What the live check says about a phrase, in words. */
export function phraseMessage(check: PhraseCheck | null, t: (key: string, opts?: Record<string, unknown>) => string): { text: string; bad: boolean } | null {
  if (!check) return null;
  switch (check.kind) {
    case "empty":
      return null;
    case "incomplete":
      return { text: t("{{n}} of 12 words", { n: check.count }), bad: false };
    case "unknownWord":
      return { text: t("word {{n}}, “{{word}}”, is not a recovery word", { n: check.index + 1, word: check.word }), bad: true };
    case "wordCount":
      return { text: t("a recovery phrase is 12 words, not {{n}}", { n: check.count }), bad: true };
    case "checksum":
      return { text: t("these words do not check out: one is wrong or out of place"), bad: true };
    case "ok":
      return { text: check.legacy ? t("a recovery key from before words: accepted") : t("the words check out"), bad: false };
  }
}

/** The recovery words, typed: suggestions for the word being typed, and what is wrong as it is. */
export function PhraseInput({ value, onChange, onCheck }: { value: string; onChange: (v: string) => void; onCheck?: (c: PhraseCheck) => void }) {
  const { t } = useTranslation();
  const [check, setCheck] = useState<PhraseCheck | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    let live = true;
    void checkPhrase(value).then((c) => {
      if (!live) return;
      setCheck(c);
      onCheck?.(c);
    });
    return () => {
      live = false;
    };
    // onCheck is the caller's setter
  }, [value]);
  const current = /\s$/.test(value) ? "" : (value.trim().split(/\s+/).at(-1) ?? "");
  const suggestions = current.length >= 2 ? mnemonicCompletions(current, 5).filter((w) => w !== current.toLowerCase()) : [];
  const complete = (word: string) => {
    const head = value.replace(/\S*$/, "");
    onChange(`${head}${word} `);
    area.current?.focus();
  };
  const message = phraseMessage(check, t);
  return (
    <div className="flex flex-col gap-1">
      <textarea
        ref={area}
        data-testid="recovery-phrase"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Tab" && suggestions.length === 1) {
            e.preventDefault();
            complete(suggestions[0]!);
          }
        }}
        rows={3}
        autoComplete="off"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        placeholder={t("your 12 recovery words, separated by spaces")}
        className={cn(field, "h-auto py-1.5 leading-relaxed", message?.bad && "border-danger")}
      />
      {suggestions.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {suggestions.map((w) => (
            <button key={w} type="button" onClick={() => complete(w)} className="px-1.5 py-0.5 font-mono text-2xs text-muted shadow-[inset_0_0_0_1px_var(--color-border)] hover:text-fg">
              {w}
            </button>
          ))}
        </div>
      )}
      {message && <p className={cn("font-mono text-2xs", message.bad ? "text-danger" : "text-faint")}>{message.text}</p>}
    </div>
  );
}

function recoverError(error: unknown, t: (key: string, opts?: Record<string, unknown>) => string): string {
  if (error instanceof AccountError) {
    switch (error.kind) {
      case "wrongWords":
        return t("these are not this account's current recovery words");
      case "noRecoveryKey":
        return t("this account has no recovery words (any more): add this device from another one instead");
      case "noPackages":
        return t("nothing opens with these words: add this device from another one instead");
      case "tooEarly":
        return t("too early: this device can activate itself from {{time}}", { time: formatExactTime(new Date(error.readyMs ?? Date.now()).toISOString()) });
    }
  }
  return errorText(error);
}

/** On a browser not in the chain: come back with the words. */
export function RecoverForm({ account, onPending }: { account: CloudAccount; onPending: () => void }) {
  const { t } = useTranslation();
  const [phrase, setPhrase] = useState("");
  const [check, setCheck] = useState<PhraseCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={async (e) => {
        e.preventDefault();
        if (busy || check?.kind !== "ok") return;
        setBusy(true);
        setError(null);
        try {
          await account.recover(phrase);
          setPhrase("");
          onPending();
        } catch (err) {
          setError(recoverError(err, t));
          setBusy(false);
        }
      }}
    >
      <p className="text-xs leading-5 text-muted">
        {t("Type the recovery words you wrote down. This device then joins the account, but waits: another device can approve it at once, or it can activate itself after 24 hours.")}
      </p>
      <PhraseInput value={phrase} onChange={setPhrase} onCheck={setCheck} />
      <ErrorLine message={error} />
      <button type="submit" disabled={busy || check?.kind !== "ok"} data-variant="primary" className={primary}>
        {busy && <CircleNotchIcon className="size-3.5 animate-spin" />}
        {t("recover")}
      </button>
    </form>
  );
}

/** A recovered device, waiting: approval elsewhere, or the delay and the words again. */
export function PendingScreen({ account, onActive, onRemoved, onSignOut }: { account: CloudAccount; onActive: () => void; onRemoved: () => void; onSignOut: () => void }) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  const [phrase, setPhrase] = useState("");
  const [check, setCheck] = useState<PhraseCheck | null>(null);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const readyAt = account.recoveryReadyAt();
  const done = useRef({ onActive, onRemoved });
  done.current = { onActive, onRemoved };
  useEffect(() => {
    let stopped = false;
    const poll = async () => {
      try {
        await account.sync();
      } catch {
        // the next round
      }
      if (stopped) return;
      setNow(Date.now());
      if (account.standing === "active") done.current.onActive();
      else if (account.standing === "removed") done.current.onRemoved();
    };
    const timer = setInterval(() => void poll(), 5000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [account]);
  const ready = readyAt !== null && now >= readyAt;
  const readyText = readyAt === null ? "" : formatExactTime(new Date(readyAt).toISOString());
  return (
    <Centered>
      <Panel title={t("waiting for approval")}>
        <p className="text-xs leading-5 text-muted">
          {t("This device joined with the recovery words and is waiting. Approve it from another device of this account — every device shows a warning until then — or activate it here after the waiting period.")}
        </p>
        <p className="flex items-center gap-2 text-2xs text-faint">
          <CircleNotchIcon className="size-3 animate-spin" />
          {ready ? t("it can activate itself now") : t("it can activate itself from {{time}}", { time: readyText })}
        </p>
        {asking ? (
          <form
            className="flex flex-col gap-2"
            onSubmit={async (e) => {
              e.preventDefault();
              if (busy || check?.kind !== "ok") return;
              setBusy(true);
              setError(null);
              try {
                await account.activateRecovered(phrase);
                setPhrase("");
                onActive();
              } catch (err) {
                setError(recoverError(err, t));
                setBusy(false);
              }
            }}
          >
            <p className="text-xs leading-5 text-muted">{t("Type the recovery words again to activate this device.")}</p>
            <PhraseInput value={phrase} onChange={setPhrase} onCheck={setCheck} />
            <ErrorLine message={error} />
            <button type="submit" disabled={busy || check?.kind !== "ok"} data-variant="primary" className={primary}>
              {busy && <CircleNotchIcon className="size-3.5 animate-spin" />}
              {t("activate this device")}
            </button>
          </form>
        ) : (
          <button type="button" disabled={!ready} data-variant="primary" className={primary} onClick={() => setAsking(true)}>
            {t("activate this device")}
          </button>
        )}
        <span className="font-mono text-2xs text-faint">{account.deviceId}</span>
        <button type="button" className={quiet} onClick={onSignOut}>
          {t("sign out")}
        </button>
      </Panel>
    </Centered>
  );
}
