import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleNotchIcon } from "@phosphor-icons/react";

/*
 * QR codes for pairing a device (`lib/hub/pairing.ts`): drawing the new
 * device's offer, and reading it with the camera on the existing one. Both
 * libraries load only when a pairing screen needs them.
 *
 * - Drawing: `qrcode-generator`, alphanumeric mode (the offer is `RHP1-` and
 *   upper-case base32, all in the QR alphanumeric set), drawn as one SVG path,
 *   dark on light whatever the theme so every camera reads it.
 * - Reading: the browser's `BarcodeDetector` where it has one, else `jsqr` on
 *   frames drawn into a canvas.
 */

/** The SVG path of a QR module matrix, one unit square per dark module, offset by the quiet zone. */
export function qrPath(size: number, isDark: (row: number, col: number) => boolean, quiet = 4): string {
  let d = "";
  for (let r = 0; r < size; r++) {
    for (let c = 0; c < size; c++) if (isDark(r, c)) d += `M${c + quiet} ${r + quiet}h1v1h-1z`;
  }
  return d;
}

/** The modules of `text` as a QR code (medium error correction). */
export async function qrModules(text: string): Promise<{ size: number; path: string }> {
  const { default: qrcode } = await import("qrcode-generator");
  const qr = qrcode(0, "M");
  qr.addData(text, /^[0-9A-Z $%*+\-./:]*$/.test(text) ? "Alphanumeric" : "Byte");
  qr.make();
  const size = qr.getModuleCount();
  return { size, path: qrPath(size, (r, c) => qr.isDark(r, c)) };
}

export function QrCode({ text, label }: { text: string; label: string }) {
  const [qr, setQr] = useState<{ size: number; path: string } | null>(null);
  useEffect(() => {
    let live = true;
    void qrModules(text).then((m) => live && setQr(m));
    return () => {
      live = false;
    };
  }, [text]);
  if (!qr) {
    return (
      <div className="flex aspect-square w-full max-w-64 items-center justify-center self-center border border-border">
        <CircleNotchIcon className="size-4 animate-spin text-faint" />
      </div>
    );
  }
  const n = qr.size + 8;
  return (
    <svg
      role="img"
      aria-label={label}
      data-testid="pairing-qr"
      viewBox={`0 0 ${n} ${n}`}
      shapeRendering="crispEdges"
      className="aspect-square w-full max-w-64 self-center border border-border"
    >
      <rect width={n} height={n} fill="#fff" />
      <path d={qr.path} fill="#000" />
    </svg>
  );
}

interface Detector {
  detect(source: CanvasImageSource): Promise<{ rawValue: string }[]>;
}

type DetectorCtor = { new (opts: { formats: string[] }): Detector; getSupportedFormats?: () => Promise<string[]> };

/** The browser's own QR reader, if it has one. */
async function nativeDetector(): Promise<Detector | null> {
  const Ctor = (globalThis as { BarcodeDetector?: DetectorCtor }).BarcodeDetector;
  if (!Ctor) return null;
  try {
    const formats = (await Ctor.getSupportedFormats?.()) ?? ["qr_code"];
    return formats.includes("qr_code") ? new Ctor({ formats: ["qr_code"] }) : null;
  } catch {
    return null;
  }
}

/** A reader for one video frame: native, or `jsqr` over a canvas. */
async function frameReader(): Promise<(video: HTMLVideoElement) => Promise<string | null>> {
  const native = await nativeDetector();
  if (native) {
    return async (video) => (await native.detect(video))[0]?.rawValue ?? null;
  }
  // A CommonJS bundle: its function is the default export, or that export's own `default`.
  const mod = (await import("jsqr")) as { default: unknown };
  type Decode = typeof import("jsqr").default;
  const jsQR = (typeof mod.default === "function" ? mod.default : (mod.default as { default: Decode }).default) as Decode;
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  return async (video) => {
    if (!ctx || !video.videoWidth) return null;
    // A frame at most 640 wide: plenty for a code held up to the camera.
    const scale = Math.min(1, 640 / video.videoWidth);
    canvas.width = Math.round(video.videoWidth * scale);
    canvas.height = Math.round(video.videoHeight * scale);
    ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
    const image = ctx.getImageData(0, 0, canvas.width, canvas.height);
    return jsQR(image.data, image.width, image.height, { inversionAttempts: "dontInvert" })?.data ?? null;
  };
}

/** The camera, until it sees a QR code whose text `accept` takes. */
export function QrScanner({ onText, accept }: { onText: (text: string) => void; accept: (text: string) => boolean }) {
  const { t } = useTranslation();
  const video = useRef<HTMLVideoElement>(null);
  const [error, setError] = useState<string | null>(null);
  const done = useRef(false);
  useEffect(() => {
    let stream: MediaStream | null = null;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    void (async () => {
      try {
        if (!navigator.mediaDevices?.getUserMedia) throw new Error(t("this browser cannot use a camera here"));
        stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" }, audio: false });
        if (stopped) return;
        const el = video.current!;
        el.srcObject = stream;
        await el.play().catch(() => undefined);
        const read = await frameReader();
        const scan = async () => {
          if (stopped || done.current) return;
          try {
            const text = await read(el);
            if (text && accept(text)) {
              done.current = true;
              onText(text);
              return;
            }
          } catch {
            // a frame it could not read
          }
          timer = setTimeout(() => void scan(), 200);
        };
        void scan();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
      for (const track of stream?.getTracks() ?? []) track.stop();
    };
    // Once per mount: a new code means a new scanner.
  }, []);
  return (
    <div className="flex flex-col gap-2">
      <video ref={video} muted playsInline className="aspect-square w-full max-w-64 self-center border border-border bg-black object-cover" />
      {error ? (
        <p className="font-mono text-2xs break-words text-danger">{error}</p>
      ) : (
        <p className="text-center text-2xs text-faint">{t("point the camera at the QR code on the new device")}</p>
      )}
    </div>
  );
}
