// Regenerates the logo, favicon, app icons and launch images from the
// Droppen mark defined below. Run `pnpm gen:pwa` after touching the mark.
// The output is committed because the server embeds web/dist, so it has to
// exist without a Node toolchain on the build host.
//
// The mark is a solid drop that is also a D: a square corner at the top left
// and a 270° arc. The owner's rule: the
// browser favicon is the black mark on a transparent ground (white in dark tab
// bars, via the SVG's prefers-color-scheme), and every app icon is the paper
// mark on an ink tile. logo.png is an output of this script, not its input.
// Keep the contour in step with src/components/brand-mark.tsx: corner at c,
// arc radius r = box / 2 - c, so the arc is centred and the glyph spans
// c .. box - c.
//
// iOS has no equivalent of the manifest's icon list: a launch image is used
// only when an <link rel="apple-touch-startup-image"> media query matches the
// device exactly, and there is no scaling fallback. DEVICES below therefore
// has to mirror the tags in index.html — the script prints them so a drift is
// visible.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";

import { Resvg } from "@resvg/resvg-js";

const publicDir = join(dirname(fileURLToPath(import.meta.url)), "..", "public");
const splashDir = join(publicDir, "splash");

// The launch image is the first thing an installed app paints, so it uses the
// app's own paper and ink: an ink tile on white, a paper tile on ink.
const INK = "#0a0a0a";
const PAPER = "#f5f5f5";
const DARK = { name: "dark", bg: "#0f1115", tile: PAPER, glyph: INK };
const LIGHT = { name: "light", bg: "#ffffff", tile: INK, glyph: "#ffffff" };

// Portrait only: a launch image whose orientation does not match is ignored
// rather than rotated, and the app opens in portrait on a phone. Sizes are CSS
// px plus the device pixel ratio, which is what the media query keys on.
const DEVICES = [
  { css: [375, 667], dpr: 2, note: "iPhone SE / 8" },
  { css: [375, 812], dpr: 3, note: "iPhone X / XS / 11 Pro / 12 mini / 13 mini" },
  { css: [390, 844], dpr: 3, note: "iPhone 12 / 13 / 14" },
  { css: [393, 852], dpr: 3, note: "iPhone 14 Pro / 15 / 16" },
  { css: [402, 874], dpr: 3, note: "iPhone 16 Pro" },
  { css: [414, 896], dpr: 2, note: "iPhone XR / 11" },
  { css: [414, 896], dpr: 3, note: "iPhone XS Max / 11 Pro Max" },
  { css: [428, 926], dpr: 3, note: "iPhone 12/13/14 Pro Max" },
  { css: [430, 932], dpr: 3, note: "iPhone 14 Pro Max / 15 Pro Max / 16 Plus" },
  { css: [440, 956], dpr: 3, note: "iPhone 16 Pro Max" },
  { css: [768, 1024], dpr: 2, note: "iPad / iPad mini" },
  { css: [834, 1194], dpr: 2, note: "iPad Pro 11" },
];

// Share of the shorter edge the glyph spans on a launch image. Larger reads as
// a splash screen with a logo on it rather than as the app already starting.
const SPLASH_GLYPH_RATIO = 0.25;

/**
 * Share of the tile the glyph spans on every app icon, the logo and the
 * launch images' tile: 43%, one size smaller than the first cut's half (the
 * owner found the mark "a touch too big"). The corner sits (1 - 0.43) / 2 in.
 */
const TILE_GLYPH = 0.43;

/**
 * The filled drop-D on a square tile of side `size` placed at (x, y): the
 * glyph's square corner sits `corner` in from the tile's top left and its arc
 * is centred on the tile. The default corner gives the app-icon proportion
 * ({@link TILE_GLYPH}); small favicons pass a smaller corner so the glyph
 * grows and its straight edges land on whole pixels.
 */
function tile({ x = 0, y = 0, size = 100, corner = (size * (1 - TILE_GLYPH)) / 2, tile: fill = INK, glyph = "#ffffff" } = {}) {
  const c = corner;
  const r = size / 2 - c;
  return (
    `<rect x="${x}" y="${y}" width="${size}" height="${size}" fill="${fill}"/>` +
    `<path d="M${round(x + c)} ${round(y + c)}H${round(x + c + r)}A${round(r)} ${round(r)} 0 1 1 ${round(x + c)} ${round(y + c + r)}Z" fill="${glyph}"/>`
  );
}

/** The mark alone (no tile) in a box of side `size`, corner `corner` in. */
function glyph({ size, corner, fill = INK, attrs = "" }) {
  const r = size / 2 - corner;
  return `<path d="M${corner} ${corner}H${round(corner + r)}A${round(r)} ${round(r)} 0 1 1 ${corner} ${round(corner + r)}Z"${fill ? ` fill="${fill}"` : ""}${attrs}/>`;
}

