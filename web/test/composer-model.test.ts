// Which model the composer says a session is running, and which efforts it
// offers for it.
//
// The rule under test is that the composer never names a model the session is
// not on: an id it cannot place is shown as itself, because `activeModel.id` is
// also what the next turn is sent with.
import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import {
  commitCustomModel,
  CUSTOM_MODEL,
  defaultEffortOf,
  effortsOf,
  EFFORT_FALLBACK,
  judgeableRequest,
  MODEL_FALLBACK,
  modelDrift,
  modelOptions,
  resolveModel,
} from "../src/features/sessions/composer-controls.js";

type ModelInfo = components["schemas"]["ModelInfo"];

function model(id: string, over: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    label: `${id} (latest)`,
    efforts: ["low", "medium", "high", "xhigh", "max"],
    defaultEffort: "medium",
    ...over,
  };
}

const claudeModels = [model("fable"), model("opus"), model("sonnet"), model("haiku")];

test("a listed model is the active one, and the list is left alone", () => {
  const { options, active } = resolveModel(claudeModels, "sonnet", "claude");
  assert.equal(active.id, "sonnet");
  assert.equal(options, claudeModels);
});

test("a concrete claude id falls back to its tier alias", () => {
  assert.equal(resolveModel(claudeModels, "claude-opus-5", "claude").active.id, "opus");
  assert.equal(resolveModel(claudeModels, "claude-haiku-4-5-20251001", "claude").active.id, "haiku");
});

/**
 * What the server sends once the CLI has been asked: the id is the alias that
 * tracks the newest model of a tier, and the label is the concrete model that
 * alias resolves to today.
 */
const publishedModels: ModelInfo[] = [
  { id: "opus[1m]", label: "claude-opus-5[1m]", efforts: ["low", "medium", "high", "xhigh", "max"], defaultEffort: "medium" },
  { id: "sonnet", label: "claude-sonnet-5", efforts: ["low", "medium", "high", "xhigh", "max"], defaultEffort: "medium" },
  { id: "haiku", label: "claude-haiku-4-5-20251001", efforts: [], defaultEffort: null },
];

test("a transcript's concrete id resolves to the row already labelled with it", () => {
  const { options, active } = resolveModel(publishedModels, "claude-opus-5[1m]", "claude");
  assert.equal(active.id, "opus[1m]", "the same model, not a second option reading the same thing");
  assert.equal(options, publishedModels, "so the menu never shows two rows with one label");
  assert.equal(resolveModel(publishedModels, "claude-haiku-4-5-20251001", "claude").active.id, "haiku");
});

test("a concrete id falls back to a tier alias even when the alias carries a variant tag", () => {
  // Yesterday's opus, against a list whose opus row is `opus[1m]`.
  assert.equal(resolveModel(publishedModels, "claude-opus-4-8", "claude").active.id, "opus[1m]");
});

/**
 * An env account's list, as the server serves it when the account's block sets
 * `ANTHROPIC_MODEL=deepseek-v4.1-flash[1m]`: the named model first, the tier
 * aliases after it.
 */
const envModels: ModelInfo[] = [
  model("deepseek-v4.1-flash[1m]", { label: "deepseek-v4.1-flash[1m]" }),
  ...claudeModels,
];

test("the long-context variant a session runs is the listed one, and is what goes with the next message", () => {
  // #446: the server names the transcript's model as the session runs it, with
  // the `[1m]` the transcript leaves out, so it is the account's own entry.
  const { active, custom } = resolveModel(envModels, "deepseek-v4.1-flash[1m]", "claude");
  assert.equal(active.id, "deepseek-v4.1-flash[1m]");
  assert.equal(custom, false);
  // The bare id is not that entry: offered as itself, it would ask the CLI for
  // the 200k variant. This is why the server restores the marker.
  assert.equal(resolveModel(envModels, "deepseek-v4.1-flash", "claude").active.id, "deepseek-v4.1-flash");
});

test("mythos ids fall back to the mythos alias, not to the head of the list", () => {
  const withMythos = [...claudeModels, model("mythos")];
  assert.equal(resolveModel(withMythos, "claude-mythos-1-20260115", "claude").active.id, "mythos");
});

