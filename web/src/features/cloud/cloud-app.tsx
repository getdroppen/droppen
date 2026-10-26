import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Trans, useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { checkE2eSupport, compatCovers, E2eUnsupportedError, type E2eSupport, indexedDbKeyStore, keepStorage, type KeyStore, type UnreadableReason } from "@/lib/e2e";
import { compatAgreedFor, compatModeActive, consentToCompat, e2eReadiness } from "@/lib/e2e/compat";
import { AccountError, CloudAccount, KeysUnreadableError } from "@/lib/hub/account";
import { authErrorKind, clearSession, HubApi, HubApiError, type HubSession, loadSession, type PasskeySignedIn, PASSWORD_MIN_CHARS, saveSession, SIGNUP_CONFIG, type SignedIn, type SignupConfig } from "@/lib/hub/api";
import {
  finishPasskeyLogin,
  type PasskeyAnswer,
  PasskeyAutofill,
  passkeyCancelled,
  passkeyLogin,
  passkeySignUp,
  pageCredentials,
  probePasskeys,
  webAuthnAvailable,
} from "@/lib/hub/passkey";
import { hubOrigin } from "@/lib/hub/config";
import { HubLink } from "@/lib/hub/link";
import { rememberPasskeySignIn } from "@/lib/hub/passkey-join";
import { ChainFollower } from "@/lib/hub/follow";
import { RelayPool } from "@/lib/hub/relays";
import { meStore } from "@/lib/hub/me";
import { RenewalRunner, setRenewalRunner } from "@/lib/hub/renewal";
import { followPushRenewals } from "@/lib/push";
import { useStore } from "@/lib/machines/store";
import { HubShell, shellCourier } from "./hub-shell";
import { hubShell, setHubShell } from "@/features/machines/shells";
import { AccountScreen } from "./account-settings";
import { isAccountPath } from "./devices-route";
import { MyDevicesScreen } from "./my-devices";
import { NoKeysScreen, PairingScreen, PasskeyPendingScreen } from "./pairing-screen";
import { PendingScreen, RecoveryWords } from "./recovery";
import { clearDeviceCopies, deletingAccount, type HeldSession, pairingResume, signInAgainToPair, signOut, takeAccountDeleted, takeSignOutFailures, takeStillListed, wipeAccountHere } from "./sign-out";
import { SignOutChoice, SignOutDialog, StillListedNote } from "./sign-out-dialog";
import { authErrorText, Centered, CloudPage, danger, ErrorLine, errorText, Field, field, Note, Panel, primary, quiet, Tabs } from "./ui";
import { CheckEmailScreen, VerifyEmailPage } from "./email-verify";
import { UnsupportedBrowserScreen } from "./browser-support";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { APP_PATH, bootRoute, frontPage, goPublic, isAuthPath, isFrontPath, PRIVACY_PATH, publicHref, publicLink, publicPage, SIGN_IN_PATH, SIGN_UP_PATH, TERMS_PATH, usePathname } from "@/features/landing/route";
import { BringItOver, reloadOnArrival } from "@/features/move/bring-it-over";
import { hostMode, moveOrigins } from "@/lib/hub/hosts";
import { isLinkPath, LinkFlow, NoOpenerScreen, startHandshake, useNoOpener } from "@/features/link/link-popup";
import { FrontPageView } from "@/features/landing/front-pages";
import { provideCloudParts } from "@/app/cloud-parts";
import { CloudBar } from "./cloud-bar";
import { AddDeviceHint, SideBySideTip } from "./onboarding-hints";
import { CopyWindowNotice } from "./plan";

// What of cloud mode the app's own views draw (`app/cloud-parts.ts`): there
// before this module's app renders the router.
provideCloudParts({ CloudBar, SideBySideTip, AddDeviceHint, CopyWindowNotice });

/*
 * The page in cloud mode, before (and around) the app: sign in or sign up at
 * the hub, set this browser up as a device of the account (its keys, and on a
 * new account the genesis with optional recovery words; on an existing one,
 * pairing from another device or coming back with the words), pick a machine
 * or add one — and once one is picked, the ordinary app, every request of
 * which goes to that machine over an end-to-end channel through the hub.
 * Every machine of the account is on the page at once (`hub-shell.ts`), so
 * moving between them — the machine list, the bar's switch — reloads nothing.
 * Throughout, the account's chain is followed live (`lib/hub/follow.ts`): a
 * device that finds itself removed wipes its keys and copies and says so. A
 * device the hub refuses without the chain showing it removed stops, but
 * keeps its keys, and asks to sign in again (which reads the chain).
 *
 * Only the device session is kept in this browser (renewed with the device
 * key's signature, never the password); a password sign-in is used once, to
 * register this browser and get it a device session.
 */

type Phase =
  | { kind: "boot" }
  | { kind: "auth"; error?: string }
  /** Signed in, on the landing page at `/` or the download page: nothing started until "Open app" leaves it. */
  | { kind: "front"; session: HubSession }
  /** Signed in, at `/verify` (the link in the verification mail): nothing started until "open the app". */
  | { kind: "verify"; session: HubSession }
  | { kind: "genesis"; account: CloudAccount }
  | { kind: "recovery"; account: CloudAccount; words: string[] }
  | { kind: "pairing"; account: CloudAccount }
  | { kind: "pending"; account: CloudAccount }
  | { kind: "noKeys"; account: CloudAccount }
  | { kind: "removed"; failures?: string[] }
  /** The hub says the account itself was deleted (from another device, or by its operator): keys and copies wiped. */
  | { kind: "deleted"; failures?: string[] }
  /** The hub refuses this device, but the chain this browser can check does not show it removed: keys kept. */
  | { kind: "refused"; reason: "removed" | "denied" }
  /** Past sign-in and setup: the account's machines (`hub-shell.ts`), the list or the app on one of them. */
  | { kind: "shell"; account: CloudAccount; link: HubLink; shell: HubShell }
  /** The account's chain as served conflicts with what this browser verified (`CloudAccount.forked`): everything stopped. */
  | { kind: "fork"; message: string; held?: HeldSession }
  /**
   * This browser cannot run the end-to-end encryption (`missing`): nothing read
   * or made. `compat`: compatibility mode could stand in, once the person
   * agrees; `session` goes on then.
   */
  | { kind: "unsupported"; missing: string[]; compat: boolean; session: HubSession; held: HeldSession }
  /** This browser's device keys did not come (`reason`): nothing replaced; try again, or (chosen) a new device. */
  | { kind: "keysUnreadable"; session: HubSession; reason: UnreadableReason; message: string }
  /** `held`: the hub session the page was using, ended by this screen's sign-out (no account is open to end it). */
  | { kind: "error"; message: string; held?: HeldSession };

