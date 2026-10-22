/*
 * `/welcome`: the Droppen app's first-run guide, the whole window. The
 * progress lives on the server
 * (`GET/PUT /api/setup/progress`), so leaving and coming back — "Finish
 * later", the bar above the session list, Settings → General → Setup —
 * picks up where it stopped; `?rerun=1` starts over from step 01.
 */
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSaveProgress, useSetupProgress, useSetupTools } from "./api";
import { ActionBar, GuideSheet, GuideTopBar, pressesPrimary, type StepAction, StepHeader, StepPage, StepRail, useOnline } from "./frame";
import {
  complete,
  emptyProgress,
  finish,
  nextStep,
  opensFirstSession,
  prevStep,
  rerun,
  resumeStep,
  type Screen,
  skip,
  start,
  type WelcomeSearch,
} from "./model";
import { type StepProps, WelcomeScreen, WhereStep, whereChoice, WrapStep } from "./screens";
import { EMPTY_FIRST_UI, type FirstUi, firstOf, firstUiFrom, noToolReady } from "./first-model";
import { firstSessionOf, useOpenFirstSession } from "./first-session";
import { FirstStep, type FirstStepUi } from "./first-step";
import { SetupToasts, showSetupToast } from "./toast";
import { cardsOwnPrimary, IDLE_TOOLS_UI, type ToolsUi } from "./tools-model";
import { ToolsStep, type ToolsStepUi } from "./tools-step";
import type { SetupProgress, StepId, ToolsReport } from "./types";
import { type GuideSignIn, useGuideSignIn, type WhereSignIn } from "./where-sign-in";

/** Each step's kicker, title and lead. */
export const STEP_HEADERS: Record<StepId, { kicker: string; title: string; body?: string }> = {
  tools: {
    kicker: "01 · AI tools",
    title: "Get Claude Code or Codex ready",
    body: "Droppen runs these AI tools on this device. One is enough; you can add the other any time. Droppen only asks each tool for its own status. It never reads your keychain or saved passwords.",
  },
  where: {
    kicker: "02 · Where to use it",
    title: "Where will you use Droppen?",
    body: "Droppen runs on this Mac either way. This only decides what else can reach it, and you can change it any time in Settings.",
  },
  first: {
    kicker: "03 · First session",
    title: "Pick a folder for the AI to work in",
    // The first session runs with the device's own permission, shown in the view's row; "ask before changing" waits for approvals.
    body: "The AI works in the folder you pick. The suggested first messages only ask it to look, and nothing runs until you press Send on the next screen, where the permission it runs with is shown and can be changed.",
  },
  wrap: { kicker: "04 · Finishing up", title: "A few last things" },
};

/** The step bodies. */
const BODIES: Record<StepId, (props: StepProps & ToolsStepUi & FirstStepUi) => React.ReactNode> = {
  tools: ToolsStep,
  where: WhereStep,
  first: FirstStep,
  wrap: WrapStep,
};

const now = () => new Date().toISOString();

/** Whether Enter belongs to what has the focus (typing, a terminal, a control of its own) rather than to the page's primary. */
function ownsEnter(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest("input, textarea, select, [contenteditable=''], [contenteditable='true'], .xterm")) return true;
  // A button or link Enter already presses — but a choice is not an action: Enter on it goes on.
  return target.closest("button, a[href], [role='button'], [role='menuitem'], [role='menuitemradio']") !== null && target.closest("[role='radio'], [role='checkbox']") === null;
}

