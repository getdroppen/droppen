/*
 * A2A tokens (`docs/a2a.md` §2, §6): a client's credential, scoped to the
 * entries it may call. The secret exists in exactly one answer — the one to
 * `POST /api/a2a/tokens` — and this page holds it in one component's state
 * until it is dismissed: never in the query cache, never in storage, never
 * in the list. Revoking cancels the client's unfinished tasks.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, CircleNotchIcon, CopyIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/checkbox-field";
import { Input } from "@/components/ui/input";
import { SettingField, SettingsColumn, SettingsHint, SettingsNote } from "@/features/settings/layout";
import { useCopy } from "@/lib/clipboard";
import { formatRelative, formatTime } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { a2aAllTasksKey, a2aTokensKey } from "./api";
import { validTokenName, type A2aEntry, type A2aToken } from "./model";

/** A token just made, with the one copy of its secret there will ever be. */
export type TokenCreated = { token: A2aToken; secret: string };

/** The one look at a new token's secret: copy it, then it is gone from the page. */
export function TokenSecret({ created, onDismiss }: { created: TokenCreated; onDismiss: () => void }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  return (
    <div data-a2a-secret={created.token.id} className="mb-2 flex flex-col gap-2 border border-fg p-3">
      <SettingsNote tone="warn" mono={false} className="text-xs font-medium">
        {t("Copy the token for {{name}} now: you won't see it again.", { name: created.token.name })}
      </SettingsNote>
      <div className="flex items-center gap-1.5">
        <Input
          readOnly
          value={created.secret}
          aria-label={t("token")}
          onFocus={(e) => e.currentTarget.select()}
          spellCheck={false}
          className="min-w-0 flex-1 bg-raised font-mono"
        />
        <Button onClick={() => copy(created.secret)}>
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
          {copied ? t("copied") : t("copy")}
        </Button>
      </div>
      <SettingsHint>
        {t("The client sends it as Authorization: Bearer <token>. Droppen keeps only its hash: lost, revoke it and make another.")}
      </SettingsHint>
      <Button className="w-fit" onClick={onDismiss}>
        {t("done")}
      </Button>
    </div>
  );
}

/** A new token: a name and the entries it may call. `onCreated` gets the one copy of its secret. */
export function TokenForm({
  entries,
  onCreated,
  onCancel,
}: {
  entries: readonly A2aEntry[];
  onCreated: (created: TokenCreated) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string[]>(() => (entries.length === 1 ? [entries[0]!.id] : []));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const nameOk = validTokenName(name);
  const ready = nameOk && picked.length > 0 && !busy;

  // Not a mutation: its answer carries the secret, and a mutation's answer
  // stays in the query client's cache after the form is gone.
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.POST("/api/a2a/tokens", { body: { name: name.trim(), entries: picked } });
      if (res.error) throw new ApiError(res.error);
      onCreated(res.data);
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsColumn data-a2a-token-form="" className="mt-2 flex w-full flex-col gap-3 border border-border p-3">
      <SettingField label={t("name")} hint={t("· the caller tasks are recorded under")}>
        <Input value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} autoComplete="off" placeholder="dont-do" className="font-mono" />
      </SettingField>
      {name.trim() !== "" && !nameOk && (
        <SettingsNote tone="muted" mono={false} className="text-xs">
          {t("letters, digits, '.', '_' and '-', at most 64")}
        </SettingsNote>
      )}
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">{t("entries it may call")}</span>
        {entries.map((entry) => (
          <CheckboxField
            key={entry.id}
            checked={picked.includes(entry.id)}
            onCheckedChange={(checked) => setPicked((now) => (checked ? [...now, entry.id] : now.filter((id) => id !== entry.id)))}
            className="text-xs"
            labelClassName="text-xs text-fg"
            label={
              <>
                {entry.name} <span className="font-mono text-2xs text-faint">{entry.id}</span>
              </>
            }
          />
        ))}
      </div>
      {error !== null && <SettingsNote tone="danger">{errorText(error)}</SettingsNote>}
      <div className="flex items-center gap-1.5">
        <Button variant="primary" disabled={!ready} onClick={() => void submit()}>
          {busy && <CircleNotchIcon className="size-3 animate-spin" />}
          {t("create token")}
        </Button>
        <Button onClick={onCancel}>{t("cancel")}</Button>
      </div>
    </SettingsColumn>
  );
}

/** One token: its name, what it reaches, when it was made and last used; revoked ones only say when. */
export function TokenRow({ token, entryName }: { token: A2aToken; entryName: (id: string) => string }) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const revoke = useMutation({
    mutationFn: async () => {
      const res = await api.DELETE("/api/a2a/tokens/{id}", { params: { path: { id: token.id } } });
      if (res.error) throw new ApiError(res.error);
    },
    onSuccess: () => {
      setConfirming(false);
      void queryClient.invalidateQueries({ queryKey: a2aTokensKey(machine) });
      void queryClient.invalidateQueries({ queryKey: a2aAllTasksKey(machine) });
    },
  });
  const revoked = token.revokedAt !== null;
  const at = (iso: string, said: (when: string) => string) => <span title={formatTime(iso)}>{said(formatRelative(iso))}</span>;
  return (
    <div data-a2a-token={token.id} data-revoked={revoked ? "" : undefined} className="flex flex-col gap-1 border-b border-border py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className={revoked ? "font-mono text-xs text-faint line-through" : "font-mono text-xs text-fg"}>{token.name}</span>
        <span className="min-w-0 truncate font-mono text-2xs text-faint">{token.entries.map(entryName).join(", ")}</span>
        <span className="ml-auto flex shrink-0 items-center gap-2 font-mono text-2xs text-faint">
          {at(token.createdAt, (when) => t("created {{when}}", { when }))}
          {token.revokedAt !== null ? (
            at(token.revokedAt, (when) => t("revoked {{when}}", { when }))
          ) : token.lastUsedAt ? (
            at(token.lastUsedAt, (when) => t("last used {{when}}", { when }))
          ) : (
            <span>{t("never used")}</span>
          )}
          {!revoked && !confirming && <Button onClick={() => setConfirming(true)}>{t("revoke…")}</Button>}
        </span>
      </div>
      {confirming && (
        <div data-a2a-revoke-confirm="" className="flex flex-wrap items-center gap-2 pt-1">
          <SettingsNote tone="warn" mono={false} className="text-xs">
            {t("Revoke {{name}}? Its unfinished tasks are cancelled, and the client can no longer call any entry with it.", { name: token.name })}
          </SettingsNote>
          <Button variant="danger" data-signal="failed" disabled={revoke.isPending} onClick={() => revoke.mutate()}>
            {revoke.isPending ? t("revoking…") : t("revoke")}
          </Button>
          <Button onClick={() => setConfirming(false)}>{t("cancel")}</Button>
        </div>
      )}
      {revoke.isError && <SettingsNote tone="danger">{errorText(revoke.error)}</SettingsNote>}
    </div>
  );
}