function startLink(account: CloudAccount, origin: string): HubLink {
  const link = new HubLink({
    url: `${origin}/v1/client`,
    origin,
    account: account.account,
    deviceId: account.deviceId,
    // The device session as it stands at each sign-in (it is renewed as it goes).
    token: () => account.session.token,
    signKey: account.keys.sign.privateKey,
    renew: () => account.renewSession(),
  });
  link.start();
  return link;
}

/**
 * `store` and `keyReadRetryMs` stand in for this browser's IndexedDB and the
 * waits between reads of it (tests); the page uses its own.
 */
/** On the app host, the hub's origin a browser's device can be brought over from (the key move); else `null`. */
function movedFrom(): string | null {
  return hostMode() === "app" ? (moveOrigins()?.legacy ?? null) : null;
}

export function CloudApp({ children, store: givenStore, keyReadRetryMs }: { children: ReactNode; store?: KeyStore; keyReadRetryMs?: readonly number[] }) {
  const origin = hubOrigin()!;
  const api = useMemo(() => new HubApi(), []);
  const store = useMemo<KeyStore>(() => givenStore ?? indexedDbKeyStore(), [givenStore]);
  const [phase, setPhase] = useState<Phase>({ kind: "boot" });
  // The sign-out question a screen before the shell asked (pairing, waiting, no keys): for its account.
  const [askSignOut, setAskSignOut] = useState<CloudAccount | null>(null);
  const withSignOut = (screen: ReactNode) => (
    <>
      {screen}
      {askSignOut && <SignOutDialog account={askSignOut} onOpenChange={(open) => void (open || setAskSignOut(null))} />}
    </>
  );
  // Whether this browser can run the end-to-end encryption: checked once, as
  // the page starts, before any key is read or made (`lib/e2e/support.ts`).
  const supportCheck = useRef<Promise<E2eSupport> | null>(null);
  const checkSupport = useCallback(() => (supportCheck.current ??= checkE2eSupport()), []);
  const [support, setSupport] = useState<E2eSupport | null>(null);
  // Bumped when the person agrees to compatibility mode (`lib/e2e/compat.ts`), which is read from storage.
  const [, setConsents] = useState(0);
  useEffect(() => {
    let live = true;
    void checkSupport().then((s) => live && setSupport(s));
    return () => {
      live = false;
    };
  }, [checkSupport]);
  const booted = useRef(false);
  const opened = useRef<CloudAccount | null>(null);
  const queryClient = useQueryClient();
  const pathname = usePathname();
  // droppen.app/link, opened by a computer's local settings: the handshake
  // with that page starts at once, whatever the sign-in still has to do.
  const onLink = isLinkPath(pathname);
  if (onLink) startHandshake();
  const noOpener = useNoOpener();

  /**
   * The hub will not have this device: wipe it only if the chain this
   * browser verified shows the removal, or the hub says the account itself
   * was deleted (nothing is left for the keys to open); else stop and keep
   * the keys — a suspended account's included.
   */
  const refused = useCallback(
    async (account: CloudAccount, link: HubLink | null) => {
      // This page is deleting the account: the hub ending its sessions is that, and the page leaves on its own.
      if (deletingAccount()) return;
      account.stopSession();
      link?.stop();
      const refusal = account.refusal;
      if (refusal === "deleted") {
        const failures = await wipeRemoved(account, link, store);
        return setPhase({ kind: "deleted", failures });
      }
      if (account.standing === "removed") {
        const failures = await wipeRemoved(account, link, store);
        return setPhase({ kind: "removed", failures });
      }
      clearSession();
      hubShell()?.stop();
      setHubShell(null);
      setRenewalRunner(null);
      setPhase({ kind: "refused", reason: refusal ?? "denied" });
    },
    [store],
  );

  /** The chain as served conflicts with what this browser verified: stop everything (the keys stay) and say so. */
  const forked = useCallback((account: CloudAccount, link: HubLink | null, message: string) => {
    account.stopSession();
    link?.stop();
    hubShell()?.stop();
    setHubShell(null);
    setPhase({ kind: "fork", message, held: { api: account.api, token: account.session.token, account: account.account, store } });
  }, [store]);

  /** Where a signed-in session (a kept device session, or a password sign-in) goes next. */
  const proceed = useCallback(
    async (session: HubSession, { replaceUnreadableKeys = false }: { replaceUnreadableKeys?: boolean } = {}) => {
      try {
        // Before any key is read or made: a browser that cannot run the
        // encryption says so (or, agreed to before, goes on in compatibility
        // mode), instead of failing at its keys as if its storage had.
        const ready = await e2eReadiness(await checkSupport());
        if (ready.kind === "unsupported") {
          return setPhase({ kind: "unsupported", missing: ready.missing, compat: ready.compat, session, held: { api, token: session.token, account: session.account, store } });
        }
        const account = await CloudAccount.open(api, session, store, { origin, onSession: saveSession, replaceUnreadableKeys, keyReadRetryMs });
        // The page's account from before (a screen moving on) renews nothing any more: this one does.
        if (opened.current && opened.current !== account) opened.current.stopSession();
        opened.current = account;
        if (account.refusal) return await refused(account, null);
        let link: HubLink | null = null;
        let stopped = false;
        account.onChange(() => {
          if (stopped) return;
          if (account.forked !== null) {
            stopped = true;
            return forked(account, link, account.forked);
          }
          if (!account.refusal) return;
          stopped = true;
          void refused(account, link);
        });
        // Listened for first: a fork the seen record's watch finds as it starts (in here) is heard.
        account.keepSessionFresh();
        if (stopped) return;
        if (account.forked !== null) {
          stopped = true;
          return forked(account, null, account.forked);
        }
        if (!account.chain) return setPhase({ kind: "genesis", account });
        if (account.standing === "removed") {
          const failures = await wipeRemoved(account, null, store);
          return setPhase({ kind: "removed", failures });
        }
        if (account.standing === "pending") return setPhase({ kind: "pending", account });
        if (account.standing !== "active") return setPhase({ kind: "pairing", account });
        // A device of the account: ask the browser to keep its keys out of storage eviction.
        keepDeviceStorage();
        // In the account, but nothing here can be read: its keys never
        // arrived, or this browser never confirmed the account. It pairs again.
        if (await account.lacksKeys()) return setPhase({ kind: "noKeys", account });
        // A fork latched while that was read stops here, before any link or shell starts.
        if (stopped) return;
        if (account.forked !== null) {
          stopped = true;
          return forked(account, null, account.forked);
        }
        link = startLink(account, origin);
        const live = link;
        // Follow the chain (attesting each new head), take keys mailed to
        // this device, and notice it being removed (or refused).
        new ChainFollower({
          link,
          account,
          onRemoved: () => {
            stopped = true;
            void wipeRemoved(account, live, store).then((failures) => setPhase({ kind: "removed", failures }));
          },
          onRefused: () => {
            if (stopped) return;
            stopped = true;
            void refused(account, live);
          },
          onAlarm: (message) => {
            console.error(`ALARM: ${message}`);
            if (stopped) return;
            stopped = true;
            forked(account, live, message);
          },
        }).start();
        // The relays channels go through (relay-and-plans §2.6); the hub carries one only when none can.
        const relays = new RelayPool({ api, account });
        relays.start();
        // Every machine of the account, each on its copy until it is wanted
        // live, and the one this browser last had in view brought back.
        hubShell()?.stop();
        const shell = new HubShell({ api, account, link, relays, store, queryClient });
        setHubShell(shell);
        // Every admin page renews the identity keys of the devices that run AI a removed device could control
        // (docs/host-identity-rotation.md §4.2); every page shows where they stand.
        const renewals = new RenewalRunner({ account, link: live, courier: () => shellCourier(live, account), me: meStore(account, live) });
        setRenewalRunner(renewals);
        renewals.start();
        // This browser's push subscription follows its device's renewed key (§2.2 推送).
        followPushRenewals(account);
        // The sign-in popup (`/link`) opens no machine: it adds one.
        if (!isLinkPath(location.pathname)) shell.start();
        // For diagnostics (and the acceptance harness's latency probe): the
        // page's own way to the machine in view. Exposes nothing a script in
        // this page could not already reach.
        Object.defineProperty(globalThis, "__rhCloud", { configurable: true, get: () => shell.diagnostics() });
        setPhase({ kind: "shell", account, link, shell });
      } catch (error) {
        if ((error instanceof HubApiError && error.status === 401) || (error instanceof AccountError && error.kind === "refused")) {
          clearSession();
          // Someone who was signed in here: the sign-in form, not the landing page.
          if (publicPage(location.pathname) === "landing") goPublic(SIGN_IN_PATH, { replace: true });
          return setPhase({ kind: "auth", error: undefined });
        }
        // The session the page was using (renewed, if the account opened), for these screens' sign-out to end.
        const held: HeldSession = { api, token: (opened.current?.account === session.account && opened.current.session.token) || session.token, account: session.account, store };
        if (error instanceof AccountError && error.kind === "fork") return setPhase({ kind: "fork", message: error.message, held });
        // An algorithm the browser turned out not to know, past the check: the same screen, never "storage did not answer".
        if (error instanceof E2eUnsupportedError) {
          const compat = !compatModeActive() && compatCovers({ ok: false, missing: error.missing });
          return setPhase({ kind: "unsupported", missing: error.missing, compat, session, held });
        }
        if (error instanceof KeysUnreadableError) return setPhase({ kind: "keysUnreadable", session, reason: error.reason, message: error.message });
        setPhase({ kind: "error", message: errorText(error), held });
      }
    },
    [api, store, origin, queryClient, refused, forked, checkSupport, keyReadRetryMs],
  );

  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    const session = loadSession();
    const route = bootRoute(location.pathname, session !== null);
    // `/`, `/download`, `/pricing`, `/privacy`, `/security` and `/terms` are pages of their own, signed in or not; signed in, they start nothing until "Open app".
    if (route.show === "landing" || route.show === "download" || route.show === "pricing" || route.show === "privacy" || route.show === "security" || route.show === "terms") setPhase(session ? { kind: "front", session } : { kind: "auth" });
    // The verification link: the same page signed in or not (signed out, it is one of the public pages).
    else if (route.show === "verify") setPhase(session ? { kind: "verify", session } : { kind: "auth" });
    else if (route.show === "auth" || !session) setPhase({ kind: "auth" });
    else {
      // Signed in already: no sign-in page, straight on to the app's start (or a deep link's page).
      if (route.replace) goPublic(route.replace, { replace: true });
      void proceed(session);
    }
  }, [proceed]);

  // Another tab of this origin took this browser's device over from the hub's origin (the key move,
  // `features/move`): a tab with no account open reloads, and opens it.
  useEffect(() => reloadOnArrival({ isOpen: () => opened.current !== null }), []);

  // "Open app" on the landing page (or the download page): on into the app.
  useEffect(() => {
    if (phase.kind !== "front" || isFrontPath(pathname)) return;
    if (isAuthPath(location.pathname)) goPublic(APP_PATH, { replace: true });
    setPhase({ kind: "boot" });
    void proceed(phase.session);
  }, [phase, pathname, proceed]);

  // Back (or forward) to `/` once signed in: the landing page, from a clean
  // load. The app's router would send `/` on to the sessions (and back would
  // never get past it); a navigation started here outlives that redirect.
  // The download, pricing, privacy, security and terms pages likewise.
  const signedIn = phase.kind !== "auth" && phase.kind !== "front";
  useEffect(() => {
    if (!signedIn) return;
    const back = () => {
      if (isFrontPath(location.pathname)) location.replace(location.pathname);
    };
    addEventListener("popstate", back);
    return () => removeEventListener("popstate", back);
  }, [signedIn]);

  if (onLink && noOpener) return <NoOpenerScreen />;
  switch (phase.kind) {
    case "boot":
      return <Centered><Loader2 className="size-4 animate-spin text-faint" /></Centered>;
    case "front":
      return <FrontPageView page={frontPage(pathname) ?? "landing"} signedIn api={api} />;
    case "verify":
      return (
        <VerifyEmailPage
          api={api}
          onContinue={() => {
            goPublic(APP_PATH, { replace: true });
            setPhase({ kind: "boot" });
            void proceed(phase.session);
          }}
        />
      );
    case "auth": {
      // Signed out: the landing, download, pricing, privacy, security and terms pages at their
      // paths, sign-in and sign-up at their own, the verification link's page
      // at `/verify`, and any other path (a deep link into the app) the
      // sign-in form in place, so the link still opens once the visitor is in.
      const page = publicPage(pathname);
      const front = frontPage(pathname);
      if (front) return <FrontPageView page={front} signedIn={false} api={api} />;
      if (page === "verify") return <VerifyEmailPage api={api} />;
      // A browser that cannot run the encryption is told so first, before an
      // account is signed in to (or made) that it could not open here.
      if (!support) return <Centered><Loader2 className="size-4 animate-spin text-faint" /></Centered>;
      if (!support.ok && !(compatCovers(support) && compatAgreedFor(support.missing))) {
        const compat = compatCovers(support);
        return (
          <UnsupportedBrowserScreen
            missing={support.missing}
            compat={compat}
            onTrust={
              compat
                ? () => {
                    consentToCompat(support.missing);
                    setConsents((n) => n + 1);
                  }
                : undefined
            }
          />
        );
      }
      return (
        <AuthScreen
          api={api}
          initialMode={page === "signup" ? "signup" : "signin"}
          above={movedFrom() ? <BringItOver legacy={movedFrom()!} /> : null}
          onSignedIn={(session) => {
            // The app's router knows no `/signin`: it starts at its start (a deep link keeps its path).
            if (isAuthPath(location.pathname)) goPublic(APP_PATH, { replace: true });
            // A login token is not kept: it gets this browser its device session, which is.
            setPhase({ kind: "boot" });
            void proceed(session);
          }}
        />
      );
    }
    case "genesis":
      return (
        <GenesisScreen
          account={phase.account}
          onDone={(words) => (words ? setPhase({ kind: "recovery", account: phase.account, words }) : void proceed(phase.account.session))}
        />
      );
    case "recovery":
      return <RecoveryWords words={phase.words} onDone={() => void proceed(phase.account.session)} />;
    case "pairing":
      return withSignOut(
        <PairingScreen
          account={phase.account}
          onPaired={() => void proceed(phase.account.session)}
          onPending={() => setPhase({ kind: "pending", account: phase.account })}
          onSignOut={() => setAskSignOut(phase.account)}
          onSignInAgain={(mode) => void signInAgainToPair(phase.account, mode)}
        />,
      );
    case "pending": {
      // Joined with a passkey: the pairing offer another device approves it by; with the recovery words: their own waiting screen.
      const Waiting = phase.account.chain?.deviceRecord(phase.account.deviceId)?.viaPasskey ? PasskeyPendingScreen : PendingScreen;
      return withSignOut(
        <Waiting
          account={phase.account}
          onActive={() => void proceed(phase.account.session)}
          onRemoved={() => void wipeRemoved(phase.account, null, store).then((failures) => setPhase({ kind: "removed", failures }))}
          onSignOut={() => setAskSignOut(phase.account)}
        />,
      );
    }
    case "noKeys":
      return withSignOut(
        <NoKeysScreen
          account={phase.account}
          onKeys={() => void proceed(phase.account.session)}
          onRemoved={() => void wipeRemoved(phase.account, null, store).then((failures) => setPhase({ kind: "removed", failures }))}
          onSignOut={() => setAskSignOut(phase.account)}
        />,
      );
    case "removed":
      return <RemovedScreen failures={phase.failures} />;
    case "deleted":
      return <AccountDeletedScreen failures={phase.failures} />;
    case "refused":
      return <RefusedScreen reason={phase.reason} />;
    case "shell":
      if (onLink) return <LinkFlow account={phase.account} link={phase.link} />;
      return (
        <ShellView shell={phase.shell} origin={origin}>
          {children}
        </ShellView>
      );
    case "fork":
      return <ForkScreen message={phase.message} held={phase.held} />;
    case "unsupported": {
      const { session } = phase;
      return (
        <UnsupportedBrowserScreen
          missing={phase.missing}
          compat={phase.compat}
          held={phase.held}
          onTrust={
            phase.compat
              ? () => {
                  consentToCompat(phase.missing);
                  setConsents((n) => n + 1);
                  setPhase({ kind: "boot" });
                  void proceed(session);
                }
              : undefined
          }
        />
      );
    }
    case "keysUnreadable": {
      const { session } = phase;
      return (
        <KeysUnreadableScreen
          reason={phase.reason}
          message={phase.message}
          held={{ api, token: session.token, account: session.account, store }}
          // A kept device session outlives a reload, which reads the store on a fresh page; a sign-in's login token does not.
          onRetry={() => (session.device ? location.reload() : (setPhase({ kind: "boot" }), void proceed(session)))}
          onNewDevice={() => {
            setPhase({ kind: "boot" });
            void proceed(session, { replaceUnreadableKeys: true });
          }}
        />
      );
    }
    case "error":
      return <ErrorScreen message={phase.message} held={phase.held} />;
  }
}

