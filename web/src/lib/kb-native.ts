/**
 * The soft keyboard, left to the browser.
 *
 * The shell is pinned (`body { position: fixed; inset: 0 }`, index.css) and is
 * never resized for a keyboard here. Android shrinks the layout viewport for
 * one (the viewport meta's `interactive-widget=resizes-content`), and the shell
 * follows it up. iOS WebKit shrinks only the visual viewport, then pans it down
 * the layout viewport to reveal the focused field: the composer, the shell's
 * foot, ends up on top of the keyboard, and the header is off the top of the
 * screen until the keyboard goes. WebKit makes that pan even though the root
 * has nothing to scroll: the iOS 26.5 Simulator does it in one step, and the
 * owner's iOS 27 phone animates it.
 * The shell is never sized from `visualViewport` or `innerHeight` here, no
 * keyboard is ever detected, and nothing scrolls the page while a field has
 * focus. The one field WebKit does not reveal is the terminal's (`holdInView`).
 *
 * Measuring the keyboard instead is what broke on iOS 27. That code took the
 * keyboard to be the gap between `innerHeight` (or the fixed box) and the
 * visual viewport. While the page is
 * panned, `innerHeight` follows the visual viewport down (iOS 26 too). Where
 * the pan lands in one step the gap never reads wrong. iOS 27 animates it over
 * several frames, and `innerHeight` shrinks along with it. So before the
 * first fix the gap "closed" partway up, and the leftover-pan reset then
 * scrolled the page back to the top with the keyboard still up. That put the
 * composer under the keyboard. After that fix the shell fell back to full
 * height partway up. Then, after a dismissal, iOS 27 kept the keyboard's
 * numbers and the shell stayed a keyboard short.
 *
 * One thing is left to do here. With no field focused there is no keyboard,
 * and the root, which has nothing to scroll, belongs at the top. (Focus in an
 * embedded app's frame counts as a field: a field in it may have the keyboard.) Once the
 * keyboard is dismissed, iOS 27 can leave its pan behind: `visualViewport`,
 * `innerHeight` and `scrollY` keep their keyboard-up values and no event
 * follows (WebKit bug 322900). The pinned shell then
 * sits a keyboard high over an empty strip. So once focus has stayed off every
 * field for `RESET_MS[0]`, and again over the next second, a root that is still
 * scrolled is scrolled back to the top. The same happens whenever the viewport
 * moves with no field focused, or the page comes back from the background, and
 * within `WATCH_MS` of a focused field leaving the page (WebKit sends no
 * `focusout` for that).
 * Pinch-zoom (a scale above 1) is the reader's own pan, and is left alone.
 *
 * Where WebKit puts the pan back itself (iOS 26: it is back within a frame or
 * two of the dismissal), there is nothing left to do, and nothing is written.
 * On desktop and Android the root never scrolls, so this is never more than a
 * read of `scrollY`.
 */

/** A visual viewport scaled past this is pinch-zoomed: its pan is the reader's. */
const ZOOM_SCALE = 1.05;
/**
 * When the root is checked after focus leaves every field (or the viewport
 * moves with none focused). The first check is long enough that focus moving
 * on to the next field, or put back by a script, does not count. The later
 * ones cover iOS animating the keyboard out (about 250ms) and reporting its
 * last numbers after that.
 */
export const RESET_MS = [100, 300, 600, 1000] as const;
/**
 * How often the root is looked at again while a field has focus and the page
 * is panned to it. A field taken out of the page (the composer of a view that
 * navigated away on send) loses focus with no `focusout` in WebKit, and on iOS
 * 27 the keyboard's pan can then be left with no event at all.
 */
export const WATCH_MS = 500;

/** What the watch reads and writes; the browser's own in `startKbNative()`, fakes in tests. */
export interface NativeKbEnv {
  vv: Pick<VisualViewport, "offsetTop" | "scale" | "addEventListener" | "removeEventListener"> | null;
  win: Pick<Window, "scrollY" | "scrollTo" | "setTimeout" | "clearTimeout" | "addEventListener" | "removeEventListener">;
  doc: Pick<Document, "addEventListener" | "removeEventListener">;
  focused: () => Element | null;
}

/** A field that brings up the soft keyboard (or might: an `<input>` of any type). */
function isEditable(el: Element): el is HTMLElement {
  const h = el as HTMLElement;
  return h.tagName === "TEXTAREA" || h.tagName === "INPUT" || h.tagName === "SELECT" || h.isContentEditable === true;
}

function browserEnv(): NativeKbEnv {
  return {
    vv: window.visualViewport ?? null,
    win: window,
    doc: document,
    focused: () => document.activeElement,
  };
}

