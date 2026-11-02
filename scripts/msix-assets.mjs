#!/usr/bin/env node
// The MSIX logos of the Microsoft Store build (BRAI-442), made from src-tauri/icons/icon.png at
// package time, so the app icon stays the one source (no second set of PNGs in the repo).
//
//   node scripts/msix-assets.mjs <icon.png> <out Assets folder>
//
// Files carry MRT qualifiers (`Square44x44Logo.scale-200.png`, `….targetsize-24_altform-
// unplated.png`); makepri indexes them into resources.pri, and the manifest names the plain
// `Assets\Square44x44Logo.png`. The unplated targetsize icons are the ones the taskbar and Start
// show without a coloured plate. Plain Node (zlib only): 8-bit RGB(A) PNGs in, RGBA PNGs out.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync, deflateSync } from "node:zlib";

// [base name, qualifier, width, height, share of the shorter side the icon takes]
export const ASSETS = [
  ["StoreLogo", "scale-100", 50, 50, 1],
  ["StoreLogo", "scale-200", 100, 100, 1],
  ["StoreLogo", "scale-400", 200, 200, 1],
  ["Square44x44Logo", "scale-100", 44, 44, 1],
  ["Square44x44Logo", "scale-200", 88, 88, 1],
  ["Square44x44Logo", "scale-400", 176, 176, 1],
  ...[16, 24, 32, 48, 256].flatMap((n) => [
    ["Square44x44Logo", `targetsize-${n}`, n, n, 1],
    ["Square44x44Logo", `targetsize-${n}_altform-unplated`, n, n, 1],
  ]),
  ["Square71x71Logo", "scale-100", 71, 71, 0.7],
  ["Square71x71Logo", "scale-200", 142, 142, 0.7],
  ["Square150x150Logo", "scale-100", 150, 150, 0.66],
  ["Square150x150Logo", "scale-200", 300, 300, 0.66],
  ["Wide310x150Logo", "scale-100", 310, 150, 0.66],
  ["Wide310x150Logo", "scale-200", 620, 300, 0.66],
  ["Square310x310Logo", "scale-100", 310, 310, 0.66],
  ["Square310x310Logo", "scale-200", 620, 620, 0.66],
];

export const fileName = ([base, qualifier]) => `${base}.${qualifier}.png`;

// ---- PNG ---------------------------------------------------------------------------------------

const SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

/** `{width, height, data}`: RGBA, 8 bits per channel. Non-interlaced 8-bit RGB / RGBA only. */
export function decodePng(buf) {
  if (!buf.subarray(0, 8).equals(SIGNATURE)) throw new Error("not a PNG");
  let pos = 8;
  let width, height, colorType;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const body = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const depth = body[8];
      colorType = body[9];
      if (depth !== 8 || (colorType !== 2 && colorType !== 6) || body[12] !== 0) {
        throw new Error("only non-interlaced 8-bit RGB / RGBA PNGs are supported");
      }
    } else if (type === "IDAT") {
      idat.push(body);
    } else if (type === "IEND") {
      break;
    }
    pos += 12 + len;
  }
  const channels = colorType === 6 ? 4 : 3;
  const stride = width * channels;
  const raw = inflateSync(Buffer.concat(idat));
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? pixels[y * stride + x - channels] : 0;
      const b = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? pixels[(y - 1) * stride + x - channels] : 0;
      const predictor = [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter];
      if (predictor === undefined) throw new Error(`bad PNG filter ${filter}`);
      pixels[y * stride + x] = (line[x] + predictor) & 0xff;
    }
  }
  if (channels === 4) return { width, height, data: pixels };
  const data = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    pixels.copy(data, i * 4, i * 3, i * 3 + 3);
    data[i * 4 + 3] = 255;
  }
  return { width, height, data };
}

function chunk(type, body) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(body.length, 0);
  head.write(type, 4, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])));
  return Buffer.concat([head, body, crc]);
}