/**
 * The account's chain, as the hub serves it, conflicts with one this browser
 * verified (a fork): the page stopped using the account. Exported so the
 * screen can be previewed on its own.
 */
export function ForkScreen({ message, held }: { message: string; held?: HeldSession }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("the account does not match what this browser verified")}>
        <Note tone="danger">
          <p className="text-xs leading-5 text-danger">
            {t(
              "Droppen stopped using the account in this browser: the hub served a history of the account's devices that conflicts with the one this browser verified before. It may be holding a change back, or showing your devices different histories. Look at the account's devices from another device you trust before you use it here again.",
            )}
          </p>
          <div data-signal="fork" className="font-mono text-2xs break-words text-danger">{message}</div>
        </Note>
        <div className="flex gap-2">
          <button type="button" data-variant="primary" className={primary} onClick={() => location.reload()}>
            {t("check again")}
          </button>
          <SignOutChoice held={held}>
            {(ask) => (
              <button type="button" className={quiet} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </div>
      </Panel>
    </Centered>
  );
}

/**
 * This browser's device keys did not come (`reason`), even read again:
 * nothing was replaced, and nothing signs in as a new device unless the
 * person asks — after a question of its own, with a confirm button of its
 * own (a double tap on "use this browser as a new device" cannot reach it)
 * and a way back. When the keys will never open again (`gone`) that is the
 * thing to do, and says so; otherwise trying again is. Exported so the
 * screen can be previewed on its own.
 */
