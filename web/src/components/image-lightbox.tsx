import { DownloadSimpleIcon, ArrowSquareOutIcon, XIcon } from "@phosphor-icons/react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { DownloadLink, HostImage, useTransportEpoch } from "@/components/host-file";
import { ImageFallback, ImagePending, mediaErrorText, useGithubImage, useObjectUrl } from "@/components/source-image";
import { cn } from "@/lib/cn";
import { imageFileName, type ImageSource } from "@/lib/image-source";

/** Draws the picture, wherever it comes from, at the size it is asked for; `onFail` with why, when that is known. */
type Draw = (opts: { className?: string; thumb: boolean; onFail: (reason?: string) => void }) => React.ReactNode;

const OVERLAY_BUTTON =
  "flex size-7 items-center justify-center rounded bg-white/10 text-white hover:bg-white/25";

/** Keeps a toolbar click from also reaching the overlay, which would close it. */
function stop(e: React.MouseEvent) {
  e.stopPropagation();
}

/**
 * Thumbnail that opens a full-size modal overlay instead of a new page.
 *
 * Takes a {@link ImageSource} rather than a URL, because only one of the three
 * kinds of picture *has* a URL that can go straight into an `<img>`:
 *
 *  * a host file's URL depends on a download ticket that can be replaced under
 *    a mounted image, so building it is `HostImage`/`DownloadLink`'s job — see
 *    `components/host-file.tsx`. The full size image asks for the same `inline`
 *    URL as the thumbnail, so opening the overlay draws from the browser's
 *    cache instead of fetching again, and the save button downloads that same
 *    URL for the same reason;
 *  * a GitHub file is fetched once through the server's `gh` proxy and drawn
 *    from a `blob:` URL by both the thumbnail and the overlay (see
 *    `components/source-image.tsx`), which is also what the save button saves;
 *  * anything else is the agent's own URL, loaded by the browser with no
 *    referrer. A cross-origin `download` is ignored by browsers, so it offers
 *    "open original" rather than a save button that would only navigate.
 *
 * A picture that cannot be drawn becomes an {@link ImageFallback} instead of
 * the browser's broken-image glyph. `zoom={false}` draws the picture alone,
 * for an image that is itself the content of a link.
 */
export function LightboxImage({
  source,
  alt,
  caption,
  thumbClassName,
  zoom = true,
}: {
  source: ImageSource;
  alt: string;
  /** Shown under the thumbnail and the full-size picture; the alt text is not, by default. */
  caption?: string;
  thumbClassName?: string;
  zoom?: boolean;
}) {
  const { t } = useTranslation();
  if (source.kind === "github") {
    return <GithubLightboxImage url={source.url} alt={alt} caption={caption} thumbClassName={thumbClassName} zoom={zoom} />;
  }
  if (source.kind === "host") {
    return (
      <Lightbox
        alt={alt}
        caption={caption}
        thumbClassName={thumbClassName}
        zoom={zoom}
        draw={({ className, thumb, onFail }) => (
          <HostImage
            path={source.path}
            alt={alt}
            className={className}
            loading={thumb ? "lazy" : undefined}
            onFail={onFail}
          />
        )}
        actions={
          <DownloadLink path={source.path} inline title={t("download")} onClick={stop} className={OVERLAY_BUTTON}>
            <DownloadSimpleIcon className="size-4" />
          </DownloadLink>
        }
      />
    );
  }
  return (
    <Lightbox
      alt={alt}
      caption={caption}
      thumbClassName={thumbClassName}
      zoom={zoom}
      original={source.url}
      draw={({ className, thumb, onFail }) => (
        <img
          src={source.url}
          alt={alt}
          className={className}
          loading={thumb ? "lazy" : undefined}
          referrerPolicy="no-referrer"
          onError={() => onFail()}
        />
      )}
    />
  );
}

