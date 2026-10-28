import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ListChecks, TriangleAlert } from "lucide-react";
import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { glyphValue, OptionLabel, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, useLatest, useStableOptions } from "@/components/ui/select";
import type { Glyph } from "@/components/glyph";
import { activeRests, restChipText, restTitle, restTone } from "@/features/accounts/account-rest";
import { QuotaBlock, QuotaGauges } from "@/features/accounts/quota-bits";
import {
  accountKindLabel,
  accountUsageState,
  quotaAge,
  quotaDoubt,
  useAccounts,
  useQuota,
  useStartAccount,
  type AccountQuota,
} from "@/features/accounts/use-accounts";
import { cn } from "@/lib/cn";
import { formatInt } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { readMachineItem } from "@/lib/machines/storage";

type ModelInfo = components["schemas"]["ModelInfo"];
type ComposerChoice = components["schemas"]["ComposerChoice"];
type ComposerPrefs = components["schemas"]["ComposerPrefs"];
type CliChoice = components["schemas"]["CliChoice"];
type Source = components["schemas"]["Source"];

/**
 * The agents whose model, effort, permission and account a composer chooses.
 * An ACP agent chooses all four from its own configuration, so its composer offers
 * none of them and sends none of them.
 */
export type CliSource = "claude" | "codex";

/** What the composer holds while an agent's model list is unknown: nothing to send. */
const NO_MODEL: ModelInfo = { id: "", label: null, efforts: [], defaultEffort: null };

/** Every effort each CLI accepts, for models that publish no list of their own. */
export const EFFORT_FALLBACK: Record<"claude" | "codex", string[]> = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["low", "medium", "high", "xhigh"],
};

/**
 * What the pickers offer when `/api/models` cannot be reached at all: the tier
 * aliases, which are what the CLIs themselves resolve to the newest model.
 */
export const MODEL_FALLBACK: Record<"claude" | "codex", ModelInfo[]> = {
  claude: ["fable", "opus", "sonnet", "haiku"].map((id) => ({
    id,
    label: `${id} (latest)`,
    // The haiku tier takes no `--effort` at all, which is what the server's own
    // list says about it. An offline list that disagreed would offer a rung the
    // CLI refuses, and only while the CLI is unreachable — the worst moment to
    // build a message that cannot be sent.
    efforts: id === "haiku" ? [] : EFFORT_FALLBACK.claude,
    defaultEffort: id === "haiku" ? null : "medium",
  })),
  codex: [{ id: "gpt-5.5", label: null, efforts: EFFORT_FALLBACK.codex, defaultEffort: "medium" }],
};

/** claude --permission-mode values / codex exec permission modes. */
export const PERMISSION_OPTIONS: Record<"claude" | "codex", string[]> = {
  claude: ["acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"],
  codex: ["auto", "read-only", "full-access", "bypass-all"],
};

export const PERMISSION_DEFAULT: Record<"claude" | "codex", string> = {
  claude: "acceptEdits",
  codex: "auto",
};

/**
 * Server-stored UI prefs (settings page + composer shortcut), shared across
 * devices. The pre-sync localStorage value bridges until the query resolves.
 */
export function useEnterSendsModifier(): boolean {
  const api = useApi();
  const machine = useMachine();
  const { data } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    staleTime: 30_000,
  });
  if (data) return data.composerSend === "modifier";
  return readMachineItem(localStorage, machine, "composer.enterToSend") === "modifier";
}

/**
 * The listed entry that means `id`, when `id` is not itself one of them.
 *
 * Two steps, because the server's list now names a model twice: the id is the
 * alias the CLI is asked for and the label is the concrete model that alias
 * currently resolves to. A transcript reports the concrete one, so the label is
 * matched first — that is the same row, and offering the id as an entry of its
 * own would put two options reading "claude-opus-5[1m]" in one menu. Failing
 * that it falls back to the tier ("claude-sonnet-5" → "sonnet"), whose own id
 * may carry a variant tag, which is why "opus" also answers for "opus[1m]".
 */
function claudeAliasFor(models: ModelInfo[], id: string): ModelInfo | undefined {
  const resolved = models.find((m) => m.label === id);
  if (resolved) return resolved;
  const alias = /^claude-(fable|mythos|opus|sonnet|haiku)\b/.exec(id)?.[1];
  if (!alias) return undefined;
  return models.find((m) => m.id === alias) ?? models.find((m) => m.id.startsWith(`${alias}[`));
}

/** `MODEL_PATTERN` in `drive/composer_prefs.rs`, and the same 100-char bound. */
const SENDABLE_MODEL = /^[A-Za-z0-9][\w.:-]*(?:\[[A-Za-z0-9]+\])?$/;

/**
 * Would the send routes accept this as a `--model`?
 *
 * Not every id the composer reads is one it may offer. The transcript is one of
 * the sources — and Claude Code writes `<synthetic>` as the model of an entry it
 * composed in the model's own voice, a rate-limit wall or a "Not logged in", so
 * that is exactly what the last turn reports on the sessions a user is most
 * likely to open. Showing it would put a value in the picker that the next send
 * is refused for, which is a composer that cannot send at all.
 */
function isSendable(id: string): boolean {
  return id.length <= 100 && SENDABLE_MODEL.test(id);
}

/**
 * The entry the model picker shows as current, plus the list it offers.
 *
 * A transcript reports concrete ids ("claude-opus-5") while the picker may only
 * list the tier alias, so an unlisted claude id resolves to its tier. Anything
 * still unmatched is offered as itself, appended to the list: a model we cannot
 * name is displayed as its own id, never as another model's — a composer
 * reading "fable" over a session running something else is a lie, and the id it
 * would then send with the next turn would silently switch the model.
 *
 * The one thing not offered as itself is a string that is not a model id at all
 * (see [`isSendable`]), because the id shown here is also the id sent.
 */