export function KeysUnreadableScreen({
  reason,
  message,
  held,
  onRetry,
  onNewDevice,
}: {
  reason: UnreadableReason;
  message: string;
  held?: HeldSession;
  onRetry: () => void;
  onNewDevice: () => void;
}) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const question = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const opened = useRef(false);
  const warning = useId();
  // The question takes the focus (and a screen reader's attention) when it
  // opens; closed again, the focus goes back to the button that opened it.
  useEffect(() => {
    if (confirming) question.current?.focus();
    else if (opened.current) trigger.current?.focus();
    opened.current = confirming;
  }, [confirming]);
  const gone = reason === "gone";
  const ask = (
    <button
      ref={trigger}
      type="button"
      data-testid="keys-new-device"
      data-variant={gone ? "primary" : undefined}
      className={gone ? primary : quiet}
      disabled={confirming}
      aria-expanded={confirming}
      onClick={() => setConfirming(true)}
    >
      {t("use this browser as a new device")}
    </button>
  );
  const retry = (
    <button type="button" data-testid="keys-retry" data-variant={gone ? undefined : "primary"} className={gone ? quiet : primary} onClick={onRetry}>
      {t("try again")}
    </button>
  );
  return (
    <Centered>
      <Panel title={t("this browser could not open its device keys")}>
        <Note tone={gone ? "danger" : "warn"}>
          <p data-testid="keys-unreadable-why" className="text-xs leading-5">
            {reason === "gone"
              ? t(
                  "This browser can no longer open the keys it stored for this device: the key it protects stored keys with has changed, so they will not come back. Use this browser as a new device; the old one then has to be removed in my devices on another device.",
                )
              : reason === "stored"
                ? t(
                    "The keys are still stored in this browser, but it would not hand them back just now. Nothing was replaced: this is often temporary, for example right after the page was reloaded in the background. Try again.",
                  )
                : t("This browser's storage did not answer when its device keys were read. Nothing was replaced: try again in a moment.")}
          </p>
          <div data-signal="keys-unreadable" className="font-mono text-2xs break-words text-faint">{message}</div>
        </Note>
        <div className="flex flex-wrap gap-2">
          {gone ? ask : retry}
          {gone ? retry : ask}
          <SignOutChoice held={held}>
            {(ask) => (
              <button type="button" className={quiet} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </div>
        {confirming && (
          <div
            ref={question}
            role="alertdialog"
            aria-label={t("use this browser as a new device")}
            aria-describedby={warning}
            tabIndex={-1}
            data-testid="keys-new-device-confirm"
            className="flex flex-col gap-2 border border-border bg-danger/4 p-3 shadow-[inset_2px_0_0_var(--color-danger)] outline-none"
          >
            <p id={warning} className="text-xs leading-5 text-danger">
              {t(
                "Signing in as a new device replaces these keys for good: this browser then has to be paired again, and its old device must be removed in my devices on another device.",
              )}
            </p>
            <div className="flex gap-2">
              <button type="button" data-testid="keys-new-device-confirm-button" className={danger} onClick={onNewDevice}>
                {t("replace the keys and sign in as a new device")}
              </button>
              <button type="button" className={quiet} onClick={() => setConfirming(false)}>
                {t("cancel")}
              </button>
            </div>
          </div>
        )}
      </Panel>
    </Centered>
  );
}

let storageAsked = false;

/** Ask once per page that the browser keep this site's storage (the device keys) out of eviction; what it answered goes to the console. */
function keepDeviceStorage(): void {
  if (storageAsked) return;
  storageAsked = true;
  void keepStorage().then((kept) => {
    if (kept === false) console.info("cloud: the browser did not grant persistent storage; it may still clear this site's storage under pressure");
  });
}

/** Exported so the screen can be previewed on its own. */
export function ErrorScreen({ message, held }: { message: string; held?: HeldSession }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("something went wrong")}>
        <Note tone="danger">
          <div data-signal="failed" className="font-mono text-2xs break-words text-danger">{message}</div>
        </Note>
        <div className="flex gap-2">
          <button type="button" data-variant="primary" className={primary} onClick={() => location.reload()}>
            {t("retry")}
          </button>
          <SignOutChoice held={held}>
            {(ask) => (
              <button type="button" className={quiet} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </div>
      </Panel>
    </Centered>
  );
}

/**
 * This device was removed from the account (the chain says so): forget its
 * keys (user keys, the cached keyring, its device keys) and every offline
 * copy and the key that sealed them, and stop talking to the hub as it.
 * Every clear is waited for; what failed comes back, to be shown.
 */
async function wipeRemoved(account: CloudAccount, link: HubLink | null, store: KeyStore): Promise<string[]> {
  account.stopSession();
  link?.stop();
  hubShell()?.stop();
  setHubShell(null);
  setRenewalRunner(null);
  clearSession();
  const failures = await wipeAccountHere(account, store);
  if (failures.length) console.warn("cloud: the removed device could not clear:", failures);
  return failures;
}

/** What could not be cleared, said plainly (a removed device's wipe, a sign-out). */
export function ClearFailures({ failures }: { failures: string[] | null | undefined }) {
  const { t } = useTranslation();
  if (!failures?.length) return null;
  return (
    <Note tone="danger">
      <span data-signal="failed" className="font-mono text-xs text-danger">{t("not everything could be cleared from this browser")}</span>
      <ul className="flex flex-col gap-0.5 font-mono text-2xs break-words text-muted">
        {failures.map((f, i) => (
          <li key={i}>{f}</li>
        ))}
      </ul>
      <p className="text-xs leading-5 text-muted">{t("Clear this site's data in the browser's settings to be sure.")}</p>
    </Note>
  );
}

/** Exported so the screen can be previewed on its own. */
export function RemovedScreen({ failures }: { failures?: string[] }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("this device was removed from the account")}>
        <p data-testid="device-removed" data-signal="removed" className="font-mono text-xs text-danger">
          {t("this device was removed from the account")}
        </p>
        <p className="text-xs leading-5 text-muted">
          {t("Its keys and its offline copies are gone from this browser. To use it again, sign in and add it as a new device from one that is still in the account.")}
        </p>
        <ClearFailures failures={failures} />
        <button
          type="button"
          data-variant="primary"
          className={primary}
          onClick={() => {
            clearSession();
            location.replace(SIGN_IN_PATH);
          }}
        >
          {t("sign in again")}
        </button>
      </Panel>
    </Centered>
  );
}