/** A GitHub file: drawn once its bytes are in, from the object URL they get. */
function GithubLightboxImage({
  url,
  alt,
  caption,
  thumbClassName,
  zoom,
}: {
  url: string;
  alt: string;
  caption?: string;
  thumbClassName?: string;
  zoom: boolean;
}) {
  const { t } = useTranslation();
  const image = useGithubImage(url);
  const src = useObjectUrl(image.data);
  if (image.isError) return <ImageFallback alt={alt} href={zoom ? url : null} reason={mediaErrorText(image.error, t)} />;
  if (!src) return <ImagePending alt={alt} />;
  return (
    <Lightbox
      alt={alt}
      caption={caption}
      thumbClassName={thumbClassName}
      zoom={zoom}
      original={url}
      draw={({ className, onFail }) => <img src={src} alt={alt} className={className} onError={() => onFail()} />}
      actions={
        <a
          href={src}
          download={imageFileName({ kind: "github", url })}
          title={t("download")}
          onClick={stop}
          className={OVERLAY_BUTTON}
        >
          <DownloadSimpleIcon className="size-4" />
        </a>
      }
    />
  );
}

/** The thumbnail, its failure state, and the overlay it opens. */
function Lightbox({
  alt,
  caption,
  thumbClassName,
  zoom,
  draw,
  original,
  actions,
}: {
  alt: string;
  caption?: string;
  thumbClassName?: string;
  zoom: boolean;
  draw: Draw;
  /** Where the picture lives, to open in a new tab; absent for host files. */
  original?: string;
  actions?: React.ReactNode;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // `null` until it fails; then why, if the picture said.
  const [failed, setFailed] = useState<{ reason?: string } | null>(null);
  // In cloud mode, a picture that failed on one side (the offline copy) is
  // tried again once the page is on the other (the machine is back).
  const epoch = useTransportEpoch();
  useEffect(() => setFailed(null), [epoch]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);

  if (failed) {
    return <ImageFallback alt={alt} href={zoom ? original : null} reason={failed.reason || t("image could not be loaded")} />;
  }
  const picture = draw({ className: thumbClassName, thumb: true, onFail: (reason) => setFailed({ reason }) });
  if (!zoom) return picture;

  return (
    <>
      <button
        type="button"
        title={caption || t("view image")}
        onClick={() => setOpen(true)}
        // `min-w-0` so the button can be narrower than the image it holds: as
        // a flex item its automatic minimum is the picture's own width, which
        // is what let a thumbnail hang out of a narrow pane and take the
        // transcript's horizontal scroll with it. With the floor gone the
        // button shrinks to the row and the `max-w` on the image follows it.
        className={cn(
          "min-w-0 cursor-zoom-in",
          caption && "inline-flex max-w-full flex-col items-start gap-0.5 align-top",
        )}
      >
        {picture}
        {caption && <span className="block max-w-full truncate text-2xs text-muted">{caption}</span>}
      </button>
      {open &&
        createPortal(
          <LightboxOverlay
            alt={alt}
            caption={caption}
            draw={draw}
            original={original}
            actions={actions}
            onClose={() => setOpen(false)}
          />,
          document.body,
        )}
    </>
  );
}

/**
 * The full-size picture over everything else. Exported for its test: there is
 * no DOM in this repo's runner to click the thumbnail with, and a portal does
 * not render on the server.
 */
export function LightboxOverlay({
  alt,
  caption,
  draw,
  original,
  actions,
  onClose,
}: {
  alt: string;
  caption?: string;
  draw: Draw;
  original?: string;
  actions?: React.ReactNode;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={alt || t("view image")}
      className="fixed inset-0 z-50 flex cursor-zoom-out flex-col items-center justify-center gap-2 bg-black/85 p-6"
      onClick={onClose}
    >
      <div className="flex min-h-0 w-full flex-1 items-center justify-center">
        {draw({ className: "max-h-full max-w-full rounded object-contain", thumb: false, onFail: () => {} })}
      </div>
      {caption && <div className="max-w-full shrink-0 truncate text-xs text-white/80">{caption}</div>}
      <div className="absolute top-3 right-3 flex items-center gap-1.5">
        {original && (
          <a
            href={original}
            target="_blank"
            rel="noreferrer"
            title={t("open original")}
            onClick={stop}
            className={OVERLAY_BUTTON}
          >
            <ArrowSquareOutIcon className="size-4" />
          </a>
        )}
        {actions}
        <button type="button" title={t("back")} onClick={onClose} className={OVERLAY_BUTTON}>
          <XIcon className="size-4" />
        </button>
      </div>
    </div>
  );
}
