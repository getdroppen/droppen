import { PlusIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { errorText } from "@/api/errors";
import { Sheet } from "@/components/sheet";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { a2aTokensKey, useA2aEntries, useA2aTokens } from "@/features/a2a/api";
import { EntryRow } from "@/features/a2a/entries";
import { EntryFormView } from "@/features/a2a/entry-form";
import { cardBase } from "@/features/a2a/model";
import { TaskList } from "@/features/a2a/tasks";
import { TokenForm, TokenRow, TokenSecret, type TokenCreated } from "@/features/a2a/tokens";
import { SettingsColumn, SettingsHint, SettingsNote, SettingsSection } from "@/features/settings/layout";
import { isCloud } from "@/lib/hub/config";
import { useHomeMachine, useMachine } from "@/lib/machines/context";

/** The one form open at a time: a new entry, an entry by id, or a new token. */
type Open = { kind: "entry"; id: string | null } | { kind: "token" } | null;

/**
 * Settings › A2A (`docs/a2a.md`): what A2A is and what a token lets its
 * holder do, then the entries, the tokens and the tasks of the view's device.
 *
 * One form is open at a time, so the page never has two primary actions at
 * once (docs/ui-spec.md §7 V10). A new token's secret is not a form: it stays
 * until it is dismissed, whatever else is opened meanwhile.
 *
 * An entry's URL starts with this page's origin only on the device's own
 * page; on droppen.app, or for another device, this page cannot name the
 * address a client reaches that device at, and shows the path.
 */
export function A2aSection() {
  const { t } = useTranslation();
  const machine = useMachine();
  const home = useHomeMachine();
  const queryClient = useQueryClient();
  const entries = useA2aEntries();
  const tokens = useA2aTokens();
  const [open, setOpen] = useState<Open>(null);
  const [created, setCreated] = useState<TokenCreated | null>(null);

  const list = entries.data?.entries ?? [];
  const defaultTemplate = entries.data?.defaultTemplate ?? "";
  const ownPage = !isCloud() && machine === home;
  // TODO(API): the configured `a2a.publicBase` is not in the management API
  // yet; until it is, an entry's URL starts with this page's origin.
  const base = cardBase(null, ownPage ? window.location.origin : null);
  const live = (tokens.data?.tokens ?? []).filter((token) => token.revokedAt === null);
  const revoked = (tokens.data?.tokens ?? []).filter((token) => token.revokedAt !== null);
  const entryName = (id: string) => list.find((entry) => entry.id === id)?.name ?? id;
  const close = () => setOpen(null);

  return (
    <Sheet>
      <SettingsSection title={t("A2A")}>
        <SettingsColumn className="flex flex-col gap-2">
          <SettingsHint>
            {t(
              "A2A (Agent2Agent) lets another agent — a todo app, a planner — hand a coding task to this device. Each entry below is one agent it can call: the directory, the agent and its permissions are set here, and a task only says what to do. The task runs as a Droppen session you can watch; the client follows it, can answer its questions, and gets the result.",
            )}
          </SettingsHint>
          <p data-signal="warn" data-a2a-warning="" className="py-0.5 pl-2.5 text-xs leading-[18px] text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]">
            <span className="font-medium">{t("Anyone holding a token can make a session on this device read any file you can read and send it back.")}</span>{" "}
            {t("Give each client its own token, only for the entries it needs, and revoke it when you stop using it.")}
          </p>
        </SettingsColumn>
      </SettingsSection>

      <SettingsSection title={t("entries")} data-a2a-entries="">
        <SettingsColumn>
          <SettingsHint>
            {t("A client calls an entry at its URL. A token is accepted only on this device itself (127.0.0.1) or through a tunnel to it, never over the local network.")}
          </SettingsHint>
          <div className="mt-2">
            {entries.isPending && <Spinner />}
            {entries.error && <SettingsNote tone="danger">{errorText(entries.error)}</SettingsNote>}
            {entries.data && list.length === 0 && open?.kind !== "entry" && (
              <p className="py-2 text-xs text-muted">{t("no entries yet: add one to give an A2A client something to call")}</p>
            )}
            {list.map((entry) => (
              <EntryRow
                key={entry.id}
                entry={entry}
                base={base}
                defaultTemplate={defaultTemplate}
                editing={open?.kind === "entry" && open.id === entry.id}
                onEdit={() => setOpen({ kind: "entry", id: entry.id })}
                onDone={close}
              />
            ))}
          </div>
          {entries.data &&
            (open?.kind === "entry" && open.id === null ? (
              <EntryFormView defaultTemplate={defaultTemplate} onDone={close} />
            ) : (
              <Button className="mt-3" onClick={() => setOpen({ kind: "entry", id: null })}>
                <PlusIcon className="size-3" />
                {t("add entry")}
              </Button>
            ))}
        </SettingsColumn>
      </SettingsSection>

      <SettingsSection title={t("access tokens")} data-a2a-tokens="">
        <SettingsColumn>
          <SettingsHint>{t("A token lets one client call the entries you choose. Its secret is shown once, when it is made.")}</SettingsHint>
          <div className="mt-2">
            {created && <TokenSecret created={created} onDismiss={() => setCreated(null)} />}
            {tokens.isPending && <Spinner />}
            {tokens.error && <SettingsNote tone="danger">{errorText(tokens.error)}</SettingsNote>}
            {tokens.data && live.length === 0 && <p className="py-2 text-xs text-muted">{t("no tokens yet")}</p>}
            {live.map((token) => (
              <TokenRow key={token.id} token={token} entryName={entryName} />
            ))}
            {revoked.length > 0 && (
              <details className="mt-2">
                <summary className="cursor-pointer py-1 text-[13px] text-muted">{t("revoked tokens ({{n}})", { n: revoked.length })}</summary>
                {revoked.map((token) => (
                  <TokenRow key={token.id} token={token} entryName={entryName} />
                ))}
              </details>
            )}
          </div>
          {open?.kind === "token" ? (
            <TokenForm
              entries={list}
              onCancel={close}
              onCreated={(next) => {
                setCreated(next);
                close();
                void queryClient.invalidateQueries({ queryKey: a2aTokensKey(machine) });
              }}
            />
          ) : (
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button disabled={list.length === 0} onClick={() => setOpen({ kind: "token" })}>
                <PlusIcon className="size-3" />
                {t("new token")}
              </Button>
              {entries.data && list.length === 0 && <SettingsNote tone="muted" mono={false} className="text-xs">{t("add an entry first")}</SettingsNote>}
            </div>
          )}
        </SettingsColumn>
      </SettingsSection>

      <SettingsSection title={t("tasks")} className="border-b-0" data-a2a-task-list="">
        <SettingsColumn>
          <TaskList entries={list} />
        </SettingsColumn>
      </SettingsSection>
    </Sheet>
  );
}