test("an unplaceable id is shown as itself, appended as an option", () => {
  const { options, active } = resolveModel(claudeModels, "claude-mythos-1", "claude");
  assert.equal(active.id, "claude-mythos-1", "never silently displayed as another model");
  assert.equal(active.label, null);
  assert.deepEqual(
    options.map((m) => m.id),
    ["fable", "opus", "sonnet", "haiku", "claude-mythos-1"],
    "and selectable, so the <select> value has a matching option",
  );
});

test("an id the send routes would refuse is not offered at all", () => {
  // `<synthetic>` is what Claude Code writes as the model of an entry it wrote
  // in the model's own voice — a rate-limit wall, a "Not logged in" — so it is
  // what `cliChoice.model` reports on exactly the sessions a user opens to find
  // out what went wrong. `activeModel.id` is also what the next turn is sent
  // with, and the send routes refuse it, so offering it is a composer that
  // cannot send. The list's head is wrong; unsendable is worse.
  for (const refused of ["<synthetic>", "-opus", "opus[1m", "opus`id`", "o".repeat(101)]) {
    const { options, active } = resolveModel(claudeModels, refused, "claude");
    assert.equal(active.id, "fable", `${refused} must not become the active model`);
    assert.equal(options, claudeModels, "and must not be appended as an option");
  }
});

test("a bare unknown id is shown as itself too", () => {
  const codex = [model("gpt-5.5"), model("gpt-5.4")];
  assert.equal(resolveModel(codex, "gpt-6-preview", "codex").active.id, "gpt-6-preview");
});

test("codex never resolves through the claude tier aliases", () => {
  const codex = [model("gpt-5.5"), model("opus")];
  assert.equal(resolveModel(codex, "claude-opus-5", "codex").active.id, "claude-opus-5");
});

test("with nothing wanted the freshest model leads", () => {
  const { options, active } = resolveModel(claudeModels, undefined, "claude");
  assert.equal(active.id, "fable");
  assert.equal(options, claudeModels);
});

test("efforts come from the model, else from the CLI's own ladder", () => {
  assert.deepEqual(effortsOf(model("opus", { efforts: ["low", "high"] }), "claude"), ["low", "high"]);
  assert.deepEqual(effortsOf(model("opus", { efforts: null }), "claude"), EFFORT_FALLBACK.claude);
  assert.deepEqual(effortsOf(undefined, "codex"), EFFORT_FALLBACK.codex);
});

test("an empty effort list is an answer, not a silence", () => {
  // The whole point of the server publishing `[]`: claude-haiku takes no
  // `--effort` at all, and a fallback ladder here would hand it five rungs the
  // CLI refuses — undoing the fix on the other side of the wire.
  assert.deepEqual(effortsOf(model("haiku", { efforts: [] }), "claude"), []);
  assert.equal(
    defaultEffortOf(model("haiku", { efforts: [], defaultEffort: null }), []),
    null,
    "and no rung to run at, so no flag to send",
  );
});

test("the claude ladder has five rungs and the codex one has no max", () => {
  assert.deepEqual(EFFORT_FALLBACK.claude, ["low", "medium", "high", "xhigh", "max"]);
  assert.ok(!EFFORT_FALLBACK.codex.includes("max"), "codex rejects an effort it does not define");
});

test("the default effort is the model's, else medium, else the middle rung", () => {
  assert.equal(defaultEffortOf(model("opus", { defaultEffort: "high" }), EFFORT_FALLBACK.claude), "high");
  assert.equal(
    defaultEffortOf(model("opus", { defaultEffort: null }), EFFORT_FALLBACK.claude),
    "medium",
    "a longer ladder must not drift the implicit default upwards",
  );
  assert.equal(defaultEffortOf(undefined, ["a", "b", "c"]), "b");
});

test("a default the ladder does not contain is not offered", () => {
  // The server keeps the two in step; if it ever does not, the rung sent has to
  // be one the model accepts rather than one the list happened to name.
  const stale = model("opus", { efforts: ["low", "medium", "high", "max"], defaultEffort: "xhigh" });
  assert.equal(defaultEffortOf(stale, effortsOf(stale, "claude")), "medium");
});