export function resolveModel(
  models: ModelInfo[],
  wanted: string | undefined,
  source: "claude" | "codex",
): { options: ModelInfo[]; active: ModelInfo; custom: boolean } {
  const exact = wanted ? models.find((m) => m.id === wanted) : undefined;
  if (exact) return { options: models, active: exact, custom: false };
  const alias = source === "claude" && wanted ? claudeAliasFor(models, wanted) : undefined;
  if (alias) return { options: models, active: alias, custom: false };
  // Nothing is wanted yet (a brand new session): the freshest model leads. An
  // id the send routes would refuse resolves the same way — naming the list's
  // head is a smaller lie than offering a model that cannot be sent.
  if (!wanted || !isSendable(wanted)) return { options: models, active: models[0]!, custom: false };
  const unknown: ModelInfo = { id: wanted, label: null, efforts: null, defaultEffort: null };
  return { options: [...models, unknown], active: unknown, custom: true };
}

/** What a composer asks [`resolveModel`] for: what is stored, unless it cannot be sent and the server named a model to start on. */
export function startingModel(stored: string | undefined, startModel: string | null | undefined): string | undefined {
  return stored && isSendable(stored) ? stored : (startModel ?? stored);
}

/**
 * The select value that opens the "type a model id" box rather than picking a
 * model. Two underscores each side because it shares a namespace with real
 * ids, and `__custom__` is not one the send routes would even accept.
 */
export const CUSTOM_MODEL = "__custom__";

/**
 * The id a typed custom model commits as, or null to leave the previous pick
 * alone.
 *
 * Empty is a cancel — backspacing the box clear is how someone changes their
 * mind — and so is anything the send routes would refuse (see [`isSendable`]):
 * committing it would store a model every later send is rejected for, which is
 * worse than the box having done nothing.
 */
export function commitCustomModel(raw: string): string | null {
  const id = raw.trim();
  return id !== "" && isSendable(id) ? id : null;
}

/**
 * What a model select offers: the list, the model in use when it is not on the
 * list, and the entry that opens the text box.
 *
 * `customId` is the id to mark as typed-in rather than published. It is
 * appended when the list does not already carry it — the composer's list does
 * (see [`resolveModel`]), a form holding a bare string does not — so the
 * current model is always one of the options and the select is never blank.
 */
export function modelOptions(
  models: ModelInfo[],
  customId: string | null,
  text: { custom: string; marker: string },
): { value: string; label: string }[] {
  const mark = (id: string, label: string) =>
    id === customId ? `${label} · ${text.marker}` : label;
  const items = models.map((m) => ({ value: m.id, label: mark(m.id, m.label ?? m.id) }));
  if (customId && !models.some((m) => m.id === customId)) {
    items.push({ value: customId, label: `${customId} · ${text.marker}` });
  }
  items.push({ value: CUSTOM_MODEL, label: text.custom });
  return items;
}

/**
 * The efforts a model accepts.
 *
 * `null` and `[]` are different answers and the difference is the whole point:
 * `null` is "this list says nothing about efforts" (a codex entry, or a model we
 * invented an entry for), which falls back to the CLI's whole ladder; `[]` is
 * the list stating that the model takes no `--effort` at all — claude-haiku and
 * claude-sonnet-4-5 refuse the flag outright. Treating the empty list as "said
 * nothing" is how the picker ends up offering five rungs for a model that
 * accepts none, which is exactly what the server-side ladder exists to prevent.
 */
export function effortsOf(model: ModelInfo | undefined, source: "claude" | "codex"): string[] {
  return model?.efforts ?? EFFORT_FALLBACK[source];
}

/**
 * The effort to run at when nothing usable is stored: what the model itself
 * names, else the middle of the ladder — which "medium" is on both CLIs, and
 * stays so however many rungs a future ladder grows.
 *
 * `null` when the model has no ladder: there is no rung to pick, and inventing
 * "medium" for it would send a flag the model refuses.
 */
export function defaultEffortOf(model: ModelInfo | undefined, options: string[]): string | null {
  if (options.length === 0) return null;
  const named = model?.defaultEffort;
  // A named default outside the ladder describes a flag that would be refused.
  if (named && options.includes(named)) return named;
  return options.includes("medium") ? "medium" : options[Math.floor(options.length / 2)]!;
}

/** Ids a drift verdict can be built from; `<synthetic>` and friends are not. */
const MODEL_ID = /^[a-z0-9][a-z0-9.:_-]*$/;

/**
 * A model id reduced to the segments that name it: `claude-opus-4-8[1m]` →
 * `["opus", "4", "8"]`. Null for anything that is not an id at all.
 *
 * Both `-` and `.` split, because the two CLIs disagree about which separates a
 * version: `claude-opus-5` and `gpt-5.5` name their generation the same way.
 */
function modelCore(id: string | null | undefined): string[] | null {
  if (!id) return null;
  // `claude --model claude-opus-4-8[1m]` asks for the long-context variant of a
  // model the transcript then reports without the marker: same model, and a
  // suffix that only ever describes the window is not a drift.
  let core = id.trim().toLowerCase().replace(/\[1m\]$/, "");
  // Bedrock/Vertex spell the same model with a routing prefix in front of it.
  const family = core.lastIndexOf("claude-");
  if (family >= 0) core = core.slice(family + "claude-".length);
  if (!MODEL_ID.test(core)) return null;
  const parts = core.split(/[-.]/).filter(Boolean);
  return parts.length > 0 ? parts : null;
}

