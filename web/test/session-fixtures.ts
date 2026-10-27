// The sessions view's models, as the tests make them: a resident with
// nothing running, a transcript item of the common shape, a background task.
// Each factory is the default object the suite used to re-declare, with the
// fields a test varies passed in.
import type { BackgroundTask, ResidentInfo } from "../src/features/sessions/session-activity.js";
import type { TranscriptItem } from "../src/features/sessions/session-feed-state.js";

export function resident(over: Partial<ResidentInfo> = {}): ResidentInfo {
  return {
    phase: "idle",
    queued: 0,
    queuedMessages: [],
    background: 0,
    tasks: [],
    awaitingBackground: false,
    stalled: false,
    turnStartedAt: null,
    ...over,
  };
}

export function transcriptItem(over: Partial<TranscriptItem> = {}): TranscriptItem {
  return {
    detail: null,
    isError: false,
    kind: "text",
    meta: null,
    question: null,
    role: "assistant",
    text: "",
    timestamp: null,
    toolName: null,
    truncated: false,
    ...over,
  };
}

export function backgroundTask(over: Partial<BackgroundTask> & Pick<BackgroundTask, "id">): BackgroundTask {
  return { toolUseId: null, description: null, taskType: null, startedAt: null, ...over };
}