test("the offline model list is the four tier aliases, each labelled", () => {
  assert.deepEqual(
    MODEL_FALLBACK.claude.map((m) => m.id),
    ["fable", "opus", "sonnet", "haiku"],
  );
  for (const m of MODEL_FALLBACK.claude) {
    assert.ok(m.label, `${m.id} needs a human-facing name`);
  }
  // Even offline, haiku's ladder is the empty one the CLI actually enforces.
  const haiku = MODEL_FALLBACK.claude.find((m) => m.id === "haiku")!;
  assert.deepEqual(haiku.efforts, []);
  assert.equal(haiku.defaultEffort, null);
  assert.deepEqual(MODEL_FALLBACK.claude[0]!.efforts, EFFORT_FALLBACK.claude);
});

// The downgrade badge: what the session was asked for against what its own
// transcript says ran. It accuses the CLI of swapping the model out, so every
// case it cannot settle has to come back false.

test("a tier alias is answered by any concrete id of that tier", () => {
  assert.equal(modelDrift("opus", "claude-opus-5"), false);
  assert.equal(modelDrift("claude-opus-5", "opus"), false);
  assert.equal(modelDrift("sonnet", "claude-sonnet-4-5-20250929"), false);
  assert.equal(modelDrift("haiku", "claude-3-5-haiku-20241022"), false, "older spelling, same tier");
});

test("a different tier or a different generation is a drift", () => {
  assert.equal(modelDrift("fable", "claude-sonnet-4-5"), true, "the silent downgrade");
  assert.equal(modelDrift("opus-5", "opus-4-8"), true);
  assert.equal(modelDrift("claude-opus-5", "claude-opus-4-8"), true);
  assert.equal(modelDrift("gpt-5.5", "gpt-5.6-sol"), true);
});

test("the [1m] marker describes the window, not the model", () => {
  assert.equal(modelDrift("claude-opus-4-8[1m]", "claude-opus-4-8"), false);
  assert.equal(modelDrift("opus[1m]", "claude-opus-5"), false);
  assert.equal(modelDrift("claude-opus-4-8[1m]", "claude-opus-5"), true, "still a real swap");
});

test("case and vendor routing prefixes do not make a drift", () => {
  assert.equal(modelDrift("Claude-Opus-5", "claude-opus-5"), false);
  assert.equal(modelDrift("opus", "us.anthropic.claude-opus-5-v1:0"), false);
});

test("anything unjudgeable is not a drift", () => {
  assert.equal(modelDrift(undefined, "claude-opus-5"), false, "nothing was requested");
  assert.equal(modelDrift("opus", null), false, "the transcript said nothing");
  assert.equal(modelDrift("opus", ""), false);
  assert.equal(
    modelDrift("opus", "<synthetic>"),
    false,
    "the CLI's own voice is not a model the session was moved to",
  );
});

test("a pick newer than the last turn is intent, not a broken promise", () => {
  const ran = "2026-09-02T10:00:00.000Z";
  assert.equal(
    judgeableRequest("fable", "2026-09-02T10:05:00.000Z", ran),
    undefined,
    "the dropdown was just changed and nothing has been sent yet",
  );
  assert.equal(judgeableRequest("fable", "2026-09-02T09:55:00.000Z", ran), "fable");
  assert.equal(judgeableRequest("fable", ran, ran), undefined, "a tie cannot be ordered");
});

test("a request or a reading that cannot be placed in time is not judged", () => {
  assert.equal(judgeableRequest(undefined, "2026-09-02T09:00:00.000Z", "2026-09-02T10:00:00.000Z"), undefined);
  assert.equal(judgeableRequest("fable", "2026-09-02T09:00:00.000Z", null), undefined);
  assert.equal(
    judgeableRequest("fable", undefined, "2026-09-02T10:00:00.000Z"),
    "fable",
    "an undated pick predates the server-side store, so the turn is the later of the two",
  );
});

