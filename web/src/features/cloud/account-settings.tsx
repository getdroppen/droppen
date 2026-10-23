/**
 * droppen.app's account settings (「账户」, `/account`, `devices-route.ts`):
 * the account's e-mail, how it signs in — its password and its passkeys
 * (docs/passkeys.md §3.6, §7.3) — its recovery words, and signing out.
 * Reached from the bar's 「账户」 and from 「我的设备」; like 「我的设备」 a page
 * with no device in view.
 *
 * Every way to sign in is changed from a full device only (the hub refuses
 * anything else, §3.4 B); any other device sees the page read-only. The
 * account keeps at least one way to sign in: the last passkey of an account
 * without a password, or the password of one without passkeys, cannot go.
 * Setting, changing or removing the password also asks for proof it is still
 * the person — a passkey of the account's, or the current password — so a
 * browser left signed in is no way to give the account someone else's
 * password.
 *
 * A passkey can also let new devices join (§4): its switch puts a passkey
 * slot on the chain (asking that passkey for its PRF: one more Face ID). A
 * new phone or browser then signs in with the passkey and asks to join; it
 * waits until a full device pairs with it (its QR code or 8-digit code), and
 * any full device can refuse it (`passkey-join.tsx`). Adding a passkey turns
 * this on in the same gesture where the authenticator answers the PRF at
 * creation. Which slot names which passkey is worked out here, from the
 * credential ids this page asks the authenticator with — never from a hash
 * the hub sends alongside.
 *
 * Every change to the ways to sign in is said on the account's other full
 * devices (`SignInChangeAlerts`: the hub records and pushes each).
 */
import { type FormEvent, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, KeyRound, Loader2, Plus } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { cn } from "@/lib/cn";
import { formatMoment } from "@/lib/format";
import { credentialHash, hex } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import { type AccountPasskey, HubApiError, PASSWORD_MIN_CHARS, type Reauth, type SignInMethods } from "@/lib/hub/api";
import type { HubLink } from "@/lib/hub/link";
import { fromBase64Url, pageCredentials, passkeyAdd, passkeyCancelled, passkeyPrf, passkeyReauth, signalUnknownCredential } from "@/lib/hub/passkey";
import { AccountNotices, PendingRecoveryBanner, SquareSwitch, useAccountVersion } from "./devices";
import { VerifyEmailBanner } from "./email-verify";
import { askRemoval, takeAskedRemoval } from "./devices-route";
import { AccountName, openMyDevices } from "./my-devices";
import { PendingPasskeyBanner } from "./passkey-join";
import { SignInChangeAlerts, useMe } from "./plan";
import { RecoveryWords } from "./recovery";
import { SignOutChoice } from "./sign-out-dialog";
import { CloudPage, danger, ErrorLine, errorText, field, Note, primary, quiet, secondary } from "./ui";

