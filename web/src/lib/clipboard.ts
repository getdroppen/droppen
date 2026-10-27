/*
 * The one "copied" state: a control writes a value to the clipboard, flips to
 * `copied` for a moment, and tells the caller about the outcome when it has a
 * better fallback to offer than silence (the files browser shows a notice and
 * focuses its address bar).
 */
import { useEffect, useRef, useState } from "react";

/** How long `copied` stays true: long enough to read, short enough not to linger. */
const COPIED_MS = 1500;

/**
 * Copy `text`, and say so for a moment. A refused copy (no permission, no
 * secure context) calls `onError` if the caller passed one; the value is
 * still on screen and selectable.
 */
export function useCopy({
  onSuccess,
  onError,
}: {
  onSuccess?: () => void;
  onError?: () => void;
} = {}): { copied: boolean; copy: (text: string) => void } {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return {
    copied,
    copy: (text) => {
      void navigator.clipboard?.writeText(text).then(
        () => {
          setCopied(true);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), COPIED_MS);
          onSuccess?.();
        },
        () => onError?.(),
      );
    },
  };
}