/**
 * Is the session running something other than what was asked for?
 *
 * The badge this drives is an accusation — "the CLI quietly gave you a smaller
 * model" — so it is built to under-report rather than over-report. Anything it
 * cannot adjudicate is not a drift: a missing request, a missing reading, and
 * ids that are not ids (`<synthetic>`, which Claude Code writes as the model of
 * an entry it composed itself) all return false.
 *
 * Two ids are the same model when one's segments are a prefix of the other's,
 * because that is what asking for a tier means: `opus` is answered by
 * `claude-opus-5`, and `claude-opus-5` by `claude-opus-5-20260114`. A bare tier
 * word is matched anywhere in the other id rather than only at the front, so the
 * older `claude-3-5-sonnet-20241022` spelling still answers `sonnet`. What is
 * left is two ids that name different models at some segment they both have —
 * `opus-5` against `opus-4-8` — and that is a drift.
 */
export function modelDrift(
  requested: string | null | undefined,
  observed: string | null | undefined,
): boolean {
  const want = modelCore(requested);
  const got = modelCore(observed);
  if (!want || !got) return false;
  if (want.length === 1) return !got.includes(want[0]!);
  if (got.length === 1) return !want.includes(got[0]!);
  const [short, long] = want.length <= got.length ? [want, got] : [got, want];
  return !short.every((part, i) => long[i] === part);
}

/**
 * The stored request to judge a transcript reading against, or undefined when
 * the two cannot be ordered.
 *
 * A pick made after the last turn ran is intent, not a broken promise: the
 * window between changing the dropdown and sending the next message is exactly
 * when the session is legitimately running something else, and lighting a
 * warning there would make the badge fire on every model change. Same for a
 * reading with no timestamp — nothing can be said about which came first.
 */
export function judgeableRequest(
  requested: string | undefined,
  requestedAt: string | undefined,
  observedAt: string | null | undefined,
): string | undefined {
  if (!requested || !observedAt) return undefined;
  if (requestedAt && requestedAt >= observedAt) return undefined;
  return requested;
}

/**
 * The model a new composer of this CLI has stored: the synced default, else
 * this device's older pick, else none — what [`useComposerChoice`] starts
 * from before any login's list is in.
 */
export function useComposerDefaultModel(source: "claude" | "codex"): string | null {
  const api = useApi();
  const machine = useMachine();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "composer-prefs"),
    queryFn: async () => (await api.GET("/api/composer-prefs")).data,
    staleTime: 5 * 60_000,
  });
  return prefs?.defaults[source]?.model ?? readMachineItem(localStorage, machine, `composer.model.${source}`) ?? null;
}

/**
 * Where a new composer's start runs when its account is left to the server,
 * and — with nothing (sendable) stored — the model it should send: the server
 * chooses the login and the model together (`useStartAccount` with
 * `chooseModel`), and that model goes to [`useComposerChoice`] as `startModel`.
 * With a model stored, the model is that one and the server only places it.
 * Either way the start, sent with the composer's model and no account, lands
 * on the login shown.
 */
export function useComposerStart(source: Source) {
  const cli: CliSource = source === "acp" ? "claude" : source;
  const stored = useComposerDefaultModel(cli);
  const named = stored && isSendable(stored) ? stored : null;
  // An ACP agent has no accounts: nothing to ask the server where a start would land.
  return useStartAccount(cli, named, !named, source !== "acp");
}

/**
 * Composer model/effort/permission, stored server-side so every device sees
 * the same choices. Pass a session id as `scope` to read/write that session's
 * entry; every write also refreshes the per-source default used for new
 * sessions. Values chosen before the server store existed are picked up from
 * localStorage until the first server-side write.
 *
 * `cliChoice` is what the session's own transcript says it last ran with.
 * It races against the stored pick: a pick made after the last turn is intent
 * not yet executed and wins; a transcript newer than the pick means the model
 * was changed elsewhere (e.g. /model in a terminal) and wins instead.
 *
 * `accountId` is the account whose credentials the run will use — null for the
 * local login. It only selects the *model list*: entitlements are per login, so
 * a picker fed the wrong account offers models the run cannot start and hides
 * ones it can. Every caller has to name it, because there is no safe default:
 * "whatever the server itself runs as" is precisely the wrong answer for a
 * session bound to an account.
 */
