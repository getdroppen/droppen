/*
 * The account's waits and caps (`CloudAccount`): when its session is
 * renewed, how often it asks for keys again, and the most it mails or
 * appends at once.
 */

/** Renew the device session this long before it expires. */
export const RENEW_MARGIN_MS = 5 * 60 * 1000;
/** A session minted on a 401 and refused again within this long: the hub will not have this device. */
export const SECOND_REFUSAL_MS = 60 * 1000;
export const RENEW_RETRY_MS = 30 * 1000;
/** How often a device missing the newest user key asks the others for it again. */
export const KEY_REQUEST_EVERY_MS = 10 * 60 * 1000;
/**
 * The most host keys one answer seals for one device (newest epochs first):
 * under the hub's share of one sender in one mailbox (128), with room left
 * for the other mail.
 */
export const MAX_HOST_KEY_MAILS = 96;
/**
 * How long a new host's registration waits for its device role to sign in at
 * the hub (it does within a second of seeing itself in the chain) before the
 * other hosts' keys are sealed to it (S1-4); past it, the host asks for them
 * itself (purpose 4).
 */
export const HOST_KEYS_WAIT = { forMs: 20_000, everyMs: 1_000 };
/**
 * How long an enrollment waits for the new host's agent to declare `chain/3`
 * (its proof can land a moment before its declaration): a 409
 * `chain_v3_not_ready` naming only that host is tried again for this long.
 */
export const CHAIN_V3_WAIT = { forMs: 5000, everyMs: 500 };
/** The most entries the hub takes in one append (`POST /v1/chain`, docs/hub-api/http.md): a bigger batch is refused here, before anything is signed. */
export const MAX_APPEND_ENTRIES = 64;
/** How many times a page's first load fetches the chain before a record it cannot reach is a fork (other pages may record more meanwhile). */
export const FIRST_LOAD_TRIES = 5;