/** The 100-unit mark used by logo.svg and the launch images. */
function mark({ tile: fill = INK, glyph = "#ffffff" } = {}) {
  return tile({ size: 100, tile: fill, glyph });
}

const round = (value) => Number(value.toFixed(4));

function svg(width, height, body) {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" ` +
    `viewBox="0 0 ${width} ${height}">${body}</svg>`
  );
}

/**
 * Encodes resvg's RGBA buffer, preserving transparent rounded corners.
 *
 * The whole set ships inside the server binary, so the bytes matter more than
 * the encoding time resvg's own `asPng()` is tuned for. Two flat colours with
 * anti-aliased edges between them stay well inside a 256-entry palette, which
 * costs a third of truecolour before deflate even runs; richer art would fall
 * back. Every row but the first is then identical to the one above it, which
 * the Up filter turns into a run of zeros.
 */
function encodePng(width, height, rgba) {
  const hasAlpha = rgba.some((value, index) => index % 4 === 3 && value !== 255);
  const palette = new Map();
  for (let at = 0; at < rgba.length && palette.size <= 256; at += 4) {
    const colour = (rgba[at] << 16) | (rgba[at + 1] << 8) | rgba[at + 2];
    if (!palette.has(colour)) palette.set(colour, palette.size);
  }
  const indexed = !hasAlpha && palette.size <= 256;
  const channels = hasAlpha ? 4 : 3;
  const stride = width * (indexed ? 1 : channels);

  const samples = Buffer.allocUnsafe(stride * height);
  for (let pixel = 0, at = 0; at < rgba.length; pixel += 1, at += 4) {
    if (indexed) {
      samples[pixel] = palette.get((rgba[at] << 16) | (rgba[at + 1] << 8) | rgba[at + 2]);
    } else {
      samples.set(rgba.subarray(at, at + channels), pixel * channels);
    }
  }
  const raw = Buffer.allocUnsafe((stride + 1) * height);
  for (let y = 0; y < height; y += 1) {
    const row = y * (stride + 1);
    raw[row] = 2; // Up: subtract the row above, taken as zeros for the first.
    for (let i = 0; i < stride; i += 1) {
      const at = y * stride + i;
      raw[row + 1 + i] = (samples[at] - (y === 0 ? 0 : samples[at - stride])) & 0xff;
    }
  }

  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const out = Buffer.allocUnsafe(body.length + 8);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc32(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = indexed ? 3 : hasAlpha ? 6 : 2;
  const plte = Buffer.allocUnsafe(palette.size * 3);
  for (const [colour, index] of palette) plte.writeUIntBE(colour, index * 3, 3);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    ...(indexed ? [chunk("PLTE", plte)] : []),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function render(markup) {
  const image = new Resvg(markup, { fitTo: { mode: "original" } }).render();
  return encodePng(image.width, image.height, image.pixels);
}

// Launch images keep the original composition; app icons use a tighter crop.
const metrics = { cx: 50, cy: 50, width: 100, height: 100, radius: Math.hypot(50, 50) };

function compose(width, height, scale, theme) {
  const tx = round(width / 2 - scale * metrics.cx);
  const ty = round(height / 2 - scale * metrics.cy);
  return svg(
    width,
    height,
    `<rect width="${width}" height="${height}" fill="${theme.bg}"/>` +
      `<g transform="translate(${tx} ${ty}) scale(${round(scale)})">${mark({ tile: theme.tile, glyph: theme.glyph })}</g>`,
  );
}

function emit(path, bytes) {
  writeFileSync(path, bytes);
  return bytes.length;
}

// "any" icons (favicon, 192, 512) are a square tile inset by 6% so their
// corners are transparent and the launcher's own shape never crops the mark;
// apple-touch and maskable icons are full bleed, because the OS supplies the
// mask. With the glyph at 43% of the tile its farthest point (its square
// corner) is 30% of the size from the centre, inside the maskable 40% safe circle.
// Square corners everywhere: the rounding, if any, is the OS's.
function appIcon(size, inset, corner) {
  const pad = inset ? Math.round(size * 0.06) : 0;
  const side = size - pad * 2;
  return svg(size, size, tile({ x: pad, y: pad, size: side, corner: corner ?? (side * (1 - TILE_GLYPH)) / 2 }));
}

/**
 * The 16px favicon, drawn on the pixel grid rather than scaled: the black
 * drop-D on a transparent ground, its corner on pixel (2, 2) and its arc a
 * 13px pixel circle, so the glyph spans 13 of the 16 pixels (81%, one size
 * smaller than edge to edge, as the owner asked). A pixel is ink when the
 * vector shape covers at least half of it, so there is no grey fringe at 1x.
 */
function pixelFavicon() {
  const inside = (x, y) => x >= 2 && y >= 2 && ((x <= 8.5 && y <= 8.5) || (x - 8.5) ** 2 + (y - 8.5) ** 2 <= 6.5 ** 2);
  let d = "";
  for (let j = 0; j < 16; j += 1) {
    let run = null;
    for (let i = 0; i <= 16; i += 1) {
      let hit = 0;
      if (i < 16) for (let u = 0; u < 8; u += 1) for (let v = 0; v < 8; v += 1) hit += inside(i + (u + 0.5) / 8, j + (v + 0.5) / 8);
      const on = i < 16 && hit >= 32;
      if (on && run === null) run = i;
      if (!on && run !== null) {
        d += `M${run} ${j}h${i - run}v1h${run - i}z`;
        run = null;
      }
    }
  }
  return svg(16, 16, `<path d="${d}" fill="${INK}" shape-rendering="crispEdges"/>`);
}

const assetVersions = new Map();
const version = (file, bytes) => assetVersions.set(file, createHash("sha256").update(bytes).digest("hex").slice(0, 12));
function emitIcon(file, size, inset, corner) {
  const bytes = render(appIcon(size, inset, corner));
  version(file, bytes);
  return emit(join(publicDir, file), bytes);
}

mkdirSync(splashDir, { recursive: true });

let total = 0;
const icons = [
  ["apple-touch-icon.png", 180, false],
  ["icon-192.png", 192, true],
  ["icon-512.png", 512, true],
  ["icon-maskable-512.png", 512, false],
];
for (const [file, size, inset] of icons) {
  total += emitIcon(file, size, inset);
}
// Browser favicons: the black mark on a transparent ground, corner 3px in at
// 32 (2px at 16): the glyph spans 26 of 32 pixels (81%) rather than edge to
// edge, and its straight edges sit on whole pixels at both sizes.
// favicon.svg turns white when the browser is dark, so it never disappears
// into a dark tab bar; the PNG fallbacks cannot, and stay black.
const favicon32 = render(svg(32, 32, glyph({ size: 32, corner: 3 })));
version("favicon.png", favicon32);
total += emit(join(publicDir, "favicon.png"), favicon32);
const favicon16 = render(pixelFavicon());
version("favicon-16.png", favicon16);
total += emit(join(publicDir, "favicon-16.png"), favicon16);
const faviconSvg =
  svg(32, 32, `<style>path{fill:${INK}}@media (prefers-color-scheme:dark){path{fill:#ffffff}}</style>${glyph({ size: 32, corner: 3, fill: null })}`) + "\n";
version("favicon.svg", Buffer.from(faviconSvg));
writeFileSync(join(publicDir, "favicon.svg"), faviconSvg);
// The mark itself, for anything that wants the artwork rather than an icon.
total += emit(join(publicDir, "logo.png"), render(appIcon(1024, false)));
writeFileSync(join(publicDir, "logo.svg"), svg(100, 100, mark()) + "\n");

// Keep the manifest URL/id stable for installed apps; change only image URLs.
// Hash rendered bytes so framing/rounding edits also invalidate old icons.
for (const relative of ["index.html", "public/manifest.webmanifest"]) {
  const path = join(publicDir, "..", relative);
  const before = readFileSync(path, "utf8");
  const after = before.replace(/\/(favicon|favicon-16|apple-touch-icon|icon-192|icon-512|icon-maskable-512)\.(png|svg)(?:\?v=[a-f0-9]+)?/g,
    (_, name, ext) => `/${name}.${ext}?v=${assetVersions.get(`${name}.${ext}`)}`);
  if (after !== before) writeFileSync(path, after);
}

const tags = [];
for (const device of DEVICES) {
  const [cssWidth, cssHeight] = device.css;
  const width = cssWidth * device.dpr;
  const height = cssHeight * device.dpr;
  const scale = (SPLASH_GLYPH_RATIO * Math.min(width, height)) / metrics.width;
  for (const theme of [DARK, LIGHT]) {
    const file = `apple-splash-${width}x${height}-${theme.name}.png`;
    total += emit(join(splashDir, file), render(compose(width, height, scale, theme)));
    tags.push(
      `<link rel="apple-touch-startup-image" href="/splash/${file}"` +
        ` media="(prefers-color-scheme: ${theme.name}) and (device-width: ${cssWidth}px)` +
        ` and (device-height: ${cssHeight}px) and (-webkit-device-pixel-ratio: ${device.dpr})` +
        ` and (orientation: portrait)" />`,
    );
  }
}

console.log(tags.join("\n"));
console.log(
  `\n${icons.length + 4 + DEVICES.length * 2} files, ${(total / 1024).toFixed(1)} KiB`,
);