export function useComposerChoice(
  source: Source,
  scope?: string,
  cliChoice?: CliChoice | null,
  accountId?: string | null,
  /** The model to start on when none (sendable) is stored: the one the server paired with the login shown ([`useComposerStart`]). */
  startModel?: string | null,
  /** An ACP session's agent, by id: whose models and efforts are offered. */
  acpAgent?: string | null,
) {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const prefsKey = scope ? `${source}/${scope}` : null;
  // An ACP agent chooses nothing here (see `CliSource`); the hooks below still run, as
  // hooks must, against a stand-in that is never shown or sent.
  const acp = source === "acp";
  const cli: CliSource = acp ? "codex" : source;

  const { data: prefs } = useQuery({
    queryKey: mq(machine, "composer-prefs"),
    queryFn: async () => (await api.GET("/api/composer-prefs")).data,
    // Written through this cache by the one control that changes it, so the
    // only thing a refetch can bring is another device's choice — not worth a
    // request on every session switch.
    staleTime: 5 * 60_000,
  });

  const save = useMutation({
    mutationFn: async (patch: ComposerChoice) => {
      const res = scope
        ? await api.PUT("/api/composer-prefs/{source}/{id}", {
            params: { path: { source, id: scope } },
            body: patch,
          })
        : await api.PUT("/api/composer-prefs/{source}", {
            params: { path: { source } },
            body: patch,
          });
      return res.data;
    },
    onMutate: (patch) => {
      // Optimistic: selects reflect the pick immediately. The stamp goes in with
      // it because the pick is only newer than the transcript if it is dated:
      // keeping the previous entry's `updatedAt` for the length of the round
      // trip makes a just-made pick look older than the last turn, which snaps
      // the select back to the running model and flashes the drift badge.
      const stamped = { ...patch, updatedAt: new Date().toISOString() };
      queryClient.setQueryData<ComposerPrefs>(mq(machine, "composer-prefs"), (old) => {
        const doc = old ?? { defaults: {}, sessions: {} };
        return {
          defaults: { ...doc.defaults, [source]: { ...doc.defaults[source], ...stamped } },
          sessions: prefsKey
            ? { ...doc.sessions, [prefsKey]: { ...doc.sessions[prefsKey], ...stamped } }
            : doc.sessions,
        };
      });
    },
    onSuccess: (doc) => {
      if (doc) queryClient.setQueryData(mq(machine, "composer-prefs"), doc);
    },
  });

  // Kept by the machine they were chosen for (`rh.composer.…` on the local machine).
  const legacy = (field: "model" | "effort" | "permission"): string | undefined =>
    readMachineItem(localStorage, machine, `composer.${field}.${source}${scope ? `.${scope}` : ""}`) ??
    readMachineItem(localStorage, machine, `composer.${field}.${source}`) ??
    undefined;
  const entry = prefsKey ? prefs?.sessions[prefsKey] : undefined;
  const sourceDefault = prefs?.defaults[source];
  const cliIsFresher = !!cliChoice?.at && (!entry?.updatedAt || cliChoice.at > entry.updatedAt);
  const pick = (field: "model" | "effort" | "permission"): string | undefined => {
    const actual = field === "permission" ? null : (cliChoice?.[field] ?? null);
    if (actual && (cliIsFresher || entry?.[field] === undefined)) return actual;
    return entry?.[field] ?? sourceDefault?.[field] ?? legacy(field);
  };

  // Codex has one config root and no account to pick, so its key stays
  // account-free: sending a meaningless id would split one cache entry in two.
  const modelAccount = source === "claude" ? (accountId ?? null) : null;
  // Every ACP agent publishes its own list; with no agent named there is none.
  const modelAgent = acp ? (acpAgent ?? null) : null;
  const { data: modelData, isError: modelsFailed } = useQuery({
    // The account is in the key, not just the query: two accounts publish two
    // lists, and one served out of the other's entry is the bug this fixes.
    // The same goes for two ACP agents.
    queryKey: mq(machine, "models", source, acp ? modelAgent : modelAccount),
    queryFn: async () =>
      (await api.GET("/api/models", {
        params: {
          query: {
            source,
            ...(modelAccount ? { accountId: modelAccount } : {}),
            ...(modelAgent ? { acpAgent: modelAgent } : {}),
          },
        },
      })).data,
    staleTime: 5 * 60_000,
    enabled: !acp || modelAgent !== null,
  });
  // An ACP agent's list is the agent's own (its `model` option); there is no
  // static guess to fall back on, and no list means no pickers at all.
  const catalog = modelData?.models.length ? modelData.models : acp ? [] : MODEL_FALLBACK[cli];
  // Whether the list on offer describes this machine's CLI or is a static guess.
  // A pending query answers "no": a warning that flashes on every mount is noise
  // about a condition that has not been established yet.
  // `account_env` is as real a list as the cache — it is the model the
  // account's own env block names — so only the static guess is flagged.
  const modelsAreFallback = modelData ? modelData.source === "fallback" : modelsFailed;
  const resolved = catalog.length
    ? resolveModel(catalog, startingModel(pick("model"), startModel), cli)
    : { options: [], active: NO_MODEL, custom: false };
  // An ACP agent takes only what it published: a stored pick it no longer
  // offers falls back to its first model rather than becoming a typed-in one.
  const { options: models, active: activeModel, custom: modelIsCustom } =
    acp && resolved.custom ? { options: catalog, active: catalog[0]!, custom: false } : resolved;
  const effortOptions = effortsOf(activeModel, cli);
  const storedEffort = pick("effort") ?? "";
  // Null for a model with no ladder, and for a stored rung the current model
  // does not accept: the effort follows the model rather than outliving it.
  const activeEffort: string | null = effortOptions.includes(storedEffort)
    ? storedEffort
    : defaultEffortOf(activeModel, effortOptions);
  const storedPermission = pick("permission") ?? "";
  // Stored values from an older option set fall back to the safe default.
  const activePermission = PERMISSION_OPTIONS[cli].includes(storedPermission)
    ? storedPermission
    : PERMISSION_DEFAULT[cli];

  const pickModel = (id: string) => {
    const next = models.find((m) => m.id === id);
    const opts = effortsOf(next, cli);
    const patch: ComposerChoice = { model: id };
    // Efforts do not carry across models: `xhigh` is a rung opus-4-6 never had,
    // and sending it would be refused. Converge on what the new model names.
    // A model with no ladder leaves the stored rung alone — nothing is sent for
    // it either way, and switching back should restore what was picked before.
    // Which is why the test is against the *stored* rung and not the active one:
    // while a no-ladder model is selected the active rung is null, so reading it
    // here would overwrite the stored `xhigh` with a default on the way back.
    const carried = activeEffort ?? storedEffort;
    const converged = opts.includes(carried) ? null : defaultEffortOf(next, opts);
    if (converged) patch.effort = converged;
    save.mutate(patch);
  };

  if (acp) {
    // No permission modes, and nothing at all while the agent's list is
    // unknown: an empty choice is "send none", which the composers leave out
    // of the request.
    return {
      models,
      activeModel,
      customModel: null,
      effortOptions: models.length ? effortOptions : [],
      activeEffort: models.length ? activeEffort : null,
      modelsAreFallback: false,
      modelsHidden: modelData?.hidden ?? 0,
      pickModel: models.length ? pickModel : () => {},
      setEffort: (v: string) => save.mutate({ effort: v }),
      activePermission: "",
      setPermission: () => {},
    };
  }
  return {
    models,
    activeModel,
    /** The active id when it is typed-in rather than listed; null otherwise. */
    customModel: modelIsCustom ? activeModel.id : null,
    /** Empty when the model takes no `--effort`; the picker hides itself then. */
    effortOptions,
    /** Null when there is no rung to run at, and so no flag to send. */
    activeEffort,
    /** The list is the static guess, not the CLI's own; say so beside it. */
    modelsAreFallback,
    /** How many of an ACP agent's models the list leaves out; none for a CLI. */
    modelsHidden: 0,
    pickModel,
    setEffort: (v: string) => save.mutate({ effort: v }),
    activePermission,
    setPermission: (v: string) => save.mutate({ permission: v }),
  };
}