export function WelcomeView() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const search = useSearch({ strict: false }) as WelcomeSearch;
  const online = useOnline();
  const progressQuery = useSetupProgress();
  // Asked as the guide opens, so the welcome page can say what it found.
  const toolsQuery = useSetupTools();
  const save = useSaveProgress();
  const progress: SetupProgress | undefined = progressQuery.data ?? (progressQuery.isFetched ? emptyProgress() : undefined);
  const tools = toolsQuery.isPending ? undefined : (toolsQuery.data ?? null);
  const [screen, setScreen] = useState<Screen | null>(null);
  // What is open on step 01's cards: the guide keeps it, so the action bar knows whether a card holds the primary.
  const [toolsUi, setToolsUi] = useState<ToolsUi>(IDLE_TOOLS_UI);
  // What step 03 has picked: stored in the progress on Continue, taken from it when the step opens.
  const [firstUi, setFirstUi] = useState<FirstUi>(EMPTY_FIRST_UI);
  const firstOpened = useRef(false);
  const openFirstSession = useOpenFirstSession();
  // This Mac's sign-in to Droppen (step 02): the page's one, so it goes on while the guide does.
  const signIn = useGuideSignIn();
  // Kept going while it was under way: say so once this Mac is in the account.
  const signingIn = useRef(false);
  useEffect(() => {
    const next = linkedToast(signingIn.current, signIn.state.kind, screen);
    signingIn.current = next.pending;
    if (next.toast) showSetupToast(LINKED_TOAST);
  }, [signIn.state.kind, screen]);

  // Where the guide opens, once the progress is known: a rerun from step 01,
  // a guide never started on the welcome page, an unfinished one where it stopped.
  useEffect(() => {
    if (screen !== null || !progress) return;
    if (search.rerun) {
      void save(rerun(progress, now()));
      setScreen("tools");
      void navigate({ to: "/welcome", search: {}, replace: true });
      return;
    }
    setScreen(progress.startedAt && !progress.finishedAt ? resumeStep(progress) : "welcome");
  }, [screen, progress, search.rerun, save, navigate]);

  useEffect(() => {
    if (screen !== "first" || firstOpened.current || !progress) return;
    firstOpened.current = true;
    setFirstUi(firstUiFrom(progress));
  }, [screen, progress]);

  const heading = useRef<HTMLHeadingElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  // Each step takes the focus on its title as it opens.
  useEffect(() => {
    if (screen && screen !== "welcome") heading.current?.focus();
  }, [screen]);

  // Enter is the primary, unless the focus is typing or on a control of its
  // own; Esc does nothing here (Claude's TUI in a terminal has a use for it).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // Not a held Enter's repeats: each would press the next step's primary as it appears.
      if (!pressesPrimary(event)) return;
      if (ownsEnter(event.target)) return;
      const primary = frame.current?.querySelector<HTMLButtonElement>("[data-guide-primary]:not(:disabled)");
      if (!primary) return;
      event.preventDefault();
      primary.click();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Leaving the guide at its end: into the first session step 03 saved —
  // the empty-session view with it filled in, nothing sent — else the list.
  const leave = useCallback(
    (next: SetupProgress) => {
      void save(next);
      const first = firstSessionOf(next);
      if (first) openFirstSession(first);
      else void navigate({ to: "/sessions" });
    },
    [save, navigate, openFirstSession],
  );

  if (!progress || !screen) return <div className="h-full bg-bg" />;

  const goto = (step: StepId, next: SetupProgress = progress) => {
    if (next !== progress) void save(next);
    setScreen(step);
  };
  // "Finish later": what was done is kept; the bar above the session list picks it up.
  const later = () => {
    void save(start(progress, now()));
    void navigate({ to: "/sessions" });
  };

  if (screen === "welcome") {
    return (
      <GuideScreen
        frameRef={frame}
        screen="welcome"
        progress={progress}
        tools={tools}
        online={online}
        onStart={() => goto(resumeStep(progress), start(progress, now()))}
        onLater={later}
      />
    );
  }

  const step = screen;
  const forward = (next: SetupProgress) => {
    const after = nextStep(step);
    if (after) goto(after, next);
    else leave(next);
  };
  return (
    <GuideScreen
      frameRef={frame}
      headingRef={heading}
      screen={step}
      progress={progress}
      tools={tools}
      online={online}
      onLater={later}
      toolsUi={toolsUi}
      onToolsUi={setToolsUi}
      firstUi={firstUi}
      onFirstUi={setFirstUi}
      onGoto={(to) => goto(to)}
      onChange={(next) => void save(next)}
      onSkip={() => {
        const skipped = skip(start(progress, now()), step);
        // Skipping the last step ends the guide, as Finish does but without its choices.
        if (!nextStep(step)) leave({ ...skipped, finishedAt: now() });
        else forward(skipped);
      }}
      onBack={prevStep(step) ? () => goto(prevStep(step)!) : null}
      signIn={signIn}
      action={stepAction(
        step,
        progress,
        tools,
        t,
        {
          next: () => forward(complete(start(settle(step, progress, { online, tools, firstUi, t }), now()), step)),
          finish: () => leave(finish(progress, now())),
          // Step 02's sign-in stopped: this Mac only, for now.
          local: () => {
            signIn.cancel();
            forward(complete(start({ ...progress, where: { ...progress.where, choice: "local" } }, now()), step));
          },
        },
        toolsUi,
        firstUi,
        online,
        signIn,
      )}
    />
  );
}

