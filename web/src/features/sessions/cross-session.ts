/*
 * The two kinds of user-role text Droppen itself puts in a transcript.
 *
 * Both arrive looking like something the person typed — one as a `[Droppen]`
 * sentence, the other inside a `<cross-session-message …>` envelope — and both
 * read wrong under a USER head with the envelope on show.
 */

/** A message another session sent this one, taken out of its envelope. */
export interface CrossSessionMessage {
  /** The sending session's name, when the envelope named one. */
  from: string | null;
  /** The sending session's own text. */
  body: string;
}

const OPEN = /^<cross-session-message\b([^>]*)>\s*/;
const CLOSE = /<\/cross-session-message>\s*$/;

/**
 * `text` as a cross-session message, or `null` for every other text.
 *
 * The envelope is not Droppen's own — it is written by whatever session is
 * sending — so this reads the attributes it needs by name and keeps the body
 * whole rather than depending on their order or on the rest being present.
 */
export function crossSessionOf(text: string): CrossSessionMessage | null {
  const open = OPEN.exec(text);
  if (!open) return null;
  const name = /\bfrom-name="([^"]*)"/.exec(open[1] ?? "")?.[1]?.trim();
  const body = text.slice(open[0].length).replace(CLOSE, "").trim();
  return { from: name ? name : null, body };
}

/** The marker Droppen prefixes its own notices with. */
const NOTICE = /^\[Droppen\]\s+/;

/**
 * The sentence of a notice Droppen wrote into the session in its own name
 * (an account rotation, a recovered quota), or `null`.
 */
export function droppenNoticeOf(text: string): string | null {
  const match = NOTICE.exec(text);
  if (!match) return null;
  return text.slice(match[0].length);
}
