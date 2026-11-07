import { useState } from "react";
import { useTranslation } from "react-i18next";
import { BrandMark } from "@/components/brand-mark";
import { useSearch } from "@tanstack/react-router";
import { CircleNotchIcon } from "@phosphor-icons/react";
import { ApiError } from "@/api/errors";
import { login, pageDroppenCommand, safeNext } from "@/lib/auth";

/**
 * The server's one answer to every refused login — wrong name, wrong password,
 * or an account locked out after too many tries. It does not distinguish, and
 * neither does this page.
 */
const INVALID_CREDENTIALS = "invalid credentials";

/** The one screen this app draws for somebody it does not know yet. */
export function LoginView() {
  const { t } = useTranslation();
  const { next } = useSearch({ from: "/login" });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await login(username.trim(), password);
      // A full document load rather than a router navigation: everything that
      // matters here — the global feed socket, every query, the download ticket
      // prewarm — starts at boot, and a fresh boot is the simplest way to have
      // all of it start with the token instead of around it.
      location.replace(safeNext(next));
    } catch (err) {
      const code = err instanceof ApiError ? err.code : null;
      setError(
        code === INVALID_CREDENTIALS
          ? t("wrong username or password")
          : err instanceof Error
            ? err.message
            : String(err),
      );
      setBusy(false);
    }
  };

  const field =
    "h-8 w-full rounded border border-border bg-bg px-2 font-mono text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";

  return (
    <div className="flex h-full w-full items-center justify-center p-6">
      <form onSubmit={submit} className="flex w-full max-w-72 flex-col gap-3">
        <div className="mb-3 flex items-center gap-2.5">
          <BrandMark size={20} title="Droppen" />
          <span className="font-mono text-sm font-medium text-fg">Droppen</span>
        </div>
        <div className="flex items-center gap-2 border-b border-border pb-2">
          <h1 data-label="" className="label-caps text-2xs text-muted">{t("sign in")}</h1>
        </div>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("username")}</span>
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            // The page exists to be typed into, and it is the only thing on it.
            autoFocus
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("password")}</span>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            className={field}
          />
        </label>
        {error && <div data-signal="failed" className="font-mono text-2xs break-words text-danger">{error}</div>}
        <button
          type="submit"
          disabled={busy || username.trim() === "" || password === ""}
          data-variant="primary" className="flex h-8 items-center justify-center gap-1.5 bg-fg text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
        >
          {busy && <CircleNotchIcon className="size-3.5 animate-spin" />}
          {busy ? t("signing in…") : t("sign in")}
        </button>
        <p className="text-2xs text-faint">
          {t("Accounts are created on the server with `{{command}} user add <name>`.", { command: pageDroppenCommand() })}
        </p>
      </form>
    </div>
  );
}