/**
 * A model picker that can also be typed into.
 *
 * The lists the CLIs publish are not the set of models a run may ask for: an
 * account pointed at a self-hosted endpoint answers to whatever name that
 * endpoint gave its weights, and a fixed menu is a picker that cannot reach it
 * at all. So the last entry opens a box, and what is typed there becomes the
 * active id — stored, sent and shown exactly like a listed one.
 */
export function ModelSelect({
  models,
  value,
  customId,
  onChange,
  ghost = false,
  className,
  leading,
  allowCustom = true,
}: {
  models: ModelInfo[];
  /** The active id; `""` selects `leading`, where a form offers one. */
  value: string;
  /** The active id when it is typed-in rather than listed; null otherwise. */
  customId: string | null;
  onChange: (id: string) => void;
  ghost?: boolean;
  /** Overrides the bordered select's own sizing, for a form that sets its own. */
  className?: string;
  /** An entry before the models, e.g. a form's "CLI default". */
  leading?: { value: string; label: string };
  /** Offer the box a model id can be typed into; off for an agent that takes only what it lists. */
  allowCustom?: boolean;
}) {
  const { t } = useTranslation();
  // Null while the box is closed; the draft while it is open, which is why ""
  // has to be a state of its own rather than "not typing".
  const [draft, setDraft] = useState<string | null>(null);
  // Enter commits and then unmounts the input, and a blur on the way out would
  // otherwise commit the same text a second time.
  const settled = useRef(false);
  // Focused a task after the box mounts rather than with `autoFocus`: the list
  // the box is opened from hands focus back to its trigger as it closes, just
  // after this commit, and would take it off the box (whose blur commits it).
  const box = useRef<HTMLInputElement>(null);
  const editing = draft !== null;
  useEffect(() => {
    if (!editing) return;
    const timer = setTimeout(() => box.current?.focus(), 0);
    return () => clearTimeout(timer);
  }, [editing]);

  const finish = (raw: string | null) => {
    if (settled.current) return;
    settled.current = true;
    setDraft(null);
    const id = raw === null ? null : commitCustomModel(raw);
    // A cancel, an unsendable id and re-typing the current one all leave the
    // previous pick exactly where it was.
    if (id && id !== value) onChange(id);
  };

  if (draft !== null) {
    return (
      <input
        ref={box}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            finish(draft);
          } else if (e.key === "Escape") {
            e.preventDefault();
            finish(null);
          }
        }}
        onBlur={() => finish(draft)}
        autoComplete="off"
        spellCheck={false}
        aria-label={t("custom model id")}
        placeholder="GLM-4.6 / claude-opus-5 …"
        className={cn(
          "min-w-0 border border-fg bg-bg px-1.5 font-mono text-2xs text-fg placeholder:text-faint focus:outline-none",
          ghost ? "h-6 w-44 shrink-0" : "h-5.5 w-44",
          className,
        )}
      />
    );
  }

  const listed = modelOptions(models, customId, { custom: t("custom…"), marker: t("custom") });
  const options = allowCustom ? listed : listed.filter((option) => option.value !== CUSTOM_MODEL);
  return (
    <ComposerSelect
      ghost={ghost}
      className={className}
      label={t("model")}
      value={value}
      options={leading ? [leading, ...options] : options}
      onChange={(next) => {
        if (next !== CUSTOM_MODEL) {
          onChange(next);
          return;
        }
        // Opens on the id in use, so a custom model is edited rather than
        // retyped; a listed one starts empty because its name is already there.
        settled.current = false;
        setDraft(customId ?? "");
      }}
    />
  );
}

/** Permission modes that drop the guard rails, drawn in the danger colour wherever they are picked. */
export const DANGER_PERMISSIONS = new Set(["bypassPermissions", "full-access", "bypass-all"]);

/**
 * The composer's own pickers, in the order the toolbar has them: model,
 * effort, the account (`account`, where the composer chooses one — a session
 * already running has its account in the header), permission.
 *
 * One row for both composers: a session's, which saves the picks to that
 * session, and the new session's, which saves them as the CLI's default.
 */
export function ComposerChoiceChips({
  source,
  choice,
  permission = true,
  account,
}: {
  source: Source;
  choice: ReturnType<typeof useComposerChoice>;
  /** Instant chats run without tools: no permission to pick. */
  permission?: boolean;
  account?: ReactNode;
}) {
  const { t } = useTranslation();
  // An ACP agent offers its own models and efforts, and no permission
  // modes; with its list unknown there is nothing to pick at all.
  const acp = source === "acp";
  if (acp && choice.models.length === 0) {
    // Every model left out still says why there is no picker.
    return choice.modelsHidden > 0 ? <ModelSourceHint fallback={false} hidden={choice.modelsHidden} /> : null;
  }
  return (
    <>
      <ModelSelect
        ghost
        models={choice.models}
        value={choice.activeModel.id}
        customId={choice.customModel}
        onChange={choice.pickModel}
        allowCustom={!acp}
      />
      <ModelSourceHint fallback={choice.modelsAreFallback} hidden={choice.modelsHidden} />
      {/* Absent, not disabled, for a model that takes no effort: an empty
          control in a toolbar reads as something that failed to load. */}
      {choice.activeEffort !== null && (
        <ComposerSelect ghost label={t("effort")} value={choice.activeEffort} options={choice.effortOptions} onChange={choice.setEffort} />
      )}
      {account}
      {permission && !acp && (
        <ComposerSelect
          ghost
          label={t("permission")}
          value={choice.activePermission}
          options={PERMISSION_OPTIONS[source]}
          onChange={choice.setPermission}
          danger={DANGER_PERMISSIONS.has(choice.activePermission)}
        />
      )}
    </>
  );
}

