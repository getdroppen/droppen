import { useEffect, useState } from "react";
import type { HubApi, SignupMode } from "@/lib/hub/api";

/*
 * Who may sign up, as the public pages word it (`GET /v1/signup/config`,
 * docs/hub-api/http.md). droppen.app's sign-up is open, so the pages draw
 * the open wording at once and switch only once the hub says sign-up is by
 * invite (the invite code, and Free's waitlist) or closed ("closed for
 * now", and Free's waitlist): droppen.app never flickers, and another hub
 * still says what it does a moment later. A hub that cannot be asked keeps
 * the open wording; its sign-up form says the rest.
 *
 * Asked once per hub client and kept for the page's life: moving between
 * the public pages asks nothing again.
 */

type Asker = Pick<HubApi, "signupConfig">;

const asked = new WeakMap<Asker, Promise<SignupMode>>();
const known = new WeakMap<Asker, SignupMode>();

/** The hub's sign-up mode, asked once per `api` (`open` when it cannot be asked). */
export function signupModeOf(api: Asker): Promise<SignupMode> {
  let mode = asked.get(api);
  if (!mode) {
    mode = api.signupConfig().then(
      (config) => config.signup,
      (): SignupMode => "open",
    );
    void mode.then((m) => known.set(api, m));
    asked.set(api, mode);
  }
  return mode;
}

/**
 * The sign-up mode for a public page: `given` when the caller knows it
 * (tests, previews), else the hub's once it answers — `open` until then, and
 * with no `api` at all.
 */
export function useSignupMode(api: Asker | undefined, given?: SignupMode): SignupMode {
  const [mode, setMode] = useState<SignupMode>(() => (api && known.get(api)) || "open");
  useEffect(() => {
    if (given || !api) return;
    let live = true;
    void signupModeOf(api).then((m) => live && setMode(m));
    return () => {
      live = false;
    };
  }, [api, given]);
  return given ?? mode;
}
