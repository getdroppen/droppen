/*
 * Step 03 · First session: one folder
 * for the AI to work in — used recently with Claude Code or Codex, a practice
 * folder the guide makes, or any other through the empty-session view's own
 * folder browser —
 * the tool it runs with (only the ready ones), and a first message: two that
 * only ask it to look in a project, one that builds something in the practice
 * folder, or the user's own. Continue stores them; the guide's Finish opens
 * the empty-session view with them filled in (`first-session.ts`), where only
 * the user's Send starts anything.
 */
import { Check, FolderPlus, Folder, Search, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSettings } from "@/app/global-feed";
import { Button } from "@/components/ui/button";
import { DirBrowser } from "@/features/sessions/dir-browser";
import { cn } from "@/lib/cn";
import { basename, shortenHome } from "@/lib/format";
import { usePracticeFolder, useRecentFolders } from "./api";
import { FootNote } from "./frame";
import { TOOL_NAME } from "./model";
import type { StepProps } from "./screens";
import {
  agentOf,
  ago,
  EMPTY_FIRST_UI,
  type FirstUi,
  noToolReady,
  PROMPTS,
  promptChoices,
  promptOf,
  protectedFolder,
  readyAgents,
} from "./first-model";
import { Segmented } from "./tools-parts";
import type { RecentFolder, StepId, ToolsReport } from "./types";

/** What step 03 is given besides {@link StepProps}: its picks, kept by the guide, and a way back to step 01. */
export interface FirstStepUi {
  firstUi?: FirstUi;
  onFirstUi?: (next: FirstUi) => void;
  onGoto?: (step: StepId) => void;
}

const SOURCE_NAME: Record<RecentFolder["tools"][number], string> = { droppen: "Droppen", claude: "Claude Code", codex: "Codex" };

/** One folder to pick: its name, where it is, a note on the right, ticked when picked. */
function FolderRow({
  icon,
  name,
  detail,
  meta,
  checked,
  disabled,
  onPick,
  path,
  testId,
}: {
  testId?: string;
  icon: ReactNode;
  name: string;
  detail: string | null;
  meta?: string;
  checked: boolean;
  disabled?: boolean;
  onPick: () => void;
  path?: string;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={disabled}
      data-folder={path}
      data-testid={testId}
      onClick={onPick}
      className={cn(
        "grid w-full grid-cols-[16px_minmax(0,1fr)_auto_14px] items-center gap-x-3 border-b border-border px-3.5 py-2.5 text-left last:border-b-0 disabled:opacity-50",
        checked ? "bg-hover" : "hover:bg-hover",
      )}
    >
      <span className="text-muted [&_svg]:size-4">{icon}</span>
      <span className="min-w-0">
        <span className="block truncate text-[13px] leading-[18px]">{name}</span>
        {detail && <span className="block truncate font-mono text-2xs leading-4 text-faint">{detail}</span>}
      </span>
      <span className="truncate font-mono text-2xs text-faint">{meta}</span>
      <span>{checked && <Check className="size-3.5" aria-hidden="true" />}</span>
    </button>
  );
}

function Section({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div className="mb-4 border border-border">
      {title && (
        <h2 data-label="" className="label-caps flex h-8 items-center gap-2 border-b border-border px-3.5 text-2xs text-muted">
          <span className="min-w-0 flex-1 truncate">{title}</span>
          {aside}
        </h2>
      )}
      {children}
    </div>
  );
}

/** Step 03 with nothing ready to run it: say so, and the way back to step 01; Skip stays in the bar. */
function NoTool({ onGoto }: { onGoto: (step: StepId) => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="first-no-tool" className="grid max-w-[720px] grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-3 border border-border px-4 py-3.5 max-[720px]:grid-cols-[16px_minmax(0,1fr)]">
      <TriangleAlert className="size-4 text-warn-fg" aria-hidden="true" />
      <p className="text-xs leading-[18px]">{t("No AI tool is ready yet, so there is nothing to start. Get one ready in step 1, or skip this step.")}</p>
      <Button variant="primary" data-guide-primary="" data-testid="first-back" className="max-[720px]:col-start-2 max-[720px]:justify-self-start" onClick={() => onGoto("tools")}>
        {t("Back to step 1")}
      </Button>
    </div>
  );
}

