/*
 * The hand-off from the guide to the empty-session view: the one place that
 * knows how the guide opens that view with step 03's folder, tool and first
 * message — through the
 * view's own prefill entry point (`useOpenNewSession`, the
 * `?cwd=…&draft=…&agent=…` it takes into its draft and drops from the
 * address).
 *
 * Nothing is sent: the view opens with the message typed in, and only the
 * user's Send starts the session. The permission is the view's own — the
 * device's stored default, shown in its row — and is not set from here.
 */
import { useCallback } from "react";
import { useOpenNewSession } from "@/features/sessions/new-session-view";
import { useMachine } from "@/lib/machines/context";
import { LANDED } from "./first-model";
import { opensFirstSession } from "./model";
import { showSetupToast } from "./toast";
import type { SetupProgress, ToolId } from "./types";

export { LANDED };

/** What step 03 picked, as the view takes it. */
export interface FirstSession {
  folder: string;
  agent: ToolId;
  text: string;
}

/** How long the landing note stays: long enough to read it with the view in front of you. */
export const LANDED_MS = 15_000;

/** The first session step 03 saved, if it saved one (done, with a folder and a tool). */
export function firstSessionOf(progress: SetupProgress): FirstSession | null {
  const { cwd, agent, prompt } = progress.first;
  if (!opensFirstSession(progress) || !cwd || !agent) return null;
  return { folder: cwd, agent, text: prompt ?? "" };
}

/**
 * Open the empty-session view on this view's device (the app's own) with
 * `first` filled in, and show the landing note once over it.
 */
export function useOpenFirstSession(): (first: FirstSession) => void {
  const machine = useMachine();
  const open = useOpenNewSession();
  return useCallback(
    (first: FirstSession) => {
      open({ machine, cwd: first.folder, draft: first.text, source: first.agent });
      showSetupToast(LANDED, undefined, LANDED_MS);
    },
    [machine, open],
  );
}
