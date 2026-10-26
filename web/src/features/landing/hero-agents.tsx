import { type ReactNode, useRef } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { CipherChip, Ledger, ledgerH, LiveDot, Source, Stage, Wire } from "./kit";
import { chipOpacity, place, pointAt, type Pt, setAttr, setText, span, useLoop, useReducedMotion, useSize } from "./motion";

/*
 * The hero — "your agents keep working, and you keep up from your phone".
 * Two computers that run AI above (a MacBook, a Linux server), your phone
 * below, one straight wire down from each. The phone shows what the app's
 * 「需要你」 list shows: every session on both computers in one list, each
 * with what it is doing, and under it the one thing to do.
 *
 * One loop, the product's two everyday beats:
 * - the Claude session on the MacBook has stopped to ask (run the migration
 *   now?): the phone's composer is the question, you tap "yes", the answer
 *   goes up, and the session is working again on both ends;
 * - the Codex session on the Linux server is done (tests pass): you type the
 *   next prompt and send it, it goes up, and that session works on.
 * Then both come back: the Linux session finishes and the MacBook session
 * asks, each a packet down the wire, and the loop ends where it starts.
 *
 * What travels is a sealed packet (a padlock, no words): the page says
 * end-to-end encryption once, further down, not in the hero. Sizes are fixed;
 * a wider screen only leaves more grid around the figure.
 */

export const AGENTS_LOOP = 12;

/** When each thing happens, in seconds into the loop. */
export const AGENTS_T = {
  /** "yes" pressed on the question, and the answer's packet up to the MacBook. */
  tap: 1.4,
  answerUp: [2.0, 2.8],
  /** The next prompt for the Linux session: the composer, the typing, send pressed, its packet up. */
  prompt: 2.8,
  typing: [3.0, 4.6],
  send: 4.9,
  sendUp: [5.2, 6.0],
  /** The Linux session finishes, and the MacBook's asks: each a packet down to the phone. */
  doneDown: [7.6, 8.4],
  askDown: [9.2, 10.0],
} as const;
const T = AGENTS_T;

export type SessionState = "working" | "asking" | "done";
export type Panel = "question" | "prompt" | "idle";

export interface AgentsFrame {
  /** The MacBook's Claude session (`api`), as the MacBook and as the phone show it. */
  mac: SessionState;
  phoneMac: SessionState;
  /** The Linux server's Codex session (`infra`). */
  linux: SessionState;
  phoneLinux: SessionState;
  /** What the phone's composer shows: the question, the next prompt, or nothing waiting. */
  panel: Panel;
  /** The answer or the send button, held down from the tap until its packet leaves. */
  pressed: boolean;
  /** How much of the next prompt is typed (0 → 1). */
  typed: number;
  /** Packets on the wires: which computer's wire, up (to it) or down (to the phone), and how far along (0 → 1). */
  chips: { wire: "mac" | "linux"; up: boolean; at: number }[];
}

const within = (t: number, [a, b]: readonly [number, number]) => t >= a && t < b;

/** What the hero shows at `t` seconds into its loop. */
export function agentsFrame(t: number): AgentsFrame {
  const chips: AgentsFrame["chips"] = [];
  if (within(t, T.answerUp)) chips.push({ wire: "mac", up: true, at: span(t, ...T.answerUp) });
  if (within(t, T.sendUp)) chips.push({ wire: "linux", up: true, at: span(t, ...T.sendUp) });
  if (within(t, T.doneDown)) chips.push({ wire: "linux", up: false, at: span(t, ...T.doneDown) });
  if (within(t, T.askDown)) chips.push({ wire: "mac", up: false, at: span(t, ...T.askDown) });
  // A computer changes when a packet leaves it or reaches it; the phone when one reaches it or it sends one.
  const mac = t < T.answerUp[1] || t >= T.askDown[0] ? "asking" : "working";
  const phoneMac = t < T.answerUp[1] || t >= T.askDown[1] ? "asking" : "working";
  const linux = t < T.sendUp[1] || t >= T.doneDown[0] ? "done" : "working";
  const phoneLinux = t < T.sendUp[1] || t >= T.doneDown[1] ? "done" : "working";
  const panel: Panel = phoneMac === "asking" ? "question" : t >= T.prompt && t < T.sendUp[0] ? "prompt" : "idle";
  const pressed = (t >= T.tap && t < T.answerUp[0]) || (t >= T.send && t < T.sendUp[0]);
  const typed = panel === "prompt" ? span(t, ...T.typing) : 0;
  return { mac, phoneMac, linux, phoneLinux, panel, pressed, typed, chips };
}