/** What the guide draws for one screen, given what to do on each action (the render tests draw it directly). */
export function GuideScreen({
  screen,
  progress,
  tools,
  online,
  frameRef,
  headingRef,
  onStart = noop,
  onLater,
  onGoto = noop,
  onChange = noop,
  onSkip = noop,
  onBack = null,
  action = null,
  toolsUi = IDLE_TOOLS_UI,
  onToolsUi,
  firstUi = EMPTY_FIRST_UI,
  onFirstUi,
  signIn,
}: {
  screen: Screen;
  progress: SetupProgress;
  tools: ToolsReport | null | undefined;
  online: boolean;
  frameRef?: React.Ref<HTMLDivElement>;
  headingRef?: React.Ref<HTMLHeadingElement>;
  onStart?: () => void;
  /** "Finish later" on a step; "Skip setup and open Droppen" on the welcome page. */
  onLater: () => void;
  onGoto?: (step: StepId) => void;
  onChange?: (next: SetupProgress) => void;
  onSkip?: () => void;
  onBack?: (() => void) | null;
  action?: StepAction | null;
  /** Step 01's opened forms (the guide's state). */
  toolsUi?: ToolsUi;
  onToolsUi?: (next: ToolsUi) => void;
  /** Step 03's picks (the guide's state). */
  firstUi?: FirstUi;
  onFirstUi?: (next: FirstUi) => void;
  /** This Mac's sign-in to Droppen (steps 02 and 04). */
  signIn?: GuideSignIn;
}) {
  const { t } = useTranslation();
  if (screen === "welcome") {
    return (
      <div ref={frameRef} data-testid="guide" data-screen="welcome" className="flex h-full flex-col bg-bg text-fg">
        <GuideTopBar onLater={null} online={online} />
        <GuideSheet rail={null}>
          <WelcomeScreen tools={tools} onStart={onStart} onSkipAll={onLater} />
        </GuideSheet>
        <SetupToasts />
      </div>
    );
  }
  const Body = BODIES[screen];
  const header = STEP_HEADERS[screen];
  return (
    <div ref={frameRef} data-testid="guide" data-screen={screen} className="flex h-full flex-col bg-bg text-fg">
      <GuideTopBar onLater={onLater} online={online} />
      <GuideSheet rail={<StepRail current={screen} progress={progress} onGoto={onGoto} />}>
        <StepPage bar={<ActionBar step={screen} onSkip={onSkip} onBack={onBack} action={action} />}>
          <StepHeader ref={headingRef} kicker={t(header.kicker)} title={t(header.title)} body={header.body ? t(header.body) : undefined} />
          <Body
            progress={progress}
            tools={tools}
            online={online}
            onChange={onChange}
            toolsUi={toolsUi}
            onToolsUi={onToolsUi}
            firstUi={firstUi}
            onFirstUi={onFirstUi}
            onGoto={onGoto}
            signIn={signIn}
          />
        </StepPage>
      </GuideSheet>
      <SetupToasts />
    </div>
  );
}