/**
 * The hub says this device's account was deleted (from another device of it,
 * or by the hub's operator): this browser forgot its keys and copies for it.
 * Exported so the screen can be previewed on its own.
 */
export function AccountDeletedScreen({ failures }: { failures?: string[] }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={t("this account was deleted")}>
        <p data-testid="account-gone" data-signal="removed" className="font-mono text-xs text-danger">
          {t("this account was deleted")}
        </p>
        <p className="text-xs leading-5 text-muted">
          {t("It no longer exists at Droppen, so this browser forgot its keys and its offline copies. To use Droppen again, sign in to another account or make a new one.")}
        </p>
        <ClearFailures failures={failures} />
        <button
          type="button"
          data-variant="primary"
          className={primary}
          onClick={() => {
            clearSession();
            location.replace(SIGN_IN_PATH);
          }}
        >
          {t("sign in")}
        </button>
      </Panel>
    </Centered>
  );
}

/**
 * The hub refuses this device, but the chain this browser could check does
 * not show it removed (the hub's word alone): nothing is wiped. Signing in
 * again with the password reads the chain, which decides.
 * Exported so the screen can be previewed on its own.
 */
export function RefusedScreen({ reason }: { reason: "removed" | "denied" }) {
  const { t } = useTranslation();
  return (
    <Centered>
      <Panel title={reason === "removed" ? t("the hub says this device was removed") : t("the hub refuses this device")}>
        <p data-testid="device-refused" data-signal="failed" className="font-mono text-xs text-danger">
          {reason === "removed" ? t("refused: removed, says the hub") : t("refused: no session for this device")}
        </p>
        <p className="text-xs leading-5 text-muted">
          {t("The account's device chain, as this browser could check it, does not show this device removed, so its keys stay here. Sign in again: the chain is read then, and decides. If it is removed there, its keys are wiped; if the hub keeps refusing it anyway, add it again from another device.")}
        </p>
        <div className="flex gap-2">
          <button
            type="button"
            data-variant="primary"
            className={primary}
            onClick={() => {
              clearSession();
              location.replace(SIGN_IN_PATH);
            }}
          >
            {t("sign in again")}
          </button>
          <button type="button" className={quiet} onClick={() => void signOut()}>
            {t("sign out")}
          </button>
        </div>
      </Panel>
    </Centered>
  );
}