/** The page: the frame (the account by its e-mail, the way back to 「我的设备」, sign out), the account's alerts, then the settings. */
export function AccountScreen({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  const [words, setWords] = useState<string[] | null>(null);
  // A refused join's passkey to remove (the notice above): the settings open again with its removal open.
  const [asked, setAsked] = useState(0);
  if (words) {
    // A page of its own until they are written down, as in 「我的设备」.
    return (
      <div className="fixed inset-0 z-50 bg-bg">
        <RecoveryWords words={words} title={t("your new recovery words")} onDone={() => setWords(null)} />
      </div>
    );
  }
  return (
    <CloudPage
      aside={
        <>
          <AccountName account={account} link={link} />
          <button type="button" data-testid="account-back" className={cn(quiet, "h-7 px-2")} onClick={() => openMyDevices()}>
            <ArrowLeft className="size-3.5 shrink-0" />
            {t("my devices")}
          </button>
          <SignOutChoice account={account}>
            {(ask) => (
              <button type="button" className={cn(quiet, "h-7 px-2")} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </>
      }
    >
      <PendingPasskeyBanner
        account={account}
        link={link}
        onRemovePasskey={(credHash) => {
          askRemoval(credHash);
          setAsked((n) => n + 1);
        }}
      />
      <PendingRecoveryBanner account={account} link={link} />
      <SignInChangeAlerts account={account} link={link} />
      <VerifyEmailBanner account={account} />
      <AccountNotices account={account} link={link} />
      <AccountPanel key={asked} account={account} link={link} onWords={setWords} />
    </CloudPage>
  );
}

/** What the page edits at a time (one form open, so one primary action on the page). */
type Editing = { kind: "add" } | { kind: "password" } | { kind: "removePassword" } | { kind: "rename"; id: string } | { kind: "remove"; id: string } | { kind: "words" } | null;

/** The settings, with the account's sign-in methods read from the hub. */
export function AccountPanel({ account, link, onWords }: { account: CloudAccount; link: HubLink; onWords: (words: string[]) => void }) {
  const { t } = useTranslation();
  const me = useMe(account, link);
  const [methods, setMethods] = useState<SignInMethods | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // The relying party the hub's passkeys are for (`null` once known: the hub offers none).
  const [rpId, setRpId] = useState<string | null | undefined>(undefined);
  const token = account.session.token;
  const hashes = useCredentialHashes(methods);
  const reload = useCallback(async () => {
    try {
      setMethods(await account.api.signInMethods(account.session.token));
      setLoadError(null);
    } catch (err) {
      setLoadError(errorText(err));
    }
  }, [account]);
  // Changed from another device meanwhile (`sign_in_changed`): read the list again.
  const head = me?.signInChangeHead ?? 0;
  const seenHead = useRef(head);
  useEffect(() => {
    if (head <= seenHead.current) return;
    seenHead.current = head;
    void reload();
  }, [head, reload]);
  useEffect(() => {
    void reload();
    account.api
      .signupConfig()
      .then((c) => setRpId(c.passkeys?.rpId ?? null))
      .catch(() => setRpId(null));
  }, [account, reload]);
  return (
    <AccountSettings
      account={account}
      email={me?.view?.email.address ?? null}
      // Said only where the hub asks for it (a hub with verification off has nothing to say).
      verified={me?.view?.email.verification === "required" ? me.view.email.verified : null}
      methods={methods}
      hashes={hashes}
      loadError={loadError}
      rpId={rpId}
      token={token}
      onChanged={() => void reload()}
      onWords={onWords}
    />
  );
}

/** The settings as drawn, from what was read (`methods: null`: not yet). Exported to be drawn alone. */
export function AccountSettings({
  account,
  email,
  verified,
  methods,
  hashes,
  loadError,
  rpId,
  token,
  onChanged,
  onWords,
}: {
  account: CloudAccount;
  email: string | null;
  verified: boolean | null;
  methods: SignInMethods | null;
  /** Each passkey's credential hash (hex) as worked out here from its credential id (`useCredentialHashes`), by passkey id. */
  hashes: ReadonlyMap<string, string>;
  loadError: string | null;
  rpId: string | null | undefined;
  token: string;
  onChanged: () => void;
  onWords: (words: string[]) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<Editing>(null);
  const manager = account.standing === "active" && account.isAdmin;
  const webauthn = pageCredentials() !== null;
  const canAdd = manager && webauthn && !!rpId && methods !== null && methods.passkeys.length < methods.max;
  // Sent here to remove one passkey (a device that joined with it was not the person's: `passkey-join.tsx`): its removal, open.
  const [asked, setAsked] = useState<string | null>(null);
  useEffect(() => {
    const removal = takeAskedRemoval();
    if (removal) setAsked(removal);
  }, []);
  useEffect(() => {
    if (!asked || !methods || !manager) return;
    const p = methods.passkeys.find((k) => hashes.get(k.id) === asked);
    if (!p && hashes.size < methods.passkeys.length) return; // still hashing
    setAsked(null);
    if (p && editing === null) setEditing({ kind: "remove", id: p.id });
  }, [asked, methods, hashes, manager, editing]);
  return (
    <>
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-4 border-b border-border px-6 pt-8 pb-5 max-md:px-4 max-md:pt-6">
        <div className="flex min-w-0 flex-col gap-1.5">
          <h1 data-testid="account-title" className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg max-md:text-[26px]">
            {t("my account")}
          </h1>
          <p className="flex min-w-0 flex-wrap items-center gap-x-2 text-xs leading-5 text-muted">
            <span data-testid="account-email" className="truncate font-mono text-fg">
              {email ?? account.account}
            </span>
            {verified === true && <span className="font-mono text-2xs text-faint">{t("verified")}</span>}
            {verified === false && (
              <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
                {t("not verified yet")}
              </span>
            )}
          </p>
        </div>
        {canAdd && editing === null && (
          <button type="button" data-variant="primary" data-testid="add-passkey" className={cn(primary, "max-md:w-full")} onClick={() => setEditing({ kind: "add" })}>
            <Plus className="size-3.5" />
            {t("add a passkey")}
          </button>
        )}
      </div>
      {editing?.kind === "add" && rpId && methods && (
        <AddPasskey account={account} token={token} rpId={rpId} methods={methods} onDone={() => { setEditing(null); onChanged(); }} onCancel={() => setEditing(null)} />
      )}
      <section data-testid="sign-in-methods" className="border-b border-border">
        <SectionHead label={t("sign-in methods")} />
        {!manager && account.standing === "active" && (
          <p data-testid="read-only" className="border-b border-border px-6 py-3 text-xs leading-5 text-muted max-md:px-4">
            {t("Only a full device of the account changes how it signs in. Do it on one of those.")}
          </p>
        )}
        {methods === null ? (
          loadError ? (
            <div className="px-6 py-3 max-md:px-4">
              <ErrorLine message={loadError} />
            </div>
          ) : (
            <div className="flex h-12 items-center px-6 max-md:px-4">
              <Loader2 className="size-4 animate-spin text-faint" />
            </div>
          )
        ) : (
          <>
            <PasswordRow account={account} token={token} methods={methods} manager={manager} webauthn={webauthn} editing={editing} setEditing={setEditing} onChanged={onChanged} />
            <div className="flex h-10 items-center gap-2 border-t border-border px-6 max-md:px-4">
              <span className="text-xs font-medium text-fg">{t("passkeys")}</span>
              <span className="font-mono text-2xs text-faint tabular-nums">
                {methods.passkeys.length}/{methods.max}
              </span>
            </div>
            {methods.passkeys.length === 0 ? (
              <p data-testid="no-passkeys" className="border-t border-border px-6 py-3 text-xs leading-5 text-muted max-md:px-4">
                {rpId === null
                  ? t("This hub does not offer passkeys.")
                  : t("No passkeys yet. With one you sign in with Face ID, Touch ID or your device's PIN — and new devices can use it to ask to join the account.")}
              </p>
            ) : (
              <ul data-testid="passkey-list">
                {methods.passkeys.map((p) => (
                  <PasskeyRow
                    key={p.id}
                    account={account}
                    token={token}
                    passkey={p}
                    credHash={hashes.get(p.id) ?? null}
                    methods={methods}
                    manager={manager}
                    webauthn={webauthn}
                    rpId={rpId ?? null}
                    editing={editing}
                    setEditing={setEditing}
                    onChanged={onChanged}
                  />
                ))}
              </ul>
            )}
            <JoinExplainer />
          </>
        )}
      </section>
      <RecoveryWordsSection account={account} manager={manager} editing={editing} setEditing={setEditing} onWords={onWords} />
      <section data-testid="sign-out-section" className="border-b border-border">
        <SectionHead label={t("This device")} />
        <SettingRow
          title={t("sign out")}
          text={t("Sign out on this browser. Its offline copies are cleared and its notifications stop. On a device of your own it stays a device of the account; on a shared one, sign out and remove it.")}
        >
          <SignOutChoice account={account}>
            {(ask) => (
              <button type="button" data-testid="account-sign-out" className={cn(secondary, "h-7 px-3 text-xs pointer-coarse:h-9")} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </SettingRow>
      </section>
    </>
  );
}

function SectionHead({ label, children }: { label: string; children?: ReactNode }) {
  return (
    <div className="flex h-10 items-center gap-2.5 border-b border-border px-6 max-md:px-4">
      <CapsLabel className="text-fg">{label}</CapsLabel>
      {children}
    </div>
  );
}

/** A setting: what it is on the left (at most 48rem), its controls on the right — under it on a narrow page. */
function SettingRow({ title, text, children, testId }: { title: ReactNode; text: ReactNode; children?: ReactNode; testId?: string }) {
  return (
    <div data-testid={testId} className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2 px-6 py-3 max-md:px-4">
      <div className="flex max-w-[48rem] min-w-0 flex-1 flex-col gap-0.5 max-md:basis-full">
        <span className="text-xs font-medium text-fg">{title}</span>
        <span className="text-xs leading-5 text-muted">{text}</span>
      </div>
      {children && <div className="flex shrink-0 flex-wrap items-center gap-1.5">{children}</div>}
    </div>
  );
}

const rowButton = "h-7 px-3 text-xs pointer-coarse:h-9";

/**
 * Each passkey's credential hash (hex), worked out here from its credential
 * id — the id this page asks the authenticator with — so the slot a row
 * shows is the one that names that very credential, whatever else the hub
 * says. Missing while it is worked out.
 */
function useCredentialHashes(methods: SignInMethods | null): ReadonlyMap<string, string> {
  const [hashes, setHashes] = useState<ReadonlyMap<string, string>>(new Map());
  const ids = useMemo(() => methods?.passkeys.map((p) => [p.id, p.credentialId] as const) ?? [], [methods]);
  useEffect(() => {
    let live = true;
    void Promise.all(
      ids.map(async ([id, credentialId]) => {
        try {
          return [[id, hex(await credentialHash(fromBase64Url(credentialId)))] as const];
        } catch {
          return [];
        }
      }),
    ).then((pairs) => {
      if (live) setHashes(new Map(pairs.flat()));
    });
    return () => {
      live = false;
    };
  }, [ids]);
  return hashes;
}

/**
 * Remove passkey `p` with `proof`: the hub takes the proof first. Where a
 * slot on the chain still names it (409 `unlock_on`, with a grant for this
 * very removal) the slot goes first — this device signs `passkeySlotRemove`
 * for the slot that names this credential, by this page's own reckoning —
 * and the removal is asked again with the grant. A slot is never taken off
 * for a removal the hub would refuse. Exported for the tests.
 */
export async function removePasskey(
  account: Pick<CloudAccount, "api" | "disablePasskeyJoin" | "passkeySlotFor" | "sync">,
  token: string,
  p: Pick<AccountPasskey, "id" | "credentialId">,
  proof: Reauth,
): Promise<void> {
  try {
    await account.api.deletePasskey(token, p.id, proof);
    return;
  } catch (err) {
    if (!(err instanceof HubApiError) || err.code !== "unlock_on" || !err.grant) throw err;
    const hash = hex(await credentialHash(fromBase64Url(p.credentialId)));
    let slot = account.passkeySlotFor(hash);
    if (!slot) {
      // The hub's chain is past this view: look again.
      await account.sync();
      slot = account.passkeySlotFor(hash);
    }
    if (!slot) throw err;
    await account.disablePasskeyJoin(slot.slot.slot);
    await account.api.deletePasskey(token, p.id, { grant: err.grant });
  }
}

/** What a new device can do with a passkey here, in one place under the list. */
function JoinExplainer() {
  const { t } = useTranslation();
  return (
    <p data-testid="join-explainer" className="border-t border-border px-6 py-3 text-2xs leading-4 text-faint max-md:px-4">
      {t(
        "“New devices can join”: sign in with that passkey on a new phone or browser and it asks to join the account. It waits — reading nothing, controlling none of your devices — until you approve it from another of your devices by scanning the QR code it shows or typing its code; any full device can refuse it. Some browsers and password managers do not support this.",
      )}
    </p>
  );
}

// ------------------------------------------------------------ the password

function PasswordRow({
  account,
  token,
  methods,
  manager,
  webauthn,
  editing,
  setEditing,
  onChanged,
}: {
  account: CloudAccount;
  token: string;
  methods: SignInMethods;
  manager: boolean;
  webauthn: boolean;
  editing: Editing;
  setEditing: (e: Editing) => void;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const hasPasskey = methods.passkeys.length > 0;
  const open = editing?.kind === "password";
  const removing = editing?.kind === "removePassword";
  return (
    <div data-testid="password-row">
      <SettingRow
        title={t("password")}
        text={
          methods.password
            ? t("Set: you can sign in with your e-mail and this password.")
            : t("Not set: you sign in with a passkey only. Set one to sign in with your e-mail and a password too.")
        }
      >
        {manager && !open && !removing && (
          <>
            <button
              type="button"
              data-testid="password-edit"
              className={cn(secondary, rowButton)}
              disabled={editing !== null || (!methods.password && !(hasPasskey && webauthn))}
              title={!methods.password && !(hasPasskey && webauthn) ? t("setting a password asks for one of your passkeys, which this browser cannot use") : undefined}
              onClick={() => setEditing({ kind: "password" })}
            >
              {methods.password ? t("change password") : t("set a password")}
            </button>
            {methods.password && hasPasskey && webauthn && (
              <button type="button" data-testid="password-remove" className={cn(quiet, rowButton)} disabled={editing !== null} onClick={() => setEditing({ kind: "removePassword" })}>
                {t("remove")}
              </button>
            )}
          </>
        )}
      </SettingRow>
      {open && <PasswordForm account={account} token={token} methods={methods} webauthn={webauthn} onDone={() => { setEditing(null); onChanged(); }} onCancel={() => setEditing(null)} />}
      {removing && <RemovePassword account={account} token={token} methods={methods} webauthn={webauthn} onDone={() => { setEditing(null); onChanged(); }} onCancel={() => setEditing(null)} />}
    </div>
  );
}

/** Ask one of the account's passkeys to prove it is still the person (a gesture). */
async function passkeyProof(account: CloudAccount, token: string): Promise<Reauth> {
  const credentials = pageCredentials();
  if (!credentials) throw new Error("no passkeys in this browser");
  return { passkey: await passkeyReauth(account.api, { credentials }, { token }) };
}

/**
 * How a form proves it is still the person, as the hub asks before a way to
 * sign in is added or taken away: one of the account's passkeys (asked for
 * as the form is sent; the default where there is one and this browser can
 * use it), or the current password typed here (where there is one). `fields`
 * goes in the form; `ready` says whether it can be sent; `get` makes the proof.
 */
function useProof(account: CloudAccount, token: string, methods: SignInMethods, webauthn: boolean) {
  const { t } = useTranslation();
  const canPasskey = webauthn && methods.passkeys.length > 0;
  const [withPasskey, setWithPasskey] = useState(canPasskey || !methods.password);
  const [current, setCurrent] = useState("");
  const ready = withPasskey ? canPasskey : methods.password && current.length > 0;
  const fields = (
    <>
      {!withPasskey && methods.password && (
        <label className="flex flex-col gap-1.5">
          <span className="text-2xs text-muted">{t("current password")}</span>
          <input type="password" autoComplete="current-password" className={field} value={current} onChange={(e) => setCurrent(e.target.value)} />
        </label>
      )}
      {withPasskey && canPasskey && <p className="text-xs leading-5 text-muted">{t("First, one of your passkeys checks it is you.")}</p>}
      {methods.password && canPasskey && (
        <button type="button" className={cn(quiet, "h-7 self-start px-2 text-2xs")} onClick={() => setWithPasskey(!withPasskey)}>
          {withPasskey ? t("use the current password instead") : t("use a passkey instead")}
        </button>
      )}
    </>
  );
  const get = (): Promise<Reauth> => (withPasskey ? passkeyProof(account, token) : Promise.resolve({ currentPassword: current }));
  return { fields, ready, withPasskey, get };
}

/**
 * Set or change the password, once it is proved still the person: the
 * current password, or one of the account's passkeys. Setting the first one
 * (an account made with a passkey): a passkey, asked for as the form is sent.
 */
function PasswordForm({ account, token, methods, webauthn, onDone, onCancel }: { account: CloudAccount; token: string; methods: SignInMethods; webauthn: boolean; onDone: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const proof = useProof(account, token, methods, webauthn);
  const [next, setNext] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const short = [...next].length < PASSWORD_MIN_CHARS;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (short) return setError(t("a password needs at least {{n}} characters", { n: PASSWORD_MIN_CHARS }));
    setBusy(true);
    setError(null);
    try {
      await account.api.setPassword(token, next, await proof.get());
      onDone();
    } catch (err) {
      setError(passkeyCancelled(err) ? t("no passkey was used") : errorText(err));
      setBusy(false);
    }
  };
  return (
    <form data-testid="password-form" className="flex max-w-xl flex-col gap-3 px-6 pb-4 max-md:px-4" onSubmit={(e) => void submit(e)}>
      {proof.fields}
      <label className="flex flex-col gap-1.5">
        <span className="text-2xs text-muted">{methods.password ? t("new password") : t("password")}</span>
        <input type="password" autoComplete="new-password" className={field} value={next} onChange={(e) => setNext(e.target.value)} />
      </label>
      <div className="flex flex-wrap gap-2">
        <button type="submit" data-variant="primary" className={primary} disabled={busy || next.length === 0 || !proof.ready}>
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {proof.withPasskey ? t("check with a passkey and save") : t("save")}
        </button>
        <button type="button" className={quiet} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
      <ErrorLine message={error} />
    </form>
  );
}

/** Remove the password: the account then signs in with its passkeys only. A passkey (or the password itself) proves it is the person. */
function RemovePassword({ account, token, methods, webauthn, onDone, onCancel }: { account: CloudAccount; token: string; methods: SignInMethods; webauthn: boolean; onDone: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const proof = useProof(account, token, methods, webauthn);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      await account.api.deletePassword(token, await proof.get());
      onDone();
    } catch (err) {
      setError(passkeyCancelled(err) ? t("no passkey was used") : errorText(err));
      setBusy(false);
    }
  };
  return (
    <div className="px-6 pb-4 max-md:px-4">
      <Note tone="warn" className="max-w-xl">
        <p className="text-xs leading-5 text-muted">{t("Without a password this account signs in with its passkeys only.")}</p>
        {proof.fields}
        <div className="flex flex-wrap gap-2">
          <button type="button" className={cn(danger, "h-7 px-3 text-xs pointer-coarse:h-9")} disabled={busy || !proof.ready} onClick={() => void go()}>
            {busy && <Loader2 className="size-3 animate-spin" />}
            {t("remove the password")}
          </button>
          <button type="button" className={cn(quiet, "h-7 pointer-coarse:h-9")} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
        <ErrorLine message={error} />
      </Note>
    </div>
  );
}

// ------------------------------------------------------------ passkeys

/** Add a passkey; with "new devices can join" (on unless unticked), its slot too — in the same gesture where the authenticator gives the PRF at creation. */
function AddPasskey({
  account,
  token,
  rpId,
  methods,
  onDone,
  onCancel,
}: {
  account: CloudAccount;
  token: string;
  rpId: string;
  methods: SignInMethods;
  onDone: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  // Adding a way in takes proof it is still the person, as changing the password does.
  const proof = useProof(account, token, methods, true);
  const v3 = account.chain?.isV3 ?? false;
  const [label, setLabel] = useState("");
  const [join, setJoin] = useState(v3);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const add = async (e: FormEvent) => {
    e.preventDefault();
    const credentials = pageCredentials();
    if (!credentials || busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    let added = false;
    try {
      const plan = join ? account.passkeyJoinPlan() : null;
      const made = await passkeyAdd(account.api, { credentials }, { token, proof: await proof.get(), label: label.trim() || undefined, prfInput: plan?.prfInput, rpId });
      added = true;
      if (plan) {
        // No PRF at creation (many authenticators answer it only on use): ask the new passkey once more.
        const prf = made.prf ?? (await passkeyPrf({ credentials }, { rpId, credentialId: made.credentialId, prfInput: plan.prfInput })).prf;
        // The credential the authenticator made, as it gave it (not the hub's echo of it).
        await account.enablePasskeyJoin(fromBase64Url(made.credentialId), plan, prf);
      }
      onDone();
    } catch (err) {
      if (added) {
        // The passkey signs in; only letting new devices join with it did not happen.
        setNote(t("The passkey was added, but new devices cannot join with it yet: {{why}}", { why: passkeyCancelled(err) ? t("no passkey was used") : errorText(err) }));
        setBusy(false);
        return;
      }
      setError(passkeyCancelled(err) ? t("no passkey was created") : errorText(err));
      setBusy(false);
    }
  };
  return (
    <section data-testid="add-passkey-section" className="border-b border-border">
      <SectionHead label={t("add a passkey")} />
      <form className="flex max-w-xl flex-col gap-3 px-6 py-4 max-md:px-4" onSubmit={(e) => void add(e)}>
        <label className="flex flex-col gap-1.5">
          <span className="text-2xs text-muted">{t("name (optional)")}</span>
          <input className={field} value={label} maxLength={64} placeholder={t("e.g. iPhone, 1Password")} onChange={(e) => setLabel(e.target.value)} />
        </label>
        <label className={cn("flex items-start gap-2", !v3 && "opacity-60")}>
          <input type="checkbox" className="mt-0.5 accent-fg" checked={join} disabled={!v3} onChange={(e) => setJoin(e.target.checked)} />
          <span className="flex flex-col gap-0.5">
            <span className="text-xs font-medium text-fg">{t("new devices can join with it")}</span>
            <span className="text-xs leading-5 text-muted">
              {v3
                ? t("A new phone or browser that signs in with this passkey asks to join, and waits until you approve it from another of your devices with the QR code or code it shows.")
                : t("This needs the account upgraded first (in my devices).")}
            </span>
          </span>
        </label>
        {proof.fields}
        <div className="flex flex-wrap gap-2">
          <button type="submit" data-variant="primary" className={primary} disabled={busy || note !== null || !proof.ready}>
            {busy && <Loader2 className="size-3.5 animate-spin" />}
            {t("create a passkey")}
          </button>
          <button type="button" className={quiet} onClick={note !== null ? onDone : onCancel}>
            {note !== null ? t("done") : t("cancel")}
          </button>
        </div>
        {note && (
          <p data-signal="warn" className="text-xs leading-5 text-warn-fg">
            {note}
          </p>
        )}
        <ErrorLine message={error} />
      </form>
    </section>
  );
}

/** Why a passkey cannot be removed now (`null`: it can): the account's last way to sign in stays. */
function removeBlocked(methods: SignInMethods, t: (key: string) => string): string | null {
  if (!methods.password && methods.passkeys.length <= 1) return t("the account's last way to sign in: set a password or add another passkey first");
  return null;
}

function PasskeyRow({
  account,
  token,
  passkey: p,
  credHash,
  methods,
  manager,
  webauthn,
  rpId,
  editing,
  setEditing,
  onChanged,
}: {
  account: CloudAccount;
  token: string;
  passkey: AccountPasskey;
  /** Its credential hash as worked out here (`null`: not yet). */
  credHash: string | null;
  methods: SignInMethods;
  manager: boolean;
  webauthn: boolean;
  rpId: string | null;
  editing: Editing;
  setEditing: (e: Editing) => void;
  onChanged: () => void;
}) {
  const { t, i18n } = useTranslation();
  const slot = credHash ? account.passkeySlotFor(credHash) : null;
  const v3 = account.chain?.isV3 ?? false;
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [label, setLabel] = useState(p.label);
  const renaming = editing?.kind === "rename" && editing.id === p.id;
  const removing = editing?.kind === "remove" && editing.id === p.id;
  const blocked = removeBlocked(methods, t);
  // Removing a way in takes proof it is still the person (this passkey itself will do).
  const proof = useProof(account, token, methods, webauthn);
  const run = async (what: string, work: () => Promise<void>) => {
    setBusy(what);
    setError(null);
    try {
      await work();
      onChanged();
    } catch (err) {
      setError(passkeyCancelled(err) ? t("no passkey was used") : errorText(err));
    } finally {
      setBusy(null);
    }
  };
  const toggleJoin = (on: boolean) =>
    run("join", async () => {
      if (!on) return slot ? account.disablePasskeyJoin(slot.slot.slot) : undefined;
      const credentials = pageCredentials();
      if (!credentials) return;
      const plan = account.passkeyJoinPlan();
      const { credentialId, prf } = await passkeyPrf({ credentials }, { rpId: rpId ?? undefined, credentialId: p.credentialId, prfInput: plan.prfInput });
      await account.enablePasskeyJoin(fromBase64Url(credentialId), plan, prf);
    });
  // Why the switch cannot be moved here (`null`: it can; while another form is open it waits, unexplained).
  const joinBlocked = !manager
    ? t("only a full device changes this")
    : !v3
      ? t("This needs the account upgraded first (in my devices).")
      : !slot && (!webauthn || !rpId)
        ? t("this browser cannot use passkeys")
        : null;
  // (Held a moment, unexplained, while its credential hash is worked out.)
  const switchHeld = joinBlocked !== null || credHash === null || busy !== null || editing !== null;
  // Its name is the provider's when it was never named: said once.
  const named = !!p.label && p.label !== p.provider;
  const meta = [
    ...(named ? [p.provider] : []),
    p.backupEligible ? (p.backupState ? t("synced") : t("can sync")) : t("not synced: on this device or security key only"),
    t("added {{when}}", { when: formatMoment(p.createdMs, i18n.language) }),
    p.lastUsedMs ? t("last used {{when}}", { when: formatMoment(p.lastUsedMs, i18n.language, true) }) : t("never used"),
  ];
  return (
    <li data-testid="passkey-row" data-passkey={p.id} className="border-t border-border">
      <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2 px-6 py-3 max-md:px-4">
        <div className="flex min-w-0 flex-1 items-start gap-2.5 max-md:basis-full">
          <KeyRound className="mt-0.5 size-4 shrink-0 text-muted" />
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="truncate text-[13px] font-medium text-fg">{p.label || p.provider}</span>
            <span className="font-mono text-2xs break-words text-faint">{meta.join(" · ")}</span>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 max-md:pl-6.5">
          {/* The switch's words stay on a phone too (a bare switch says nothing there): the label clicks it. */}
          <label className={cn("flex cursor-pointer items-center pr-1.5 font-mono text-2xs", switchHeld ? "cursor-not-allowed text-faint" : "text-muted hover:text-fg")} title={joinBlocked ?? undefined}>
            <SquareSwitch on={slot !== null} blocked={switchHeld} title={joinBlocked ?? undefined} label={t("new devices can join with it")} labelHidden testId="join-switch" onChange={(on) => void toggleJoin(on)} />
            <span>{t("new devices can join with it")}</span>
          </label>
          {manager && !renaming && !removing && (
            <>
              <button type="button" className={cn(quiet, rowButton)} disabled={editing !== null || busy !== null} onClick={() => setEditing({ kind: "rename", id: p.id })}>
                {t("rename")}
              </button>
              <button
                type="button"
                data-testid="passkey-remove"
                className={cn(quiet, rowButton, "hover:text-danger")}
                disabled={editing !== null || busy !== null || blocked !== null}
                title={blocked ?? undefined}
                onClick={() => setEditing({ kind: "remove", id: p.id })}
              >
                {t("remove")}
              </button>
            </>
          )}
        </div>
      </div>
      {busy === "join" && (
        <p className="flex items-center gap-2 px-6 pb-3 text-2xs text-faint max-md:px-4">
          <Loader2 className="size-3 animate-spin" />
          {t("asking the passkey…")}
        </p>
      )}
      {renaming && (
        <form
          className="flex max-w-xl flex-wrap items-center gap-2 px-6 pb-3 max-md:px-4"
          onSubmit={(e) => {
            e.preventDefault();
            void run("rename", async () => {
              await account.api.renamePasskey(token, p.id, label);
              setEditing(null);
            });
          }}
        >
          <input aria-label={t("name")} className={cn(field, "min-w-0 flex-1")} value={label} maxLength={64} onChange={(e) => setLabel(e.target.value)} />
          <button type="submit" data-variant="primary" className={primary} disabled={busy !== null || label.trim().length === 0}>
            {t("save")}
          </button>
          <button type="button" className={quiet} onClick={() => setEditing(null)}>
            {t("cancel")}
          </button>
        </form>
      )}
      {removing && (
        <div className="px-6 pb-3 max-md:px-4">
          <Note tone="danger" className="max-w-xl">
            <p className="text-xs leading-5 text-muted">
              {t("Remove “{{name}}”? It no longer signs in to Droppen{{join}}. It stays in {{provider}} until you delete it there.", {
                name: p.label || p.provider,
                provider: p.provider,
                join: slot ? t(", and new devices can no longer join with it") : "",
              })}
            </p>
            {proof.fields}
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                data-testid="passkey-remove-confirm"
                className={cn(danger, "h-7 px-3 text-xs pointer-coarse:h-9")}
                disabled={busy !== null || !proof.ready}
                onClick={() =>
                  void run("remove", async () => {
                    // The proof first, at the hub; a slot that names it goes only once the hub took that (`removePasskey`).
                    await removePasskey(account, token, p, await proof.get());
                    if (rpId) await signalUnknownCredential(rpId, p.credentialId);
                    setEditing(null);
                  })
                }
              >
                {busy === "remove" && <Loader2 className="size-3 animate-spin" />}
                {t("remove")}
              </button>
              <button type="button" className={cn(quiet, "h-7 pointer-coarse:h-9")} onClick={() => setEditing(null)}>
                {t("cancel")}
              </button>
            </div>
          </Note>
        </div>
      )}
      {error && (
        <div className="px-6 pb-3 max-md:px-4">
          <ErrorLine message={error} />
        </div>
      )}
    </li>
  );
}

// ------------------------------------------------------------ the recovery words

function RecoveryWordsSection({
  account,
  manager,
  editing,
  setEditing,
  onWords,
}: {
  account: CloudAccount;
  manager: boolean;
  editing: Editing;
  setEditing: (e: Editing) => void;
  onWords: (words: string[]) => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const has = account.chain?.currentRecoveryKey != null;
  const confirming = editing?.kind === "words";
  const make = async () => {
    setBusy(true);
    setError(null);
    try {
      onWords(await account.setRecoveryWords());
      setEditing(null);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section data-testid="recovery-words-section" className="border-b border-border">
      <SectionHead label={t("recovery words")} />
      <SettingRow
        title={has ? t("set") : t("not set")}
        text={
          has
            ? t("If you lose every device, these 12 words bring the account back. New ones replace them: the old ones stop working at once.")
            : t("Without them, losing every device means losing everything encrypted. (Words are cleared when the device that made them, or one that came back with them, is removed.)")
        }
      >
        {manager && !confirming && (
          <button
            type="button"
            data-testid="new-words"
            className={cn(secondary, rowButton)}
            disabled={editing !== null || busy}
            onClick={() => (has ? setEditing({ kind: "words" }) : void make())}
          >
            {busy && <Loader2 className="size-3 animate-spin" />}
            {has ? t("make new recovery words") : t("create new recovery words")}
          </button>
        )}
      </SettingRow>
      {confirming && (
        <div className="px-6 pb-4 max-md:px-4">
          <Note tone="warn" className="max-w-xl">
            <p className="text-xs leading-5 text-muted">{t("The words you wrote down before stop working. Have something to write the new ones on.")}</p>
            <div className="flex flex-wrap gap-2">
              <button type="button" className={cn(secondary, "h-7 px-3 text-xs pointer-coarse:h-9")} disabled={busy} onClick={() => void make()}>
                {busy && <Loader2 className="size-3 animate-spin" />}
                {t("make new recovery words")}
              </button>
              <button type="button" className={cn(quiet, "h-7 pointer-coarse:h-9")} onClick={() => setEditing(null)}>
                {t("cancel")}
              </button>
            </div>
          </Note>
        </div>
      )}
      {error && (
        <div className="px-6 pb-3 max-md:px-4">
          <ErrorLine message={error} />
        </div>
      )}
    </section>
  );
}