/** The still frame (reduced motion, and the first paint): the question waiting and the other session done, as the loop starts. */
export function agentsStill(): AgentsFrame {
  return agentsFrame(0);
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface AgentsLayout {
  h: number;
  /** Below 480px: the computers' rows show a state's colour, not its words. */
  narrow: boolean;
  mac: Box;
  linux: Box;
  phone: Box;
  /** Top to bottom: out of each computer, straight down into the phone. */
  macWire: Pt[];
  linuxWire: Pt[];
}

/** The phone's composer under its list: a session's label, then the question and its answers, or the prompt and send. */
export const PANEL_H = 74;
const COMPUTER_ROWS = 2;
const PHONE_ROWS = 4;

export function agentsLayout(w: number): AgentsLayout {
  const compH = ledgerH(COMPUTER_ROWS);
  const phoneH = ledgerH(PHONE_ROWS) + PANEL_H + 1;
  const narrow = w < 480;
  // Narrow: the two computers side by side over the phone, as wide as the column; else fixed sizes, centred.
  const colW = narrow ? Math.min(w - 24, 360) : 452;
  const compW = narrow ? Math.floor((colW - 12) / 2) : 208;
  const phoneW = narrow ? colW : 340;
  const x0 = Math.round((w - colW) / 2);
  const top = narrow ? 16 : 32;
  const mac = { x: x0, y: top, w: compW, h: compH };
  const linux = { x: x0 + colW - compW, y: top, w: compW, h: compH };
  const phone = { x: x0 + Math.round((colW - phoneW) / 2), y: top + compH + (narrow ? 36 : 44), w: phoneW, h: phoneH };
  // Each wire drops from inside its computer to inside the phone: under the computer's middle when narrow, else an inset from the phone's sides.
  const macX = narrow ? Math.round(mac.x + compW / 2) : phone.x + 56;
  const linuxX = narrow ? Math.round(linux.x + compW / 2) : phone.x + phoneW - 56;
  return {
    h: phone.y + phoneH + top,
    narrow,
    mac,
    linux,
    phone,
    macWire: [{ x: macX, y: mac.y + compH }, { x: macX, y: phone.y }],
    linuxWire: [{ x: linuxX, y: linux.y + compH }, { x: linuxX, y: phone.y }],
  };
}

export function agentsWires(L: AgentsLayout): Pt[][] {
  return [L.macWire, L.linuxWire];
}

const STATES: readonly SessionState[] = ["working", "asking", "done"];

/** A session's state at the end of its row: every state's words in one grid cell, only the current one visible, so nothing moves. */
function StateCell({ refs, first, words, bare }: { refs?: Record<SessionState, { current: HTMLSpanElement | null }>; first: SessionState; words: Record<SessionState, ReactNode>; bare?: boolean }) {
  const tone: Record<SessionState, string> = { working: "text-ok", asking: "text-warn-fg", done: "text-fg" };
  return (
    <span className="ml-auto grid shrink-0 justify-items-end">
      {STATES.map((s) => (
        <span
          key={s}
          ref={refs ? (el) => void (refs[s].current = el) : undefined}
          data-on={s === first ? "true" : "false"}
          data-signal={s === "asking" ? "warn" : s === "working" ? "working" : undefined}
          className={cn("col-start-1 row-start-1 flex items-center gap-1 data-[on=false]:invisible", tone[s])}
        >
          {/* The app's marks: a green dot while working, an amber square while asking, an ink one for news. */}
          {s === "working" ? <LiveDot /> : <span className={cn("size-1.5 shrink-0", s === "asking" ? "bg-warn" : "bg-fg")} />}
          {!bare && words[s]}
        </span>
      ))}
    </span>
  );
}

const useStateRefs = () => ({ working: useRef<HTMLSpanElement>(null), asking: useRef<HTMLSpanElement>(null), done: useRef<HTMLSpanElement>(null) });

function drawState(refs: ReturnType<typeof useStateRefs>, state: SessionState) {
  for (const s of STATES) setAttr(refs[s].current, "data-on", s === state ? "true" : "false");
}

/** A session row: its CLI, its name (a folder, never translated), where it runs (on the phone), and its state. */
function SessionRow({ cli, name, where, children }: { cli: "CLAUDE" | "CODEX"; name: string; where?: string; children: ReactNode }) {
  return (
    <>
      <Source>{cli}</Source>
      <span className="text-fg">{name}</span>
      {where && <span className="truncate text-faint">{where}</span>}
      {children}
    </>
  );
}

export function HeroAgents() {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const { w, h: boxH } = useSize(box, 560);
  const L = agentsLayout(w);
  const reduced = useReducedMotion();
  const first = agentsStill();
  const prompt = t("now deploy it to staging");

  const mac = useStateRefs();
  const linux = useStateRefs();
  const phoneMac = useStateRefs();
  const phoneLinux = useStateRefs();
  const panels = { question: useRef<HTMLDivElement>(null), prompt: useRef<HTMLDivElement>(null), idle: useRef<HTMLDivElement>(null) };
  const yes = useRef<HTMLSpanElement>(null);
  const send = useRef<HTMLSpanElement>(null);
  const typed = useRef<HTMLSpanElement>(null);
  const macChip = useRef<SVGGElement>(null);
  const linuxChip = useRef<SVGGElement>(null);

  const draw = (f: AgentsFrame) => {
    drawState(mac, f.mac);
    drawState(linux, f.linux);
    drawState(phoneMac, f.phoneMac);
    drawState(phoneLinux, f.phoneLinux);
    for (const p of ["question", "prompt", "idle"] as const) setAttr(panels[p].current, "data-on", f.panel === p ? "true" : "false");
    setAttr(yes.current, "data-pressed", f.pressed && f.panel === "question" ? "true" : "false");
    setAttr(send.current, "data-pressed", f.pressed && f.panel === "prompt" ? "true" : "false");
    const chars = [...prompt];
    setText(typed.current, chars.slice(0, Math.round(f.typed * chars.length)).join(""));
    setAttr(typed.current, "data-caret", f.panel === "prompt" && !f.pressed ? "true" : "false");
    for (const [wire, el, points] of [
      ["mac", macChip.current, L.macWire],
      ["linux", linuxChip.current, L.linuxWire],
    ] as const) {
      const chip = f.chips.find((c) => c.wire === wire);
      place(el, chip ? pointAt(points, chip.up ? 1 - chip.at : chip.at) : null, chip ? chipOpacity(chip.at) : 0);
    }
  };

  useLoop(box, AGENTS_LOOP, { frame: (time) => draw(agentsFrame(time)), still: () => draw(agentsStill()) }, reduced, [w, t]);

  const words: Record<SessionState, ReactNode> = { working: t("working"), asking: t("asking you"), done: t("done") };
  const phoneWords: Record<SessionState, ReactNode> = { ...words, done: t("done, tests pass") };
  const on = (value: boolean) => (value ? "true" : "false");
  const button = "border px-2 font-mono text-[11px] leading-[18px] whitespace-nowrap transition-colors data-[pressed=true]:border-fg data-[pressed=true]:bg-fg data-[pressed=true]:text-bg";
  return (
    <Stage
      name="hero"
      ref={box}
      width={w}
      height={Math.max(boxH, L.h)}
      drawn={L.h}
      className="h-full"
      grid={22}
      svg={
        <>
          <Wire points={L.macWire} weight="hair" />
          <Wire points={L.linuxWire} weight="hair" />
        </>
      }
      over={
        <>
          <CipherChip ref={macChip} />
          <CipherChip ref={linuxChip} />
        </>
      }
    >
      <Ledger
        box={L.mac}
        head="MacBook"
        status={<LiveDot />}
        rows={[
          <SessionRow key="api" cli="CLAUDE" name="api">
            <StateCell refs={mac} first={first.mac} words={words} bare={L.narrow} />
          </SessionRow>,
          <SessionRow key="docs" cli="CLAUDE" name="docs">
            <StateCell first="working" words={words} bare={L.narrow} />
          </SessionRow>,
        ]}
      />
      <Ledger
        box={L.linux}
        head={t("Linux server")}
        status={<LiveDot />}
        rows={[
          <SessionRow key="infra" cli="CODEX" name="infra">
            <StateCell refs={linux} first={first.linux} words={words} bare={L.narrow} />
          </SessionRow>,
          <SessionRow key="ci" cli="CODEX" name="ci">
            <StateCell first="working" words={words} bare={L.narrow} />
          </SessionRow>,
        ]}
      />
      <Ledger
        box={L.phone}
        head={t("your phone")}
        status={<LiveDot />}
        rows={[
          <SessionRow key="api" cli="CLAUDE" name="api" where="MacBook">
            <StateCell refs={phoneMac} first={first.phoneMac} words={phoneWords} />
          </SessionRow>,
          <SessionRow key="infra" cli="CODEX" name="infra" where={t("Linux server")}>
            <StateCell refs={phoneLinux} first={first.phoneLinux} words={phoneWords} />
          </SessionRow>,
          <SessionRow key="docs" cli="CLAUDE" name="docs" where="MacBook">
            <StateCell first="working" words={phoneWords} />
          </SessionRow>,
          <SessionRow key="ci" cli="CODEX" name="ci" where={t("Linux server")}>
            <StateCell first="working" words={phoneWords} />
          </SessionRow>,
        ]}
      >
        {/* The composer: three faces in one cell, one shown at a time. */}
        <div className="grid h-full">
          <div ref={panels.question} data-on={on(first.panel === "question")} className="col-start-1 row-start-1 flex flex-col gap-1.5 bg-warn/7 px-2.5 py-2 shadow-[inset_2px_0_0_var(--color-warn)] data-[on=false]:invisible">
            <span className="flex items-center gap-2 font-mono text-[10px] leading-[14px]">
              <span data-signal="warn" className="inline-flex h-3.5 items-center bg-warn px-1 font-semibold text-on-signal">
                {t("waiting for your answer")}
              </span>
              <span className="truncate text-faint">api · MacBook</span>
            </span>
            <span className="text-xs leading-4 text-fg">{t("Run the migration now?")}</span>
            <span className="flex gap-1.5">
              <span ref={yes} data-pressed={on(first.pressed && first.panel === "question")} className={cn(button, "border-border-strong text-fg")}>
                {t("yes, run it")}
              </span>
              <span className={cn(button, "border-border text-muted")}>{t("not now")}</span>
            </span>
          </div>
          <div ref={panels.prompt} data-on={on(first.panel === "prompt")} className="col-start-1 row-start-1 flex flex-col justify-center gap-1.5 px-2.5 py-2 data-[on=false]:invisible">
            <span className="truncate font-mono text-[10px] leading-[14px] text-faint">infra · {t("Linux server")}</span>
            <span className="flex items-stretch gap-1.5">
              <span className="flex h-7 min-w-0 flex-1 items-center border border-border-strong px-2 text-xs text-fg">
                <span
                  ref={typed}
                  data-caret={on(first.panel === "prompt")}
                  className="truncate whitespace-pre data-[caret=true]:bg-[linear-gradient(to_left,currentColor_6px,transparent_6px)] data-[caret=true]:pr-[8px]"
                />
              </span>
              <span ref={send} data-pressed="false" className={cn(button, "flex items-center border-border-strong text-fg")}>
                {t("send")}
              </span>
            </span>
          </div>
          <div ref={panels.idle} data-on={on(first.panel === "idle")} className="col-start-1 row-start-1 flex items-center px-2.5 data-[on=false]:invisible">
            <span className="font-mono text-[11px] text-faint">{t("nothing is asking you")}</span>
          </div>
        </div>
      </Ledger>
    </Stage>
  );
}