/** The languages the verification mail is written in (the page's, else English). */
const MAIL_LANGS = ["en", "zh-CN", "zh-TW", "ja"];

/** How the form asks who you are: a passkey first, the password, or not known yet (the caller has no answer). */
export type AuthMethod = "passkey" | "password" | "pending";

/**
 * A passkey first where both this browser and the hub can (docs/passkeys.md
 * §7.1, §7.2). The password where either cannot, as the form always was
 * and with no word of passkeys, or where the person asked for it.
 */
export function authMethod(webauthn: boolean, config: SignupConfig | "failed" | null, wantPassword: boolean): AuthMethod {
  if (!webauthn || config === "failed") return "password";
  if (config === null) return "pending";
  if (!config.passkeys) return "password";
  return wantPassword ? "password" : "passkey";
}

/**
 * Sign in, or sign up: sign-up is open, as `SIGNUP_CONFIG` says, so an invite
 * code is optional — it may give the account a plan — and waits behind "have
 * an invite code?". No Turnstile widget is drawn. An account whose e-mail must
 * be verified first waits on "check your e-mail" before going on.
 *
 * Where passkeys are offered and the browser does WebAuthn, a passkey comes
 * first (docs/passkeys.md §3.4 A, §3.5): sign-up makes one, sign-in asks for
 * one, modally or from the e-mail field's autofill, and the password is a step
 * away. A passkey sign-in goes on exactly as a password one does.
 * Exported so the screen can be previewed on its own.
 */