/** The same CLI model/effort controls on both conversation creation surfaces. */
export function CliModelControls({ choice }: { choice: ReturnType<typeof useComposerChoice> }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <ModelSelect
        models={choice.models}
        value={choice.activeModel.id}
        customId={choice.customModel}
        onChange={choice.pickModel}
        className="h-6.5 min-w-0 max-w-full"
      />
      {choice.activeEffort !== null && (
        <ComposerSelect className="h-6.5 min-w-0 max-w-full" label={t("effort")} value={choice.activeEffort} options={choice.effortOptions} onChange={choice.setEffort} />
      )}
      <ModelSourceHint fallback={choice.modelsAreFallback} hidden={choice.modelsHidden} />
    </div>
  );
}

/**
 * A mark beside the model picker when the list it offers is the static guess,
 * or leaves out `hidden` of an ACP agent's models: ones whose ids look like a
 * credential, which the device does not show or send back.
 *
 * Nothing at all in the normal case: a list read from the CLI's own cache is
 * simply correct, and a badge saying so on every session is noise. The fallback
 * is the one worth knowing about, because picking a model off it is picking one
 * this machine may not have.
 */
export function ModelSourceHint({ fallback, hidden = 0 }: { fallback: boolean; hidden?: number }) {
  const { t } = useTranslation();
  if (!fallback && hidden === 0) return null;
  const said = hidden > 0
    ? t("{{n}} of the agent's models are not shown: their ids look like credentials", { n: hidden })
    : t("model list is a static fallback — the CLI's own list could not be read");
  return (
    // Native tooltip: the hint sits in a toolbar that already scrolls
    // horizontally on a phone, where a popover would be a trap.
    <span data-signal="warn" className="flex shrink-0 items-center text-warn-fg/70" title={said} aria-label={said}>
      <TriangleAlert className="size-3 shrink-0" />
    </span>
  );
}

/**
 * The session is running a different model than the one it was asked for.
 *
 * Permanent rather than transient, and in the header rather than the transcript,
 * because a silent downgrade is a property of the session as it stands: the
 * answers are coming back from a smaller model for as long as it lasts, and a
 * notice that scrolled away with the turn it happened on is one the reader has
 * already lost by the time they wonder why the agent got worse.
 *
 * It reads the same two facts the composer's own race resolution does, so the
 * badge and the model select can never disagree: what the stored pick asked for,
 * and what the transcript says actually ran. Every ambiguity resolves to no
 * badge — see [`modelDrift`] and [`judgeableRequest`].
 *
 * A bare mark rather than the model's name: the header is two lines on a phone
 * and a model id is long enough to spend one of them by itself. The names are
 * what the reader wants *once*, on the hover or tap that asks.
 */