/** An RGBA image as a PNG (filter 0, best compression). */
export function encodePng({ width, height, data }) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) data.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([
    SIGNATURE,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---- resampling --------------------------------------------------------------------------------

// Weights of a triangle filter widened by the scale when shrinking (an antialiased bilinear).
function weights(srcSize, dstSize) {
  const scale = srcSize / dstSize;
  const support = Math.max(1, scale);
  return Array.from({ length: dstSize }, (_, i) => {
    const center = (i + 0.5) * scale;
    const from = Math.max(0, Math.floor(center - support));
    const to = Math.min(srcSize - 1, Math.ceil(center + support));
    const taps = [];
    let sum = 0;
    for (let j = from; j <= to; j++) {
      const w = Math.max(0, 1 - Math.abs(j + 0.5 - center) / support);
      if (w > 0) {
        taps.push([j, w]);
        sum += w;
      }
    }
    return taps.map(([j, w]) => [j, w / sum]);
  });
}

/** Resizes an RGBA image (premultiplied while filtering, so transparent edges don't darken). */
export function resize(img, width, height) {
  const { width: sw, height: sh, data } = img;
  const pre = new Float64Array(sw * sh * 4);
  for (let i = 0; i < sw * sh; i++) {
    const a = data[i * 4 + 3] / 255;
    pre[i * 4] = data[i * 4] * a;
    pre[i * 4 + 1] = data[i * 4 + 1] * a;
    pre[i * 4 + 2] = data[i * 4 + 2] * a;
    pre[i * 4 + 3] = data[i * 4 + 3];
  }
  const wx = weights(sw, width);
  const wy = weights(sh, height);
  const horizontal = new Float64Array(width * sh * 4);
  for (let y = 0; y < sh; y++) {
    for (let x = 0; x < width; x++) {
      for (const [j, w] of wx[x]) {
        for (let c = 0; c < 4; c++) horizontal[(y * width + x) * 4 + c] += pre[(y * sw + j) * 4 + c] * w;
      }
    }
  }
  const out = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const px = [0, 0, 0, 0];
      for (const [j, w] of wy[y]) {
        for (let c = 0; c < 4; c++) px[c] += horizontal[(j * width + x) * 4 + c] * w;
      }
      const a = Math.min(255, Math.max(0, px[3]));
      const o = (y * width + x) * 4;
      for (let c = 0; c < 3; c++) out[o + c] = a > 0 ? Math.round(Math.min(255, Math.max(0, (px[c] * 255) / a))) : 0;
      out[o + 3] = Math.round(a);
    }
  }
  return { width, height, data: out };
}

/** The icon, `share` of the shorter side, centred on a transparent `width` × `height` canvas. */
export function logo(icon, width, height, share) {
  const size = Math.max(1, Math.round(Math.min(width, height) * share));
  const small = resize(icon, size, size);
  const data = Buffer.alloc(width * height * 4);
  const left = Math.floor((width - size) / 2);
  const top = Math.floor((height - size) / 2);
  for (let y = 0; y < size; y++) small.data.copy(data, ((top + y) * width + left) * 4, y * size * 4, (y + 1) * size * 4);
  return { width, height, data };
}

/** Writes every asset of [`ASSETS`] into `outDir`; the file names written. */
export function writeAssets(iconPath, outDir) {
  const icon = decodePng(readFileSync(iconPath));
  mkdirSync(outDir, { recursive: true });
  return ASSETS.map((asset) => {
    const [, , width, height, share] = asset;
    const name = fileName(asset);
    writeFileSync(join(outDir, name), encodePng(logo(icon, width, height, share)));
    return name;
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [icon, out] = process.argv.slice(2);
  if (!icon || !out) {
    console.error("usage: node scripts/msix-assets.mjs <icon.png> <out Assets folder>");
    process.exit(2);
  }
  const names = writeAssets(icon, out);
  console.log(`${out}: ${names.length} logos`);
}
