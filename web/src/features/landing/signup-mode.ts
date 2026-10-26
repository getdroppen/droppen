import type { SignupMode } from "@/lib/hub/api";

/*
 * Who may sign up, as the public pages word it. Sign-up is open: droppen.app's
 * answers are written in `SIGNUP_CONFIG` (`lib/hub/api.ts`), and nothing asks
 * the hub any more. Every page draws the open wording; `given` lets a test or
 * a preview say otherwise.
 */

/** The sign-up mode: open, as [`SIGNUP_CONFIG`](@/lib/hub/api) says. */
export function signupModeOf(): SignupMode {
  return "open";
}

/**
 * The sign-up mode for a public page: `given` when the caller knows it
 * (tests, previews), else open.
 */
export function useSignupMode(given?: SignupMode): SignupMode {
  return given ?? "open";
}