export function ModelDriftBadge({
  source,
  id,
  cliChoice,
}: {
  source: Source;
  id: string;
  cliChoice?: CliChoice | null;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "composer-prefs"),
    queryFn: async () => (await api.GET("/api/composer-prefs")).data,
    // Written through this cache by the one control that changes it, so the
    // only thing a refetch can bring is another device's choice — not worth a
    // request on every session switch.
    staleTime: 5 * 60_000,
  });

  const entry = prefs?.sessions[`${source}/${id}`];
  // The session's own pick when it has one, else the per-source default this
  // session would have been started with. Taken as a whole entry so the model
  // and the stamp that dates it always come from the same place.
  const stored = entry?.model ? entry : prefs?.defaults[source];
  const requested = judgeableRequest(stored?.model, stored?.updatedAt, cliChoice?.at);
  const observed = cliChoice?.model ?? null;
  if (!modelDrift(requested, observed)) return null;
  const said = t("requested {{requested}}, actually running {{observed}}", { requested, observed });
  return (
    <Popover>
      <PopoverTrigger
        openOnHover
        delay={150}
        // No `title`: the popover is the tooltip, and a native one on the same
        // hover would say the same sentence twice.
        aria-label={said}
        className="flex size-5 shrink-0 items-center justify-center rounded bg-warn/15 text-warn-fg hover:bg-warn/25"
      >
        <TriangleAlert className="size-3 shrink-0" />
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-64 max-w-[calc(100vw-2rem)]">
        <div className="flex flex-col gap-1 font-mono text-2xs leading-4">
          <span data-signal="warn" className="text-warn-fg">{t("model downgraded")}</span>
          <span className="break-all text-muted">{said}</span>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/**
 * Tiny fuel gauges with the percentage left for this session's quota windows,
 * right in the composer toolbar — the bound account's for a managed claude
 * session, the CLI's own otherwise. Hover (or tap) opens an overview of every
 * account's usage instead of leaving the session.
 */
/**
 * `resting · session limit · resumes 02:30`, one line per active rest on the
 * account this session runs under. Nothing for the local login, or for an
 * account that is not resting.
 */
function BoundAccountRest({ accountId }: { accountId: string | null | undefined }) {
  const { t } = useTranslation();
  const { data } = useAccounts();
  if (!accountId) return null;
  const rests = activeRests(data?.accounts.find((a) => a.id === accountId));
  if (rests.length === 0) return null;
  return (
    <div className="mb-0.5 flex flex-col gap-0.5">
      {rests.map((rest) => (
        <div
          key={`${rest.scope} ${rest.class}`}
          className={cn("truncate rounded px-1 font-mono text-[10px] leading-4", restTone(rest.class))}
          title={restTitle(rest, t)}
        >
          {t("resting")} · {restChipText(rest, t)}
        </div>
      ))}
    </div>
  );
}

export function ComposerQuota({
  source,
  accountId,
}: {
  source: Source;
  /** Account this session runs under; null/undefined means the local login. */
  accountId?: string | null;
}) {
  const { t } = useTranslation();
  const { data } = useQuota();
  // This server reads no quota for an ACP agent: its provider is the agent's to configure.
  if (source === "acp") return null;

  if (source === "codex") {
    const codex = data?.codex;
    if (!codex || codex.windows.length === 0) return null;
    // The toolbar has room for gauges and nothing else, so the two things that
    // would make those gauges a lie get one glyph each: a reading that is old
    // (or undateable) and an account that is already blocked. The popover
    // carries the full note.
    const age = quotaAge(codex.updatedAt);
    return (
      <Popover>
        <PopoverTrigger
          openOnHover
          delay={150}
          className="flex h-6 shrink-0 items-center gap-1.5 rounded px-1.5 hover:bg-hover"
        >
          <QuotaGauges windows={codex.windows} />
          {codex.blocked === true && (
            <span data-signal="failed" className="font-mono text-[9px] leading-none text-danger" title={t("blocked")}>
              !
            </span>
          )}
          {age !== "fresh" && (
            <span data-signal="warn" className="font-mono text-[9px] leading-none text-warn-fg"
              title={age === "stale" ? t("stale") : t("age unknown")}
            >
              ~
            </span>
          )}
        </PopoverTrigger>
        <PopoverContent className="w-64 max-w-[calc(100vw-2rem)]">
          <QuotaBlock
            title={codex.plan ?? "codex"}
            windows={codex.windows}
            current
            updatedAt={codex.updatedAt}
            source={codex.source}
            blocked={codex.blocked}
            notice={<BoundAccountRest accountId={accountId} />}
          />
        </PopoverContent>
      </Popover>
    );
  }

  const accounts: AccountQuota[] = data?.accounts ?? [];
  const active = accountId ? (accounts.find((a) => a.id === accountId) ?? null) : null;
  const local = data?.claude ?? null;
  const gaugeWindows = active ? active.windows : (local?.windows ?? []);
  // The server keeps an account's last good windows when its probe fails or its
  // /usage login expires, so gauges can be drawing old numbers. Mark them the
  // way the Codex gauges are marked, and say why in the tooltip — but only once
  // they have aged: a fresh reading comes from the account's own sessions, and
  // a broken probe behind it is only the idle-time fallback (`quotaDoubt`).
  const gaugeUsage = active ? accountUsageState(active.kind, active.usageEnabled, active.error) : "ready";
  const doubt = quotaDoubt(gaugeUsage, active ? active.updatedAt : (local?.updatedAt ?? null));
  const gaugeDoubt =
    doubt === "login_expired"
      ? t("usage login expired")
      : doubt === "unavailable"
        ? t("usage unavailable")
        : doubt === "stale"
          ? t("stale")
          : doubt === "unknown"
            ? t("age unknown")
            : null;
  // Nothing to gauge and no accounts to overview: stay out of the toolbar,
  // exactly as before accounts existed. A bound account with no usage login
  // has no gauges of its own, but the overview is still worth reaching.
  if (gaugeWindows.length === 0 && accounts.length === 0) return null;

  return (
    <Popover>
      <PopoverTrigger
        openOnHover
        delay={150}
        className="flex h-6 shrink-0 items-center gap-1.5 rounded px-1.5 hover:bg-hover"
      >
        {gaugeWindows.length > 0 ? (
          <>
            <QuotaGauges windows={gaugeWindows} />
            {gaugeDoubt && (
              <span data-signal="warn" className="font-mono text-[9px] leading-none text-warn-fg" title={gaugeDoubt}>
                ~
              </span>
            )}
          </>
        ) : (
          <span className="font-mono text-[9px] leading-none text-faint">{t("no usage")}</span>
        )}
      </PopoverTrigger>
      <PopoverContent className="max-h-[60vh] w-80 max-w-[calc(100vw-2rem)] overflow-y-auto p-2">
        <div className="flex flex-col gap-0.5">
          <QuotaBlock
            title={local?.plan ? `${t("local login")} · ${local.plan}` : t("local login")}
            windows={local?.windows ?? []}
            current={!accountId}
            updatedAt={local?.updatedAt ?? null}
            source={local?.source}
          />
          {accounts.map((a) => (
            <QuotaBlock
              key={a.id}
              title={a.label}
              chip={accountKindLabel(a.kind)}
              email={a.email}
              windows={a.windows}
              current={a.id === accountId}
              usage={accountUsageState(a.kind, a.usageEnabled, a.error)}
              updatedAt={a.updatedAt}
              source={a.source}
              notice={a.id === accountId ? <BoundAccountRest accountId={accountId} /> : undefined}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function contextTone(pct: number): string {
  return pct > 85 ? "text-danger" : pct >= 60 ? "text-warn-fg" : "text-fg";
}

/**
 * How full the model's context window is, as a short bar — the same gauge the
 * account quotas use. Ink while there is room, amber past 60%, red past 85%.
 * Hidden until the session has made a request — a session that has never run
 * has no reading to show, which is not the same as zero.
 */
export function ContextRing({
  used,
  contextWindow,
}: {
  used: number | null | undefined;
  contextWindow: number;
}) {
  const { t } = useTranslation();
  if (used === null || used === undefined || !(contextWindow > 0)) return null;
  const pct = Math.max(0, Math.min(100, (used / contextWindow) * 100));
  const tone = contextTone(pct);
  return (
    <span
      data-signal={pct >= 60 ? "context" : undefined}
      className={cn("flex h-6 shrink-0 items-center gap-1.5 px-1.5", tone)}
      title={`${t("context")}: ${formatInt(used)}/${formatInt(contextWindow)}`}
    >
      <span aria-hidden="true" className="relative h-1 w-6 shrink-0 bg-border">
        <span className="absolute inset-y-0 left-0 bg-current" style={{ width: `${pct}%` }} />
      </span>
      <span className="font-mono text-[10px] leading-none tabular-nums">{Math.round(pct)}%</span>
    </span>
  );
}

function checklistGlyph(status: string): { mark: string; tone: string } {
  if (status === "completed") return { mark: "●", tone: "text-ok" };
  if (status === "in_progress") return { mark: "◐", tone: "text-warn-fg" };
  return { mark: "○", tone: "text-faint" };
}

/**
 * `☑ done/total` for the task checklist Claude Code keeps beside a session,
 * with the list itself one tap away. Claude-only (codex has no such sidecar),
 * and absent until the session has actually planned something.
 *
 * `toolTick` changes whenever the transcript grows a tool call — the sidecar is
 * only ever rewritten by one, so that is the cheapest possible "might have
 * changed" signal. Refetching is debounced, because a burst of calls would
 * otherwise poll the file once per item.
 */
export function ChecklistChip({
  source,
  id,
  toolTick,
}: {
  source: Source;
  id: string;
  toolTick: number;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const { data, refetch } = useQuery({
    enabled: source === "claude",
    queryKey: mq(machine, "checklist", source, id),
    queryFn: async () => {
      const res = await api.GET("/api/sessions/{source}/{id}/checklist", {
        params: { path: { source, id } },
      });
      // A session with no sidecar answers 404; that is "no checklist", not an
      // error worth retrying or surfacing.
      return res.data ?? null;
    },
    retry: false,
    staleTime: 10_000,
  });

  // The tick the query above was fetched against. The mount fetch already
  // read the sidecar as of this transcript, so only a tick that moves past it
  // is news: refetching on the first one as well cost every session switch a
  // second, identical request two seconds after the first.
  const fetchedAt = useRef(toolTick);
  useEffect(() => {
    if (toolTick === 0 || toolTick === fetchedAt.current) return;
    fetchedAt.current = toolTick;
    // Each new tick restarts the wait, so a burst collapses into one refetch.
    const timer = setTimeout(() => void refetch(), 2000);
    return () => clearTimeout(timer);
  }, [toolTick, refetch]);

  if (!data || data.total === 0) return null;
  return (
    <Popover>
      <PopoverTrigger
        title={t("task checklist")}
        className="flex h-6 shrink-0 items-center gap-1 rounded px-1.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
      >
        <ListChecks className="size-3 shrink-0" />
        <span className="tabular-nums">
          {data.done}/{data.total}
        </span>
      </PopoverTrigger>
      <PopoverContent className="max-h-[50vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
        <div className="mb-1 font-mono text-2xs text-faint">{t("task checklist")}</div>
        <div className="flex flex-col gap-1">
          {data.items.map((task) => {
            const { mark, tone } = checklistGlyph(task.status);
            return (
              <div key={task.id} className="flex items-start gap-1.5 font-mono text-2xs">
                <span className={cn("shrink-0 leading-4", tone)} title={task.status}>
                  {mark}
                </span>
                <span
                  className={cn(
                    "min-w-0 leading-4 break-words",
                    task.status === "completed" ? "text-faint line-through" : "text-fg",
                  )}
                >
                  {task.title}
                </span>
              </div>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** Grow a textarea with its content; the element's CSS max-height caps it. */
export function autoGrowTextarea(el: HTMLTextAreaElement | null): void {
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight + 2}px`;
}

/** A bare string is its own label; the object form separates the two. */
export type ComposerOption = string | { value: string; label: string; glyph?: Glyph };

type ComposerSelectProps = {
  value: string;
  options: ComposerOption[];
  onChange: (v: string) => void;
  danger?: boolean;
  /** Borderless toolbar style with a drawn chevron, for inside the composer card. */
  ghost?: boolean;
  /** Overrides the bordered select's own sizing, for a form that sets its own. */
  className?: string;
  /** What the control picks, for screen readers and the tooltip; a form's `<label>` can name it instead. */
  label?: string;
};

export function ComposerSelect({ options, onChange, ...rest }: ComposerSelectProps) {
  // Stable items and handler, so typing in the composer (which redraws the
  // whole toolbar) does not redraw every select in it.
  const items = useStableOptions(options.map((o) => (typeof o === "string" ? { value: o, label: o } : o)));
  const pick = useLatest(onChange);
  return <ComposerSelectView {...rest} items={items} onChange={pick} />;
}

const ComposerSelectView = memo(function ComposerSelectView({
  value,
  items,
  onChange,
  danger = false,
  ghost = false,
  className,
  label,
}: Omit<ComposerSelectProps, "options"> & { items: readonly { value: string; label: string; glyph?: Glyph }[] }) {
  return (
    <Select
      value={value}
      items={items}
      onValueChange={(next) => {
        if (next !== null && next !== value) onChange(next);
      }}
    >
      <SelectTrigger
        variant={ghost ? "chip" : "field"}
        aria-label={label}
        title={label}
        data-signal={danger ? "danger" : undefined}
        iconClassName={danger ? "text-danger" : undefined}
        className={
          ghost
            ? cn(danger && "bg-danger/10 text-danger hover:bg-danger/10 hover:text-danger data-[popup-open]:bg-danger/10 data-[popup-open]:text-danger")
            : cn(
                "h-5.5 w-auto px-1",
                danger ? "border-danger/50 text-danger" : "text-muted hover:text-fg data-[popup-open]:text-fg",
                className,
              )
        }
      >
        {/* Wide enough for a dated model id (`claude-haiku-4-5-20251001`), which
            is the longest thing this control has to say. The cap still exists so
            one freakish value cannot stretch the row, and the toolbar wraps
            rather than scrolls, so a long name costs a line, not a cut-off. */}
        <SelectValue className={ghost ? "max-w-48" : undefined}>{glyphValue(items)}</SelectValue>
      </SelectTrigger>
      {/* Anchored to the chip like every select: a chip at the bottom of the
          composer opens upward, over the message box. */}
      <SelectContent variant={ghost ? "chip" : "field"}>
        {items.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            <OptionLabel label={o.label} glyph={o.glyph} />
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
});
