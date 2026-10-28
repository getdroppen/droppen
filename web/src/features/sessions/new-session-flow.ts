export type SessionSource = "claude" | "codex" | "acp";

/** The CLIs an instant chat runs on; the ACP agent has no chats. */
export type ChatSource = "claude" | "codex";

/** Whether `value` names an agent a session can run on. */
export function isSessionSource(value: unknown): value is SessionSource {
  return value === "claude" || value === "codex" || value === "acp";
}

/** The agent a new session runs on: a CLI, or one configured ACP agent by id. */
export interface SessionAgent {
  source: SessionSource;
  /** The ACP agent's id when `source` is `acp`; `null` is none picked yet. */
  acpAgent: string | null;
}

/** An ACP agent id as the server takes it: lowercase letters, digits and `-`. */
const ACP_AGENT_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;

/** How the agent picker and an address name an agent: `claude`, `codex`, or `acp:<id>`. */
export function agentKey(agent: SessionAgent): string {
  return agent.source === "acp" && agent.acpAgent ? `acp:${agent.acpAgent}` : agent.source;
}

/** The agent `value` names ({@link agentKey}), or `null` when it names none. */
export function parseAgentKey(value: unknown): SessionAgent | null {
  if (typeof value !== "string") return null;
  if (isSessionSource(value)) return { source: value, acpAgent: null };
  const id = value.startsWith("acp:") ? value.slice(4) : null;
  return id !== null && ACP_AGENT_ID.test(id) ? { source: "acp", acpAgent: id } : null;
}

export type NewConversation = {
  text: string;
  model: string;
  effort?: string | null;
} & (
  | {
      kind: "session"; source: SessionSource; cwd: string; permission: string; accountId?: string | null;
      /** The ACP agent an `acp` session starts on, by id. */
      acpAgent?: string | null;
      /** `"start"`: `accountId` is the one the start rule named (`placed: false`), not a pick. */
      accountFrom?: "start";
      /** Paths `POST /api/uploads` answered, sent with the first message as a later message sends them. */
      images?: string[];
      files?: string[];
    }
  | {
      kind: "chat"; source: ChatSource | "api"; providerReady: boolean;
      /** A CLI chat's login, as for a session: absent follows the default, `null` is the local login. */
      accountId?: string | null;
      /** `"start"`: `accountId` is the one the start rule named (`placed: false`), not a pick. */
      accountFrom?: "start";
    }
);

export function canCreateConversation(input: NewConversation, busy: boolean): boolean {
  if (busy || !input.text.trim()) return false;
  return input.kind === "session"
    ? !!input.cwd.trim() && (input.source !== "acp" || !!input.acpAgent)
    : input.source !== "api" || (input.providerReady && !!input.model);
}

/**
 * Keep omitted effort/account distinct from an explicit local login.
 *
 * Both CLIs now carry an account: `accountId` absent means "follow this CLI's
 * stored default", `null` means the user asked for the local login by hand.
 *
 * An empty model or permission is left out: an ACP agent may choose both itself, and
 * refuses a start that names either.
 */
export function creationRequest(input: NewConversation, clientMessageId?: string) {
  const common = { text: input.text.trim(), ...(input.model ? { model: input.model } : {}), ...(input.effort ? { effort: input.effort } : {}) };
  return input.kind === "session"
    ? { path: "/api/sessions/new" as const, body: {
        ...common, source: input.source, cwd: input.cwd.trim(), ...(input.permission ? { permission: input.permission } : {}),
        ...(input.source === "acp" && input.acpAgent ? { acpAgent: input.acpAgent } : {}),
        ...(input.accountId !== undefined ? { accountId: input.accountId } : {}),
        ...(input.accountFrom ? { accountFrom: input.accountFrom } : {}),
        // Omitted when empty, so a start with nothing attached is the body an
        // older server always took.
        ...(input.images?.length ? { images: input.images } : {}),
        ...(input.files?.length ? { files: input.files } : {}),
        ...(clientMessageId ? { clientMessageId } : {}),
      } }
    : { path: "/api/chats/new" as const, body: input.source === "api"
        ? { backend: input.source, ...(input.model ? { model: input.model } : {}) }
        : { ...common, backend: input.source,
            ...(input.accountId !== undefined ? { accountId: input.accountId } : {}),
            ...(input.accountFrom ? { accountFrom: input.accountFrom } : {}) } };
}

export interface PendingCreatedSession {
  source: SessionSource;
  id: string;
}

interface SessionRef {
  source: SessionSource;
  id: string;
}

export function resolveCreatedSession(
  pending: PendingCreatedSession | null,
  sessions: readonly SessionRef[] | null,
): PendingCreatedSession | null {
  if (!pending || !sessions) return null;
  return sessions.some((session) => session.source === pending.source && session.id === pending.id)
    ? pending
    : null;
}

/** The id a start attempt goes under, and the request it was minted for. */
export interface CreationAttempt {
  key: string;
  clientMessageId: string;
}

/**
 * The `clientMessageId` for starting this session, reused by every retry of it.
 *
 * A start whose answer was lost is retried by pressing the button again, and
 * the server answers a repeated id with the session it already started rather
 * than a second one. The id only holds while the request is the same request:
 * an edited prompt, directory or setting is a different session, and gets a
 * new one.
 */
export function creationAttempt(
  previous: CreationAttempt | null,
  input: NewConversation,
  mint: () => string,
): CreationAttempt {
  const key = JSON.stringify(creationRequest(input).body);
  return previous?.key === key ? previous : { key, clientMessageId: mint() };
}