/** Start keeping the root at the top whenever no field has focus; returns the stop (tests; the app never stops). */
export function startKbNative(env: NativeKbEnv = browserEnv()): () => void {
  const { vv, win, doc, focused } = env;
  let timers: number[] = [];
  let watchTimer: number | undefined;

  // A field of this page, or anything in an embedded app's frame (code-server
  // and the like, `features/apps/app-frames.tsx`): this document sees only
  // the IFRAME as focused, whatever field inside it has the keyboard up.
  const editing = () => {
    const field = focused();
    return field !== null && (field.tagName === "IFRAME" || isEditable(field));
  };

  const check = () => {
    if ((vv?.scale ?? 1) > ZOOM_SCALE) return;
    if (Math.abs(win.scrollY) < 1 && Math.abs(vv?.offsetTop ?? 0) < 1) return;
    if (editing()) {
      // A focused field may have a keyboard on its way up, and the pan is
      // WebKit's reveal of it: a scroll now is taken by iOS as a "scroll to
      // dismiss", or undoes the reveal. Look again later, for a field that
      // goes without a `focusout`.
      watchTimer ??= win.setTimeout(() => {
        watchTimer = undefined;
        check();
      }, WATCH_MS);
      return;
    }
    win.scrollTo(0, 0);
  };

  // Each call starts the checks over, so a run of events (the keyboard's
  // numbers moving frame by frame on its way out) is checked once it is over.
  // Focus is only looked at when a check runs: during `focusout` it is
  // nowhere yet, whether or not it is about to land on the next field.
  const later = () => {
    for (const timer of timers) win.clearTimeout(timer);
    timers = RESET_MS.map((ms) => win.setTimeout(check, ms));
  };

  vv?.addEventListener("resize", later);
  vv?.addEventListener("scroll", later);
  win.addEventListener("scroll", later);
  win.addEventListener("pageshow", later);
  doc.addEventListener("visibilitychange", later);
  doc.addEventListener("focusout", later);
  later();
  return () => {
    vv?.removeEventListener("resize", later);
    vv?.removeEventListener("scroll", later);
    win.removeEventListener("scroll", later);
    win.removeEventListener("pageshow", later);
    doc.removeEventListener("visibilitychange", later);
    doc.removeEventListener("focusout", later);
    for (const timer of timers) win.clearTimeout(timer);
    if (watchTimer !== undefined) win.clearTimeout(watchTimer);
  };
}

/** The shortest the held box is made: a couple of terminal rows, even with no room. */
const HOLD_MIN_PX = 48;

/** What `holdInView` reads and writes; the browser's own by default, fakes in tests. */
export interface HoldEnv {
  vv: Pick<VisualViewport, "height" | "offsetTop" | "scale" | "addEventListener" | "removeEventListener"> | null;
  win: Pick<Window, "addEventListener" | "removeEventListener">;
  /** The pinned shell (`document.body`): where the layout viewport's top is, in client coordinates. */
  shell: Pick<HTMLElement, "getBoundingClientRect">;
}

/**
 * Keep the bottom of `el` on screen while focus is inside it, for the one
 * field WebKit will not reveal: xterm's, a transparent textarea at the cursor.
 * WebKit by design neither shows its own caret for a transparent field nor
 * scrolls one into view (an editor typing into a hidden field draws its own
 * caret), so with the keyboard up the terminal's cursor row would stay under
 * the keyboard. While focus is inside `el`, its `max-height` ends it at the
 * bottom of the visual viewport, and xterm refits its rows to that (its
 * ResizeObserver), the cursor row last. Focus leaving takes the limit off.
 *
 * This is a bound, not a keyboard reading: nothing is decided from the numbers
 * but where the bottom of what is on screen is, read while the terminal has
 * focus (so while the keyboard is really up, never from what it leaves behind).
 * Where nothing covers the page (desktop, Android, whose keyboard shrinks the
 * layout viewport) the bound is below the box and changes nothing. Under
 * pinch-zoom it is lifted. Client rects are the visual viewport's in WebKit
 * and the layout viewport's elsewhere; measured from the shell's top (the
 * layout viewport's), both read the same.
 */
export function holdInView(el: HTMLElement, env: HoldEnv | null = holdEnv()): () => void {
  if (env === null) return () => {};
  const { vv, win, shell } = env;
  if (vv === null) return () => {};
  let holding = false;

  const update = () => {
    if (!holding || vv.scale > ZOOM_SCALE) {
      el.style.removeProperty("max-height");
      return;
    }
    const top = el.getBoundingClientRect().top - shell.getBoundingClientRect().top;
    const room = vv.offsetTop + vv.height - top;
    el.style.setProperty("max-height", `${Math.max(HOLD_MIN_PX, Math.floor(room))}px`);
  };
  const onFocusIn = () => {
    holding = true;
    update();
  };
  const onFocusOut = (e: FocusEvent) => {
    // Focus moving within the terminal keeps the hold.
    const next = e.relatedTarget as Node | null;
    if (next !== null && el.contains(next)) return;
    holding = false;
    update();
  };
  el.addEventListener("focusin", onFocusIn);
  el.addEventListener("focusout", onFocusOut);
  vv.addEventListener("resize", update);
  vv.addEventListener("scroll", update);
  win.addEventListener("resize", update);
  return () => {
    el.removeEventListener("focusin", onFocusIn);
    el.removeEventListener("focusout", onFocusOut);
    vv.removeEventListener("resize", update);
    vv.removeEventListener("scroll", update);
    win.removeEventListener("resize", update);
    el.style.removeProperty("max-height");
  };
}

function holdEnv(): HoldEnv | null {
  if (typeof window === "undefined") return null;
  return { vv: window.visualViewport ?? null, win: window, shell: document.body };
}
