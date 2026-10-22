/*
 * The composer: the one box a message is typed into, wherever it goes.
 *
 * A session's composer sends into that session; the new-session view's starts
 * one with it. Both are this component — the attachments (uploaded the moment
 * they are pasted, dropped or picked, so sending is instant later), the
 * textarea with its IME handling and its send key, the attach and photo
 * buttons, the chips row and the send button — and only what the message is
 * for differs, which the caller supplies: the chips, the placeholder, what
 * sending does and what the send button says.
 */
import { Camera, FileText, ListPlus, Loader2, Paperclip, SendHorizontal, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { HostImage } from "@/components/host-file";
import { authHeader, noteUnauthorized } from "@/lib/auth";
import { cn } from "@/lib/cn";
import { isImeKeydown, useImeComposition } from "@/lib/ime";
import { useMachineRuntime } from "@/lib/machines/context";
import { autoGrowTextarea, useEnterSendsModifier } from "./composer-controls";

/** How many attachments one message carries at most. */
export const MAX_ATTACHMENTS = 20;

export interface ComposerAttachment {
  key: string;
  /** The file picked, pasted or dropped; null for one restored from a saved draft (already uploaded). */
  file: File | null;
  name: string;
  isImage: boolean;
  /** Object URL for image preview; empty for non-images and for restored images (drawn from the device). */
  preview: string;
  status: "uploading" | "ready" | "error";
  path?: string;
  error?: string;
}

/** An uploaded attachment as a draft keeps it: what the device answered, nothing of this page's. */
export interface StoredAttachment {
  path: string;
  name: string;
  isImage: boolean;
}

/** The uploaded attachments, as a message carries them. */
export function attachmentPaths(attachments: readonly ComposerAttachment[]): { images: string[]; files: string[] } {
  const ready = attachments.filter((a): a is ComposerAttachment & { path: string } => a.status === "ready" && !!a.path);
  return {
    images: ready.filter((a) => a.isImage).map((a) => a.path),
    files: ready.filter((a) => !a.isImage).map((a) => a.path),
  };
}

/** The uploaded attachments as a draft stores them. */
export function storedAttachments(attachments: readonly ComposerAttachment[]): StoredAttachment[] {
  return attachments.flatMap((a) => (a.status === "ready" && a.path ? [{ path: a.path, name: a.name, isImage: a.isImage }] : []));
}

/** A draft's attachments, back in the composer: uploaded already, so ready. */
export function restoredAttachments(stored: readonly StoredAttachment[]): ComposerAttachment[] {
  return stored.map((a, i) => ({ key: `restored-${i}-${a.path}`, file: null, name: a.name, isImage: a.isImage, preview: "", status: "ready", path: a.path }));
}

/**
 * A refused message's attachments put back beside the ones added since: the
 * refused ones first (they were there first, and the message is theirs), then
 * as many of the later ones as still fit under [`MAX_ATTACHMENTS`]; the later
 * ones that do not fit, apart, for the composer to drop and name. No
 * attachment twice.
 */
export function mergeBack(
  back: readonly ComposerAttachment[],
  now: readonly ComposerAttachment[],
  max: number = MAX_ATTACHMENTS,
): { kept: ComposerAttachment[]; dropped: ComposerAttachment[] } {
  const later = now.filter((a) => !back.some((b) => b.key === a.key));
  const room = Math.max(0, max - back.length);
  return { kept: [...back, ...later.slice(0, room)], dropped: later.slice(room) };
}

let attachmentSequence = 0;

/**
 * A composer's attachments, on the view's device: each file is uploaded
 * (`POST /api/uploads`, which needs no session) the moment it is added.
 */
export function useComposerAttachments(initial: () => ComposerAttachment[] = () => []) {
  const runtime = useMachineRuntime();
  const [attachments, setState] = useState<ComposerAttachment[]>(initial);
  // The list as of the last change — what the next change starts from, and
  // `addFiles` counts its room in. Set here, not in a state updater: React
  // runs one of those at once only when nothing else on this component is
  // waiting to be drawn, so after a start's other updates its `take` would
  // land only at the next render, and a paste before then would still count
  // what was taken.
  const current = useRef(attachments);
  const update = (change: (prev: ComposerAttachment[]) => ComposerAttachment[]) => {
    const next = change(current.current);
    current.current = next;
    setState(next);
  };

  const uploadOne = async (entry: ComposerAttachment & { file: File }) => {
    try {
      const form = new FormData();
      form.append("file", entry.file);
      // Multipart: outside the typed client, so the bearer token and the 401
      // handling its middleware does are both repeated here by hand.
      const res = await runtime.transport.fetch(new Request("/api/uploads", { method: "POST", headers: authHeader(), body: form }));
      if (res.status === 401) noteUnauthorized("/api/uploads");
      const body = (await res.json()) as { path?: string; name?: string; isImage?: boolean; error?: string };
      if (!res.ok || !body.path) throw new Error(body.error ?? "upload failed");
      update((prev) =>
        prev.map((a) =>
          a.key === entry.key
            ? { ...a, status: "ready", path: body.path, isImage: body.isImage ?? a.isImage, name: body.name ?? a.name }
            : a,
        ),
      );
    } catch (err) {
      const error = err instanceof Error ? err.message : "upload failed";
      update((prev) => prev.map((a) => (a.key === entry.key ? { ...a, status: "error", error } : a)));
    }
  };

  // Uploads start the moment a file is added, so send is instant later.
  const addFiles = (incoming: Iterable<File>) => {
    const list = [...incoming];
    if (!list.length) return;
    const room = Math.max(0, MAX_ATTACHMENTS - current.current.length);
    const entries = list.slice(0, room).map((file) => {
      const isImage = file.type.startsWith("image/");
      return {
        key: `${Date.now()}-${++attachmentSequence}-${Math.random().toString(36).slice(2)}`,
        file,
        name: file.name,
        isImage,
        preview: isImage ? URL.createObjectURL(file) : "",
        status: "uploading" as const,
      };
    });
    if (!entries.length) return;
    update((prev) => [...prev, ...entries]);
    entries.forEach((entry) => void uploadOne(entry));
  };

  const remove = (key: string) => {
    update((prev) => {
      const gone = prev.find((a) => a.key === key);
      if (gone?.preview) URL.revokeObjectURL(gone.preview);
      return prev.filter((a) => a.key !== key);
    });
  };

  /** Empty the composer's list, previews and all: a message takes the paths, not the pictures. */
  const clear = () => {
    current.current.forEach((a) => a.preview && URL.revokeObjectURL(a.preview));
    update(() => []);
  };

  /**
   * Take these out of the list for a message that carries them. Their
   * previews stay alive: the message shows them until it is accepted (which
   * revokes them), or puts them back if it is refused ([`putBack`]).
   */
  const take = (keys: readonly string[]) => update((prev) => prev.filter((a) => !keys.includes(a.key)));

  /**
   * Put back what a refused message carried, in front of whatever was added
   * since — under the cap ([`mergeBack`]): the later attachments that no longer
   * fit are dropped, previews and all, and returned so the composer can say so.
   */
  const putBack = (list: readonly ComposerAttachment[]): ComposerAttachment[] => {
    const { kept, dropped } = mergeBack(list, current.current);
    dropped.forEach((a) => a.preview && URL.revokeObjectURL(a.preview));
    update(() => kept);
    return dropped;
  };

  return {
    attachments,
    addFiles,
    remove,
    clear,
    take,
    putBack,
    anyUploading: attachments.some((a) => a.status === "uploading"),
    anyFailed: attachments.some((a) => a.status === "error"),
  };
}

export type ComposerAttachments = ReturnType<typeof useComposerAttachments>;

/** Whether a drag carries files (rather than text or a link being moved about). */
function draggingFiles(e: React.DragEvent): boolean {
  return [...(e.dataTransfer?.types ?? [])].includes("Files");
}

/** The words after a placeholder that say how to send: they follow the user's own send-key preference. */
export function useSendHint(): string {
  const { t } = useTranslation();
  return useEnterSendsModifier() ? t("⌘/Ctrl+Enter to send") : t("Enter to send, Shift+Enter for newline");
}

/** One attachment in the strip above the textarea. */
function AttachmentThumb({ attachment: a, onRemove }: { attachment: ComposerAttachment; onRemove: () => void }) {
  const { t } = useTranslation();
  const frame = cn(
    a.status === "error" ? "border-danger" : "border-border",
    a.status === "uploading" && "opacity-50",
  );
  return (
    <span data-attachment={a.isImage ? "image" : "file"} className="group relative">
      {a.isImage && a.preview ? (
        <img src={a.preview} alt={a.name} title={a.error ?? a.name} className={cn("size-10 rounded border object-cover", frame)} />
      ) : a.isImage && a.path ? (
        // Restored from a draft: the picture is on the device, not in this page.
        <span title={a.error ?? a.name} className={cn("block size-10 overflow-hidden rounded border", frame)}>
          <HostImage path={a.path} alt={a.name} className="size-full object-cover" />
        </span>
      ) : (
        <span
          title={a.error ?? a.name}
          className={cn("flex h-10 max-w-40 items-center gap-1.5 rounded border bg-panel px-2 font-mono text-2xs text-fg", frame)}
        >
          <FileText className="size-3.5 shrink-0 text-muted" />
          <span className="truncate">{a.name}</span>
        </span>
      )}
      {a.status === "uploading" && <Loader2 className="absolute inset-0 m-auto size-4 animate-spin text-fg" />}
      <button
        type="button"
        title={t("remove attachment")}
        onClick={onRemove}
        data-signal="danger"
        className="absolute -top-1 -right-1 hidden size-3.5 items-center justify-center bg-danger text-white group-hover:flex"
      >
        <X className="size-2.5" />
      </button>
    </span>
  );
}

/**
 * The composer box: attachments, the message, and one toolbar row — attach and
 * the caller's chips on the left, send on the right.
 *
 * `text` is the caller's: a session keeps it in its draft store, the new
 * session in its own draft. `onText` commits a change; `onTextLive` only
 * mirrors the field while an IME is composing, when anything more (a draft
 * write fanned out to other composers) would abort the composition.
 */
export function ComposerBox({
  attachments,
  allowAttach = true,
  text,
  onText,
  onTextLive = onText,
  disabled = false,
  placeholder,
  onSubmit,
  chips,
  send,
  autoFocus = false,
}: {
  attachments: ComposerAttachments;
  /** Claude instant chats run without tools, so they cannot read attachments. */
  allowAttach?: boolean;
  text: string;
  onText: (text: string) => void;
  onTextLive?: (text: string) => void;
  /** Nothing can be typed or attached (a send being settled, a read-only copy). */
  disabled?: boolean;
  placeholder: string;
  onSubmit: () => void;
  /** The pickers between the attach buttons and send. */
  chips?: ReactNode;
  send: {
    disabled: boolean;
    /** The button's name: send, add to queue, send as your answer, start session. */
    label: string;
    /** A spinner instead of the arrow. */
    busy: boolean;
    /** The message will wait in a queue: the queue glyph instead of the arrow. */
    queues?: boolean;
  };
  autoFocus?: boolean;
}) {
  const { t } = useTranslation();
  const enterSendsModifier = useEnterSendsModifier();
  const ime = useImeComposition(onText);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => autoGrowTextarea(textareaRef.current), [text]);
  const [dragging, setDragging] = useState(false);
  const canAttach = allowAttach && !disabled;

  return (
    <div
      data-composer=""
      className={cn("border border-border-strong bg-bg focus-within:border-fg", dragging && "border-fg bg-hover")}
      // Files dropped anywhere on the box are attached, as pasted ones are.
      onDragOver={(e) => {
        if (!canAttach || !draggingFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
      }}
      onDrop={(e) => {
        setDragging(false);
        if (!canAttach) return;
        const files = [...(e.dataTransfer?.files ?? [])];
        if (!files.length) return;
        e.preventDefault();
        attachments.addFiles(files);
      }}
    >
      {attachments.attachments.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 px-2 pt-2">
          {attachments.attachments.map((a) => (
            <AttachmentThumb key={a.key} attachment={a} onRemove={() => attachments.remove(a.key)} />
          ))}
        </div>
      )}
      <textarea
        ref={textareaRef}
        value={text}
        disabled={disabled}
        autoFocus={autoFocus}
        {...ime.props}
        onChange={(e) =>
          // Mid-composition, only mirror the DOM into state; the draft
          // write (and its fan-out to other mounted composers) waits for
          // compositionend so nothing can abort the IME session.
          ime.isComposing.current ? onTextLive(e.target.value) : onText(e.target.value)
        }
        onPaste={(e) => {
          if (!allowAttach) return;
          const files = [...(e.clipboardData?.files ?? [])];
          if (files.length) {
            e.preventDefault();
            attachments.addFiles(files);
          }
        }}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || isImeKeydown(e)) return;
          const sendKey = enterSendsModifier ? e.metaKey || e.ctrlKey : !e.shiftKey;
          if (sendKey) {
            e.preventDefault();
            onSubmit();
          }
        }}
        rows={2}
        placeholder={placeholder}
        className="max-h-40 min-h-11 w-full resize-none bg-transparent px-2.5 pt-2 pb-1 text-[13px] leading-5 text-fg placeholder:text-faint focus:outline-none disabled:opacity-40 md:max-h-72"
      />
      {/* One toolbar row: attach + pickers left, send right. */}
      <div className="flex flex-wrap items-center gap-0.5 px-1.5 pb-1.5">
        {allowAttach && (
          <>
            <input
              ref={fileInputRef}
              type="file"
              disabled={disabled}
              multiple
              className="hidden"
              onChange={(e) => {
                if (e.target.files) attachments.addFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <button
              type="button"
              title={t("attach files (or paste into the message box)")}
              disabled={disabled}
              onClick={() => fileInputRef.current?.click()}
              className="flex size-6 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg disabled:pointer-events-none disabled:opacity-40"
            >
              <Paperclip className="size-3.5" />
            </button>
            {/* Phones only: accept="image/*" with no capture attribute makes
                the OS offer the camera AND the photo library, where the
                generic file picker buries both behind a file browser. */}
            <input
              ref={imageInputRef}
              type="file"
              disabled={disabled}
              multiple
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                if (e.target.files) attachments.addFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <button
              type="button"
              title={t("take or choose a photo")}
              disabled={disabled}
              onClick={() => imageInputRef.current?.click()}
              className="flex size-6 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg disabled:pointer-events-none disabled:opacity-40 md:hidden"
            >
              <Camera className="size-3.5" />
            </button>
          </>
        )}
        {chips}
        <button
          type="button"
          onClick={onSubmit}
          disabled={send.disabled}
          title={send.label}
          aria-label={send.label}
          data-variant="primary"
          className="ml-auto flex size-6.5 shrink-0 items-center justify-center bg-fg text-bg hover:bg-fg/85 disabled:opacity-40"
        >
          {send.busy ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : send.queues ? (
            <ListPlus className="size-3.5" />
          ) : (
            <SendHorizontal className="size-3.5" />
          )}
        </button>
      </div>
    </div>
  );
}