const noop = () => undefined;

/** The guide's line once this Mac is in the account while the guide went on. */
export const LINKED_TOAST = "This Mac is in your Droppen account";

/**
 * Whether this Mac joining its Droppen account is the guide's toast to show,
 * given whether the sign-in was under way when the guide last looked
 * (`wasPending`), how it stands now and the screen on show. Only a sign-in
 * seen under way and now in the account, and not on step 02, which says so
 * itself; `pending` is what to remember for the next look. An account the
 * device was in all along is no news.
 */
export function linkedToast(wasPending: boolean, kind: WhereSignIn["kind"], screen: Screen | null): { toast: boolean; pending: boolean } {
  if (kind === "pending") return { toast: false, pending: true };
  if (kind === "linked") return { toast: wasPending && screen !== "where", pending: false };
  return { toast: false, pending: false };
}

/**
 * What going on from `step` keeps of what it showed: step 02 stores the
 * choice on screen, picked or not; step 03 the folder, the tool and the first
 * message (in the reader's words).
 */
export function settle(
  step: StepId,
  progress: SetupProgress,
  at: { online: boolean; tools: StepProps["tools"]; firstUi: FirstUi; t: (key: string) => string },
): SetupProgress {
  if (step === "where") return { ...progress, where: { ...progress.where, choice: whereChoice(progress, at.online) } };
  if (step === "first") return { ...progress, first: firstOf(at.firstUi, at.tools, at.t) ?? progress.first };
  return progress;
}

/**
 * The right end of a step's action bar (one primary per page). Step 01
 * goes on with "Continue" once a tool is ready — the primary only while no
 * card holds it (the first tool that needs the user does, `tools-model.ts`)
 * — else with "Continue without AI tools", never primary. Step 04 is
 * "Finish", or "Finish and open my first session" once step 03 saved a folder.
 * Step 02 with "Sign in to Droppen" chosen follows the sign-in: "Sign
 * in in the browser" until it starts, "Keep going; I'll finish in the
 * browser" while it is under way, "Continue" once this Mac is in the account,
 * and "Only on this Mac for now" (`go.local`) when it stopped.
 */
export function stepAction(
  step: StepId,
  progress: SetupProgress,
  tools: StepProps["tools"],
  t: (key: string) => string,
  go: { next: () => void; finish: () => void; local?: () => void },
  toolsUi: ToolsUi = IDLE_TOOLS_UI,
  firstUi: FirstUi = EMPTY_FIRST_UI,
  online = true,
  signIn?: GuideSignIn,
): StepAction | null {
  switch (step) {
    case "tools": {
      const anyReady = tools?.tools.some((tool) => tool.ready) ?? false;
      return anyReady
        ? { label: t("Continue"), onClick: go.next, primary: !cardsOwnPrimary(tools, toolsUi) }
        : { label: t("Continue without AI tools"), onClick: go.next, primary: false };
    }
    case "where": {
      if (!signIn || whereChoice(progress, online) !== "droppen") return { label: t("Continue"), onClick: go.next, primary: true };
      switch (signIn.state.kind) {
        case "none":
          return { label: t("Sign in in the browser"), onClick: signIn.start, primary: true };
        case "pending":
          return { label: t("Keep going; I'll finish in the browser"), onClick: go.next, primary: true };
        case "failed":
          return { label: t("Only on this Mac for now"), onClick: go.local ?? go.next, primary: true };
        default:
          return { label: t("Continue"), onClick: go.next, primary: true };
      }
    }
    case "first":
      // Nothing ready to run it: the step's own "Back to step 1" is the way on, or Skip.
      if (noToolReady(tools)) return null;
      return { label: t("Continue"), onClick: go.next, primary: true, disabled: firstOf(firstUi, tools, t) === null };
    case "wrap":
      return { label: opensFirstSession(progress) ? t("Finish and open my first session") : t("Finish"), onClick: go.finish, primary: true };
  }
}