test("the whole race window stays dark", () => {
  // Picking fable on a session running sonnet must not light the badge until
  // the pick has actually had a turn to be honoured.
  const ran = "2026-09-02T10:00:00.000Z";
  const justPicked = judgeableRequest("fable", "2026-09-02T10:00:01.000Z", ran);
  assert.equal(modelDrift(justPicked, "claude-sonnet-4-5"), false);
  const pickedBefore = judgeableRequest("fable", "2026-09-02T09:59:00.000Z", ran);
  assert.equal(modelDrift(pickedBefore, "claude-sonnet-4-5"), true);
});

// ---------------------------------------------------------------------------
// Typing a model the list does not carry
// ---------------------------------------------------------------------------

const text = { custom: "custom…", marker: "custom" };

test("what is typed into the box commits trimmed, or cancels", () => {
  assert.equal(commitCustomModel("  GLM-5.3-CYBERSECURITY-FP8  "), "GLM-5.3-CYBERSECURITY-FP8");
  assert.equal(commitCustomModel("opus[1m]"), "opus[1m]");
  // Emptying the box is how someone changes their mind, so it keeps the
  // previous pick rather than clearing the model.
  assert.equal(commitCustomModel(""), null);
  assert.equal(commitCustomModel("   "), null);
  // The id typed here is the id sent: anything `--model` would be refused for
  // must not become the stored pick, or every later send fails.
  for (const refused of ["two words", "<synthetic>", "-opus", "opus`id`", "o".repeat(101)]) {
    assert.equal(commitCustomModel(refused), null, refused);
  }
});

test("the box's entry is last, and picking it is not a model id", () => {
  const options = modelOptions(claudeModels, null, text);
  assert.deepEqual(
    options.map((o) => o.value),
    ["fable", "opus", "sonnet", "haiku", CUSTOM_MODEL],
  );
  assert.equal(options.at(-1)!.label, "custom…");
  assert.ok(!claudeModels.some((m) => m.id === CUSTOM_MODEL), "and cannot collide with a model");
});

test("a typed-in model stays selected, marked, and never blank", () => {
  // The composer's list already carries it (resolveModel appended it), so it
  // is marked where it stands rather than added a second time.
  const { options, active, custom } = resolveModel(claudeModels, "GLM-4.6", "claude");
  assert.equal(custom, true);
  const shown = modelOptions(options, active.id, text);
  assert.deepEqual(
    shown.map((o) => o.value),
    ["fable", "opus", "sonnet", "haiku", "GLM-4.6", CUSTOM_MODEL],
  );
  assert.equal(shown.find((o) => o.value === "GLM-4.6")!.label, "GLM-4.6 · custom");

  // A form holding a bare string (the scheduled-task editor) has a list that
  // never heard of it; the entry is appended so the select has one to select.
  const bare = modelOptions(claudeModels, "GLM-4.6", text);
  assert.deepEqual(bare.map((o) => o.value), ["fable", "opus", "sonnet", "haiku", "GLM-4.6", CUSTOM_MODEL]);
  assert.equal(bare.at(-2)!.label, "GLM-4.6 · custom");
});

test("only the typed-in model is marked, and a listed one never is", () => {
  assert.equal(resolveModel(claudeModels, "sonnet", "claude").custom, false);
  assert.equal(resolveModel(claudeModels, "claude-opus-5", "claude").custom, false, "an alias is listed");
  assert.equal(resolveModel(claudeModels, undefined, "claude").custom, false);
  assert.equal(resolveModel(claudeModels, "<synthetic>", "claude").custom, false, "and unsendable is not custom");
  for (const o of modelOptions(claudeModels, null, text)) {
    assert.ok(!o.label.includes("· custom"), o.label);
  }
});

test("a typed-in model gets the whole ladder, since no list names its rungs", () => {
  const { active } = resolveModel(claudeModels, "GLM-4.6", "claude");
  assert.deepEqual(effortsOf(active, "claude"), EFFORT_FALLBACK.claude);
  assert.equal(defaultEffortOf(active, effortsOf(active, "claude")), "medium");
});