export function AuthScreen({
  api,
  onSignedIn,
  initialMode = "signin",
  above,
}: {
  api: HubApi;
  onSignedIn: (session: HubSession) => void;
  initialMode?: "signin" | "signup";
  /**
   * Shown above the sign-in form (not sign-up's: a new account has nothing to
   * bring over): on the app host, the offer to bring this browser's device over
   * from the hub's origin.
   */
  above?: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const [mode, setModeState] = useState<"signin" | "signup">(initialMode);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState("");
  // Open sign-up: the person asked for the optional invite field (or the hub refused an invite).
  const [inviteShown, setInviteShown] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The browser's passkey prompt was closed: said, but not as an error.
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // What the last sign-out in this tab could not clear (read once).
  const [failures] = useState(() => takeSignOutFailures());
  // A browser just signed out and removed that the account still lists (read once).
  const [stillListed] = useState(() => takeStillListed());
  // This tab's account was just deleted, from its settings (read once).
  const [deleted] = useState(() => takeAccountDeleted());
  // Signed out by the pairing screen's "sign in again": it comes back once signed in.
  const [resumesPairing] = useState(() => pairingResume() !== null);
  // Signed in, the e-mail still to verify.
  const [waiting, setWaiting] = useState<{ session: SignedIn; address: string } | null>(null);
  // WebAuthn in this browser at all, and passkeys in the e-mail field's autofill.
  const [webauthn] = useState(() => webAuthnAvailable());
  const [autofillable, setAutofillable] = useState(false);
  // The person chose the password over a passkey.
  const [wantPassword, setWantPassword] = useState(false);
  const autofill = useRef<PasskeyAutofill | null>(null);

  useEffect(() => {
    if (!webauthn) return;
    let live = true;
    void probePasskeys().then((support) => live && setAutofillable(support.conditional));
    return () => {
      live = false;
    };
  }, [webauthn]);

  // On the sign-in / sign-up pages the address follows the tab (a deep link keeps its own).
  const setMode = (next: "signin" | "signup") => {
    setModeState(next);
    setNote(null);
    setWantPassword(false);
    if (isAuthPath(location.pathname)) goPublic(next === "signup" ? SIGN_UP_PATH : SIGN_IN_PATH, { replace: true });
  };

  // Sign-up's answers are constants (`SIGNUP_CONFIG`): open, an optional invite, no widget.
  const config = SIGNUP_CONFIG;
  const inviteOffered = true;
  const method = authMethod(webauthn, config, wantPassword);
  // Passkeys here at all: the e-mail field offers them, and the form offers both ways in.
  const passkeys = webauthn ? config.passkeys : null;
  const lang = MAIL_LANGS.includes(i18n.language) ? i18n.language : "en";

  /** Where a sign-in goes, however it was made: on, or "check your e-mail" first. */
  const signedIn = (session: SignedIn, address: string) => {
    if (session.verification === "required" && !session.emailVerified) return setWaiting({ session, address });
    onSignedIn(session);
  };

  /** A refusal in words (a closed passkey prompt as a note). */
  const failed = (err: unknown, context: "signup" | "login") => {
    if (passkeyCancelled(err)) {
      setNote(context === "signup" ? t("no passkey was created") : t("no passkey was used"));
    } else {
      // A refused invite opens the field: the code may be mistyped or used up.
      if (authErrorKind(err) === "badInvite") setInviteShown(true);
      setError(authErrorText(err, context));
    }
    setBusy(false);
  };

  /** A passkey sign-in, modal or from the autofill: `true` once signed in. */
  const withPasskey = async (run: () => Promise<PasskeySignedIn>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const session = await run();
      // Which passkey this tab holds: a new browser joins the account with it next, with one gesture less.
      if (session.credentialId) rememberPasskeySignIn({ account: session.account, credentialId: session.credentialId, rpId: passkeys?.rpId ?? null });
      setBusy(false);
      // A passkey names no e-mail on this side: "check your e-mail" shows the hub's.
      signedIn(session, "");
      return true;
    } catch (err) {
      failed(err, "login");
      return false;
    }
  };

  // The autofill's answers, through the newest render's handlers.
  const answer = (a: PasskeyAnswer) => withPasskey(() => finishPasskeyLogin(api, {}, a));
  const handlers = useRef({ answer });
  useEffect(() => {
    handlers.current = { answer };
  });

  // Signing in: the e-mail field's autofill offers this hub's passkeys (§3.5 step 1), asked afresh before each challenge expires.
  const autofillOn = mode === "signin" && passkeys !== null && autofillable && waiting === null;
  useEffect(() => {
    const credentials = autofillOn ? pageCredentials() : null;
    if (!credentials) return;
    const fill = new PasskeyAutofill({
      options: () => api.passkeyLoginOptions(),
      credentials,
      onAnswer: (answer) => handlers.current.answer(answer),
      onError: (err) => console.warn("cloud: passkey autofill stopped:", err),
    });
    autofill.current = fill;
    fill.start();
    return () => {
      fill.close();
      if (autofill.current === fill) autofill.current = null;
    };
  }, [api, autofillOn]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy || method === "pending") return;
    const address = email.trim();
    if (method === "passkey") {
      const credentials = pageCredentials();
      if (!credentials) return;
      if (mode === "signin") {
        // Modal, with no passkey named: the person picks one (the autofill's request withdrawn first).
        await withPasskey(() => passkeyLogin(api, { credentials }, { rpId: passkeys?.rpId, autofill: autofill.current }));
        return;
      }
      setBusy(true);
      setError(null);
      setNote(null);
      try {
        const session = await passkeySignUp(api, { credentials }, { email: address, invite: invite.trim() || undefined, lang, rpId: passkeys?.rpId });
        setBusy(false);
        signedIn(session, address);
      } catch (err) {
        failed(err, "signup");
      }
      return;
    }
    // Too short for the hub: said here, before a refusal.
    if (mode === "signup" && [...password].length < PASSWORD_MIN_CHARS) {
      setNote(null);
      setError(t("a password needs at least {{n}} characters", { n: PASSWORD_MIN_CHARS }));
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const session =
        mode === "signup"
          ? await api.signUp(address, password, { invite: invite.trim() || undefined, lang })
          : await api.login(address, password);
      setBusy(false);
      signedIn(session, address);
    } catch (err) {
      failed(err, mode === "signup" ? "signup" : "login");
    }
  };

  if (waiting) {
    return (
      <CheckEmailScreen
        api={api}
        session={waiting.session}
        address={waiting.address}
        onVerified={(session) => onSignedIn(session)}
        onBack={(message) => {
          setWaiting(null);
          setPassword("");
          if (!message) setEmail("");
          setError(message ?? null);
          setMode("signin");
        }}
      />
    );
  }

  const choose = (usePassword: boolean) => {
    setWantPassword(usePassword);
    setError(null);
    setNote(null);
  };
  // One centred column: the mark and the name, then the form, and a way back to the landing page;
  // language and theme at the right of the header, as on the landing page.
  return (
    <CloudPage aside={<PrefsControls />}>
      <div className="flex min-h-[calc(100dvh-3.5rem)] flex-col items-center justify-center px-6 py-14 max-md:px-4 max-md:py-10">
        <div className="flex w-full max-w-[22rem] flex-col">
          <div className="mb-8 flex flex-col items-center gap-3">
            <BrandMark size={36} />
            <h1 className="font-mono text-lg leading-6 font-medium text-fg">Droppen</h1>
          </div>
          {deleted && (
            <div className="mb-6">
              <Note tone="ok">
                <p data-testid="account-deleted" className="text-xs leading-5 font-medium text-fg">
                  {t("Your account was deleted.")}
                </p>
                <p className="text-xs leading-5 text-muted">{t("What it kept at Droppen is erased for good, and this browser no longer holds its keys.")}</p>
              </Note>
            </div>
          )}
          {failures && (
            <div className="mb-6">
              <ClearFailures failures={failures} />
            </div>
          )}
          {stillListed && (
            <div className="mb-6">
              <StillListedNote why={stillListed} />
            </div>
          )}
          {resumesPairing && (
            <div className="mb-6">
              <Note tone="warn">
                <p data-testid="signin-to-pair" className="text-xs leading-5 text-muted">
                  {t("Sign in again to go on adding this browser: you come back to where you were.")}
                </p>
              </Note>
            </div>
          )}
          {above && mode === "signin" && (
            <div data-testid="auth-above" className="mb-6">
              {above}
            </div>
          )}
          <form onSubmit={submit} className="flex flex-col gap-3">
            <Tabs
              value={mode}
              onChange={setMode}
              options={[
                { id: "signin", label: t("sign in") },
                { id: "signup", label: t("sign up") },
              ]}
            />
            {mode === "signup" && inviteShown && (
              <Field label={t("invite code (optional)")}>
                <input
                  data-testid="auth-invite"
                  value={invite}
                  onChange={(e) => setInvite(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  // Asked for: the person just opened it.
                  autoFocus
                  className={field}
                />
              </Field>
            )}
            <Field label={t("e-mail")}>
              <input
                data-testid="auth-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                // Signing in, this hub's passkeys show in the field's autofill too.
                autoComplete={mode === "signin" && passkeys ? "username webauthn" : "username"}
                autoFocus
                className={field}
              />
            </Field>
            {method === "password" && (
              <>
                <Field label={t("password")}>
                  <input
                    data-testid="auth-password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoComplete={mode === "signup" ? "new-password" : "current-password"}
                    className={field}
                  />
                </Field>
                {mode === "signup" && <p className="text-xs text-faint">{t("At least 10 characters. Your password signs you in; it cannot decrypt anything.")}</p>}
              </>
            )}
            {method === "passkey" && mode === "signup" && (
              <p className="text-xs text-faint">{t("A passkey signs you in with your device's screen lock; it cannot decrypt anything.")}</p>
            )}
            {mode === "signup" && <SignUpTerms />}
            <ErrorLine message={error} />
            {note && (
              <p data-testid="passkey-cancelled" className="font-mono text-2xs text-muted">
                {note}
              </p>
            )}
            {method === "passkey" ? (
              mode === "signup" ? (
                <button
                  type="submit"
                  data-testid="passkey-signup"
                  data-variant="primary"
                  disabled={busy || !email.trim()}
                  className={primary}
                >
                  {busy && <Loader2 className="size-3.5 animate-spin" />}
                  {t("create a passkey and sign up")}
                </button>
              ) : (
                <button type="submit" data-testid="passkey-signin" data-variant="primary" disabled={busy} className={primary}>
                  {busy && <Loader2 className="size-3.5 animate-spin" />}
                  {t("sign in with a passkey")}
                </button>
              )
            ) : (
              <button
                type="submit"
                data-testid={method === "password" ? "password-submit" : "auth-pending"}
                data-variant="primary"
                disabled={method === "pending" || busy || !email.trim() || !password}
                className={primary}
              >
                {(busy || method === "pending") && <Loader2 className="size-3.5 animate-spin" />}
                {mode === "signup" ? t("create account") : t("sign in")}
              </button>
            )}
            {mode === "signup" && inviteOffered && !inviteShown && (
              <button type="button" data-testid="show-invite" className={quiet} onClick={() => setInviteShown(true)}>
                {t("have an invite code?")}
              </button>
            )}
            {passkeys && (
              method === "passkey" ? (
                <button type="button" data-testid="use-password" className={quiet} onClick={() => choose(true)}>
                  {mode === "signup" ? t("sign up with a password instead") : t("sign in with a password")}
                </button>
              ) : method === "password" ? (
                <button type="button" data-testid="use-passkey" className={quiet} onClick={() => choose(false)}>
                  {mode === "signup" ? t("sign up with a passkey instead") : t("sign in with a passkey instead")}
                </button>
              ) : null
            )}
          </form>
          <a {...publicLink("/")} className="mt-6 self-center text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
            {t("What is Droppen?")}
          </a>
        </div>
      </div>
    </CloudPage>
  );
}

/**
 * What signing up agrees to, said above the button: the terms of service and
 * the privacy notice (on the site host where the hub has one: `publicHref`),
 * each opening in a new tab so the form keeps what was typed (and the human
 * check its token).
 */
function SignUpTerms() {
  const { t } = useTranslation();
  const link = "text-muted underline underline-offset-4 hover:text-fg";
  return (
    <p data-testid="signup-terms" className="text-xs leading-5 text-faint">
      <Trans
        t={t}
        i18nKey="By signing up you agree to the <terms>Terms of Service</terms> and have read the <privacy>Privacy notice</privacy>."
        components={{
          terms: <a href={publicHref(TERMS_PATH)} target="_blank" rel="noopener" data-testid="signup-terms-link" className={link} />,
          privacy: <a href={publicHref(PRIVACY_PATH)} target="_blank" rel="noopener" data-testid="signup-privacy-link" className={link} />,
        }}
      />
    </p>
  );
}

/** Exported so the screen can be previewed on its own. */
export function GenesisScreen({ account, onDone }: { account: CloudAccount; onDone: (words: string[] | null) => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"with" | "without" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const create = async (withRecovery: boolean) => {
    setBusy(withRecovery ? "with" : "without");
    setError(null);
    try {
      onDone(await account.createGenesis(withRecovery));
    } catch (err) {
      setError(errorText(err));
      setBusy(null);
    }
  };
  return (
    <Centered>
      <Panel title={t("set up this device")}>
        <p className="text-xs leading-5 text-muted">
          {t("This browser now holds this account's device keys. They stay on this device and are never sent to our servers: everything you see here is decrypted on this device.")}
        </p>
        <p className="text-xs leading-5 text-muted">
          {t("Twelve recovery words let you get back in if you lose every device. They are shown once, and nobody — not the hub, not us — can recover them for you.")}
        </p>
        <ErrorLine message={error} />
        <button type="button" data-variant="primary" disabled={busy !== null} onClick={() => void create(true)} className={primary}>
          {busy === "with" && <Loader2 className="size-3.5 animate-spin" />}
          {t("create keys with recovery words")}
        </button>
        <button type="button" disabled={busy !== null} onClick={() => void create(false)} className={quiet}>
          {busy === "without" && <Loader2 className="size-3.5 animate-spin" />}
          {t("skip the recovery words")}
        </button>
      </Panel>
    </Centered>
  );
}

/**
 * 「我的设备」, or the app on the machine in view: which is the shell's
 * focus, and changing it changes this in place. Opening a machine from
 * 「我的设备」 goes back to the app's address — the same section of it for a
 * machine other than the one the app last showed (`HubShell.select`) — and
 * a session or a terminal picked on the dashboard opens on its own machine
 * (`HubShell.openAt`).
 */
function ShellView({ shell, origin, children }: { shell: HubShell; origin: string; children: ReactNode }) {
  const focus = useStore(shell.focus);
  const pathname = usePathname();
  if (focus !== null) return <>{children}</>;
  // The account's settings: the other page with no device in view (`devices-route.ts`).
  if (isAccountPath(pathname)) return <AccountScreen account={shell.account} link={shell.link} />;
  return <MyDevicesScreen account={shell.account} link={shell.link} origin={origin} onSelect={(id) => shell.select(id)} onOpenAt={(id, path) => shell.openAt(id, path)} />;
}