/** Step 03 as drawn, given everything (the render tests draw it directly). */
export function FirstStepView({
  report,
  ui,
  onUi,
  recent,
  onPractice,
  practice,
  homeDir,
  onGoto,
}: {
  report: ToolsReport | null | undefined;
  ui: FirstUi;
  onUi: (next: FirstUi) => void;
  recent: RecentFolder[];
  /** Make the practice folder (or take the one made already). */
  onPractice: () => void;
  practice: { state: "idle" | "making" | "failed"; path: string | null };
  homeDir: string;
  onGoto: (step: StepId) => void;
}) {
  const { t, i18n } = useTranslation();
  if (noToolReady(report)) return <NoTool onGoto={onGoto} />;
  const agents = readyAgents(report);
  const agent = agentOf(ui, report);
  const prompt = promptOf(ui);
  const pick = (cwd: string, practiceFolder = false) => onUi({ ...ui, cwd, practice: practiceFolder, browsing: false });
  const short = (path: string) => shortenHome(path, homeDir);
  const listed = new Set([...recent.map((f) => f.path), ...(practice.path ? [practice.path] : [])]);
  const other = ui.cwd && !listed.has(ui.cwd) ? ui.cwd : null;
  return (
    <div data-testid="first-step" className="max-w-[760px]">
      <div role="radiogroup" aria-label={t("Pick a folder for the AI to work in")}>
        {recent.length > 0 && (
          <Section title={t("Used recently with Claude Code or Codex")}>
            {recent.map((folder) => (
              <FolderRow
                key={folder.path}
                path={folder.path}
                icon={<Folder />}
                name={folder.name || basename(folder.path)}
                detail={short(folder.path)}
                meta={[...folder.tools.map((s) => SOURCE_NAME[s]), ago(folder.lastUsed, i18n.language)].filter(Boolean).join(" · ")}
                checked={ui.cwd === folder.path}
                onPick={() => pick(folder.path)}
              />
            ))}
          </Section>
        )}
        <Section title="">
          <FolderRow
            testId="practice-row"
            path={practice.path ?? undefined}
            icon={<FolderPlus />}
            name={t("Make a practice folder")}
            detail={practice.path ? short(practice.path) : t("~/Droppen/first-session: empty, safe to experiment in.")}
            checked={ui.practice && ui.cwd !== null && ui.cwd === practice.path}
            disabled={practice.state === "making"}
            onPick={() => (practice.path ? pick(practice.path, true) : onPractice())}
          />
          <FolderRow
            testId="other-row"
            path={other ?? undefined}
            icon={<Search />}
            name={t("Choose another folder…")}
            detail={other ? short(other) : null}
            checked={other !== null}
            onPick={() => onUi({ ...ui, browsing: !ui.browsing })}
          />
          {ui.browsing && (
            <div className="border-t border-border p-2">
              <DirBrowser start={ui.cwd ?? homeDir} onPick={(path) => pick(path)} />
            </div>
          )}
        </Section>
      </div>
      {practice.state === "failed" && (
        <p data-signal="failed" role="alert" className="-mt-2 mb-3 text-2xs text-danger">
          {t("That didn't work. Try again.")}
        </p>
      )}
      {protectedFolder(ui.cwd, homeDir) && (
        <FootNote className="-mt-1.5 mb-4">
          <span data-testid="first-tcc">{t("Folders in Documents, Desktop or Downloads: macOS asks once whether Droppen may open them. Choose Allow.")}</span>
        </FootNote>
      )}
      <div className="mt-5 mb-2 flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="text-xs font-semibold">{t("First message")}</span>
        <span className="ml-auto font-mono text-2xs text-muted">{t("Run with")}</span>
        {agent && <Segmented label={t("Run with")} value={agent} options={agents.map((id) => ({ value: id, label: TOOL_NAME[id] }))} onChange={(id) => onUi({ ...ui, agent: id })} />}
      </div>
      <div role="radiogroup" aria-label={t("First message")} className="flex flex-col">
        {promptChoices(ui.practice).map((choice) => {
          const on = choice === prompt;
          return (
            <button
              key={choice}
              type="button"
              role="radio"
              aria-checked={on}
              data-prompt={choice}
              onClick={() => onUi({ ...ui, prompt: choice })}
              className={cn("-mt-px grid grid-cols-[13px_minmax(0,1fr)] items-center gap-x-3 border px-3.5 py-2.5 text-left first:mt-0", on ? "relative z-[1] border-fg" : "border-border hover:bg-hover")}
            >
              <span aria-hidden="true" className={cn("flex size-[13px] items-center justify-center rounded-full border", on ? "border-fg" : "border-border-strong")}>
                {on && <span className="size-[7px] rounded-full bg-fg" />}
              </span>
              <span className="text-xs leading-[18px]">{choice === "own" ? t("Write my own") : t(PROMPTS[choice])}</span>
            </button>
          );
        })}
      </div>
      {prompt === "own" && (
        <textarea
          data-testid="first-own"
          autoFocus
          rows={3}
          value={ui.own}
          aria-label={t("First message")}
          onChange={(e) => onUi({ ...ui, own: e.target.value })}
          className="mt-2 w-full resize-y border border-border bg-bg px-3 py-2 text-xs leading-[18px] text-fg focus:border-fg focus:outline-none"
        />
      )}
      <FootNote>{t("You send it yourself on the next screen; nothing runs until you press Send.")}</FootNote>
    </div>
  );
}

/**
 * Step 03, live: the recent folders asked as the step opens (folder names
 * only, from the session lists),
 * the practice folder made when it is picked.
 */
export function FirstStep({ tools, firstUi, onFirstUi, onGoto = () => undefined }: StepProps & FirstStepUi) {
  const [localUi, setLocalUi] = useState<FirstUi>(firstUi ?? EMPTY_FIRST_UI);
  const ui = onFirstUi ? (firstUi ?? EMPTY_FIRST_UI) : localUi;
  const setUi = onFirstUi ?? setLocalUi;
  const homeDir = useFeedSettings()?.homeDir ?? "";
  const enabled = !noToolReady(tools);
  const recent = useRecentFolders(enabled).data ?? [];
  const practice = usePracticeFolder();
  const practicePath = practice.data ?? (ui.practice ? ui.cwd : null);
  return (
    <FirstStepView
      report={tools}
      ui={ui}
      onUi={setUi}
      recent={recent}
      practice={{ state: practice.isPending ? "making" : practice.isError ? "failed" : "idle", path: practicePath }}
      onPractice={() => practice.mutate(undefined, { onSuccess: (path) => setUi({ ...ui, cwd: path, practice: true, browsing: false }) })}
      homeDir={homeDir}
      onGoto={onGoto}
    />
  );
}
