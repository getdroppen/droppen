import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import { useShowingCopy } from "@/features/cloud/context";
import { isTerminalGoneFrame, RECONNECT_STABLE_MS, reconnectDelay } from "@/features/terminals/reconnect";
import { useEnsureLive, useMachineRuntime } from "@/lib/machines/context";
import type { SocketLike } from "@/lib/transport";
import { noteWsFailure } from "@/lib/ws-url";
import { kbMode } from "@/lib/kb-mode";
import { holdInView } from "@/lib/kb-native";
import "@xterm/xterm/css/xterm.css";

/** Terminals stay dark in both themes, like most embedded terminals. */
const XTERM_THEME = {
  background: "#0f1115",
  foreground: "#e9ebee",
  cursor: "#e9ebee",
  cursorAccent: "#0f1115",
  // Inverted ink, like the app's own text selection.
  selectionBackground: "#5c6470",
  green: "#3ed273",
  brightGreen: "#6ae397",
  yellow: "#f4b13a",
  brightYellow: "#ffc95c",
  red: "#ff665b",
  brightRed: "#ff8a80",
};

/** What the pane's socket is doing, as far as the user needs to care. */
type LinkState = "connecting" | "attached" | "reconnecting" | "gone";

export function XtermPane({ id, visible }: { id: string; visible: boolean }) {
  const { t: tr } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [link, setLink] = useState<LinkState>("connecting");
  // In cloud mode with the machine away the page runs on its offline copy,
  // and a terminal is the one thing a copy cannot be: say that, rather than
  // an endless "reconnecting…".
  const machineAway = useShowingCopy();
  const runtime = useMachineRuntime();
  // A terminal is the machine itself in use: live, never its copy.
  useEnsureLive();

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    setLink("connecting");

    const term = new Terminal({
      fontSize: 12,
      fontFamily: '"SF Mono", ui-monospace, Menlo, monospace',
      theme: XTERM_THEME,
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    term.open(el);
    // WebGL renders heavy output far smoother; on any failure (headless GPU,
    // context limit, addon/core mismatch) the DOM renderer takes over.
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // DOM renderer fallback
    }
    fit.fit();
    // The phone keyboard: WebKit never reveals xterm's transparent textarea,
    // so while the terminal has focus it ends where the screen does and the
    // observer below refits it (`lib/kb-native.ts`). Under `?kb=legacy` the
    // whole shell is sized above the keyboard instead.
    const unhold = kbMode() === "native" ? holdInView(el) : () => {};

    let socket: SocketLike | null = null;
    let disposed = false;
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    /** Set by the first successful attach: a later one is re-attaching. */
    let attachedBefore = false;

    const send = (message: unknown) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
    };

    const connect = () => {
      // A login-protected server wants a one-shot ticket in the URL, which is a
      // round-trip; the pane may be gone by the time it lands. Through the
      // machine's transport as it is at each attempt (`lib/machines`).
      void runtime.transport.socketUrl(`/api/terminals/${id}/ws`).then(attach, () => {
        // No ticket, no attach: keep the same escalating backoff.
        if (disposed) return;
        setLink("reconnecting");
        timer = setTimeout(connect, reconnectDelay(attempt++));
      });
    };

    const attach = (url: string) => {
      if (disposed) return;
      let ws: ReturnType<typeof runtime.transport.openSocket>;
      try {
        ws = runtime.transport.openSocket(url);
      } catch {
        // No socket to be had (the machine closed on the page as this
        // landed): a failed attempt, not an error left unhandled.
        setLink("reconnecting");
        timer = setTimeout(connect, reconnectDelay(attempt++));
        return;
      }
      socket = ws;
      let firstFrame = true;
      let gone = false;
      let openedAt = 0;
      // A re-attach opens with a full-screen repaint, so the rows left over from
      // the dropped connection have to go — but not before the repaint arrives,
      // or the pane blinks empty for the whole outage. The first attach paints
      // into an already empty terminal and needs no reset at all.
      //
      // What makes "not before the repaint arrives" safe is that the server
      // sends a frame on every attach, empty replay and all
      // (`server-rs/src/terminals/routes.rs`). Without that promise a terminal
      // with nothing buffered left this armed until the user's next keystroke
      // produced output, and the reset then wiped a screen they were reading.
      let pendingReset = attachedBefore;

      ws.onopen = () => {
        openedAt = Date.now();
        attachedBefore = true;
        setLink("attached");
        // Re-assert the grid: the pane may have been resized while the socket
        // was down, and the server sizes the screen it repaints from this.
        ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      };
      ws.onmessage = (e) => {
        if (typeof e.data !== "string") return;
        if (firstFrame) {
          firstFrame = false;
          if (isTerminalGoneFrame(e.data)) {
            // End of the line for this terminal. Leave the last output on
            // screen — unreset — and let the note land under it.
            gone = true;
            setLink("gone");
            term.write(e.data);
            return;
          }
        }
        if (pendingReset) {
          pendingReset = false;
          term.reset();
        }
        term.write(e.data);
      };
      ws.onclose = () => {
        if (disposed || socket !== ws) return;
        socket = null;
        if (gone) return;
        // An attach that held for a while earns a fresh backoff; one that was
        // dropped straight away keeps escalating.
        if (openedAt > 0 && Date.now() - openedAt >= RECONNECT_STABLE_MS) attempt = 0;
        setLink("reconnecting");
        // A handshake refused for want of a ticket looks exactly like this, so
        // a run of them asks whether this browser is still logged in.
        noteWsFailure(attempt + 1, runtime.key);
        timer = setTimeout(connect, reconnectDelay(attempt++));
      };
    };
    connect();

    const dataSub = term.onData((data) => send({ type: "input", data }));
    const resizeSub = term.onResize(({ cols, rows }) => send({ type: "resize", cols, rows }));

    // Coalesce refit into one per frame; split-drag fires the observer rapidly.
    let raf = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (el.offsetParent !== null) fit.fit();
      });
    });
    observer.observe(el);

    return () => {
      disposed = true;
      clearTimeout(timer);
      observer.disconnect();
      cancelAnimationFrame(raf);
      dataSub.dispose();
      resizeSub.dispose();
      socket?.close();
      unhold();
      term.dispose();
    };
  }, [id, runtime]);

  useEffect(() => {
    if (visible) {
      // The pane just became visible; measurements were 0 while hidden.
      requestAnimationFrame(() => fitRef.current?.fit());
    }
  }, [visible]);

  const note =
    machineAway && link !== "attached"
      ? tr("the device is offline — terminals need it")
      : link === "reconnecting"
        ? tr("reconnecting…")
        : link === "gone"
          ? tr("terminal not found")
          : null;

  return (
    // The padding lives on the wrapper: FitAddon sizes rows from the mount
    // element's full height, so padding there would overcount by a row and
    // clip the bottom line.
    <div className={visible ? "relative h-full w-full bg-[#0f1115] p-1.5" : "hidden"}>
      <div ref={containerRef} className="h-full w-full" />
      {note && (
        // Hard-coded colors, like the theme above: this badge sits on the
        // terminal, which is dark in both app themes.
        <div className="pointer-events-none absolute right-3 top-2 flex items-center gap-1.5 border border-[#5c6470] bg-[#0f1115] px-1.5 py-0.5 font-mono text-2xs text-[#a4abb5]">
          {/* "gone" is a settled fact and sits still; "reconnecting" is work in
              progress, so it gets the 1 Hz heartbeat instead of an always-on
              CSS pulse — a terminal left open on a flapping link would
              otherwise repaint the whole screen at the refresh rate all day. */}
          {link === "gone" ? (
            <span data-shape="dot" className="size-1.5 rounded-full bg-[#ff5c50]" />
          ) : (
            <LiveDot className="bg-[#f5b031]" />
          )}
          {note}
        </div>
      )}
    </div>
  );
}
