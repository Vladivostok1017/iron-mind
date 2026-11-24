#!/usr/bin/env node
/**
 * terrain.js —— 用真实高程数据（DEM）判定地形，替代手写 terrain。
 *
 * 为什么要做：手写的 `terrain` 只是作者的印象，而 `terrain` 在引擎里**是有后果的**
 * （见 `crates/sim-core/src/world.rs` 的 `Terrain::defense_multiplier` / `move_cost`）：
 * 山地防御 ×1.4、通行 ×2.5，平原 ×1.0、×1.0。地形写错，万家岭就不是万家岭。
 *
 * 数据源：AWS Terrain Tiles（`elevation-tiles-prod` 的 terrarium 编码 PNG，公有领域，
 * 底层是 SRTM/NED 等）。高程藏在 RGB 里：`elev = (R*256 + G + B/256) - 32768`。
 * 瓦片是 Web Mercator 的 256×256 PNG，zoom 10 时一格约 130 米——比"一个省一个色块"
 * 精细三个数量级，足够判"这块地是平原还是山地"。
 *
 * 用法：
 *   node tools/mapgen/terrain.js tiles        # 列出要下载的瓦片（喂给 PowerShell）
 *   node tools/mapgen/terrain.js stat         # 只统计，不改文件（调阈值用）
 *   node tools/mapgen/terrain.js build        # 统计并写回 map.json 的 terrain
 *
 * 瓦片下载（Node 的 fetch 在这个网络里常被重置，交给 PowerShell）：
 *   $list = node tools/mapgen/terrain.js tiles | ConvertFrom-Json
 *   $list | ForEach-Object -Parallel { ... Invoke-WebRequest ... } -ThrottleLimit 8
 */

"use strict";

const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const ROOT = path.resolve(__dirname, "..", "..");
const MODULE = path.join(ROOT, "content", "scenarios", "nanxun_1938.imod");
const MAP = path.join(MODULE, "map.json");
const GEO = path.join(MODULE, "provinces_geo.json");
const TILES = path.join(__dirname, "_geo", "terrarium");

const Z = 10;
const TILE_PX = 256;

/* ---------------------------------------------------------------- PNG 解码 */

/** 解码 8 位、非隔行的 RGB/RGBA/灰度 PNG。够用就行（terrarium 全是 RGB）。 */
function decodePng(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not png");
  let o = 8;
  let w = 0, h = 0, depth = 0, color = 0, interlace = 0;
  const idat = [];
  while (o < buf.length) {
    const len = buf.readUInt32BE(o);
    const type = buf.toString("latin1", o + 4, o + 8);
    const body = buf.subarray(o + 8, o + 8 + len);
    if (type === "IHDR") {
      w = body.readUInt32BE(0);
      h = body.readUInt32BE(4);
      depth = body[8];
      color = body[9];
      interlace = body[12];
    } else if (type === "IDAT") {
      idat.push(body);
    } else if (type === "IEND") break;
    o += 12 + len;
  }
  if (depth !== 8) throw new Error("unsupported bit depth " + depth);
  if (interlace !== 0) throw new Error("interlaced png unsupported");
  const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[color];
  if (!channels) throw new Error("unsupported color type " + color);

  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * channels;
  const out = Buffer.alloc(h * stride);
  let p = 0;
  for (let y = 0; y < h; y++) {
    const filter = raw[p++];
    const line = raw.subarray(p, p + stride);
    p += stride;
    const cur = out.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? cur[x - channels] : 0;
      const b = prev ? prev[x] : 0;
      const c = prev && x >= channels ? prev[x - channels] : 0;
      let v = line[x];
      switch (filter) {
        case 0: break;
        case 1: v += a; break;
        case 2: v += b; break;
        case 3: v += (a + b) >> 1; break;
        case 4: {
          const pp = a + b - c;
          const pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c);
          v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
          break;
        }
        default: throw new Error("bad filter " + filter);
      }
      cur[x] = v & 0xff;
    }
  }
  return { w, h, channels, data: out };
}

/* ------------------------------------------------------- 瓦片 / 高程取值 */

const cache = new Map();

function tilePath(z, x, y) {
  return path.join(TILES, String(z), String(x), y + ".png");
}

function loadTile(z, x, y) {
  const key = z + "/" + x + "/" + y;
  if (cache.has(key)) return cache.get(key);
  let img = null;
  const f = tilePath(z, x, y);
  if (fs.existsSync(f)) {
    try {
      img = decodePng(fs.readFileSync(f));
      img.z = z;
      img.x = x;
      img.y = y;
    } catch (e) {
      process.stderr.write("warn: " + f + ": " + e.message + "\n");
      img = null;
    }
  }
  cache.set(key, img);
  return img;
}

function lonToGx(lon, z) {
  return ((lon + 180) / 360) * TILE_PX * 2 ** z;
}
function latToGy(lat, z) {
  const r = (lat * Math.PI) / 180;
  return (
    ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) *
    TILE_PX *
    2 ** z
  );
}

let missCount = 0;

/** 取 (lon, lat) 处海拔（米）。瓦片缺失返回 null。 */
function elevAt(lon, lat) {
  const z = Z;
  const gx = lonToGx(lon, z);
  const gy = latToGy(lat, z);
  const tx = Math.floor(gx / TILE_PX);
  const ty = Math.floor(gy / TILE_PX);
  const img = loadTile(z, tx, ty);
  if (!img) {
    missCount++;
    return null;
  }
  const px = Math.min(TILE_PX - 1, Math.floor(gx - tx * TILE_PX));
  const py = Math.min(TILE_PX - 1, Math.floor(gy - ty * TILE_PX));
  const i = (py * img.w + px) * img.channels;
  const r = img.data[i], g = img.data[i + 1], b = img.data[i + 2];
  return r * 256 + g + b / 256 - 32768;
}

/** 一个像素在东西方向约多少米。 */
function pixelMeters(lat, z) {
  return (156543.03392 * Math.cos((lat * Math.PI) / 180)) / 2 ** z;
}

/** 当地坡度（rise/run，无量纲）。中心差分，160 米基线。 */
function slopeAt(lon, lat) {
  const d = pixelMeters(lat, Z) / 111320; // 1 像素约多少度经度
  const h0 = elevAt(lon, lat);
  const hE = elevAt(lon + d, lat);
  const hW = elevAt(lon - d, lat);
  const hN = elevAt(lon, lat + d);
  const hS = elevAt(lon, lat - d);
  if (h0 == null || hE == null || hW == null || hN == null || hS == null) return null;
  const dx = 2 * pixelMeters(lat, Z);
  const dy = 2 * pixelMeters(lat, Z);
  return Math.hypot(hE - hW, hN - hS) / Math.hypot(dx, dy) / 2;
}

/* --------------------------------------------------------------- 多边形工具 */

function bboxOf(ring) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of ring) {
    if (x < x0) x0 = x;
    if (y < y0) y0 = y;
    if (x > x1) x1 = x;
    if (y > y1) y1 = y;
  }
  return [x0, y0, x1, y1];
}

/** 射线法。边界上的点结果不稳定，但我们要的是"平均海拔"，无所谓。 */
function inside(ring, x, y) {
  let hit = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const xi = ring[i][0], yi = ring[i][1];
    const xj = ring[j][0], yj = ring[j][1];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
  }
  return hit;
}

/* ---------------------------------------------------------------- 地形判定 */

/**
 * 从高程统计推出地形。
 *
 * 长江中下游的实情：平原 10-50 米、相对高差几十米；丘陵 100-500 米、沟谷切割；
 * 山地 500 米以上、相对高差数百米；滨湖低洼（<25 米、几乎无高差）是水网沼泽。
 * 所以同时看**海拔**和**相对高差**：海拔低但高差大的地方依旧是山地（峡谷）。
 */
function classify(s) {
  const { mean, relief, steep, p5 } = s;
  if (p5 < 18 && mean < 30 && relief < 35) return "marsh";
  if (mean >= 320 || relief >= 300 || steep >= 0.24) return "mountain";
  if (mean >= 95 || relief >= 130 || steep >= 0.10) return "hill";
  return "plain";
}

/* --------------------------------------------------------------------- 采样 */

function summarise(vals, slopes) {
  vals.sort((a, b) => a - b);
  const n = vals.length;
  const q = (p) => vals[Math.min(n - 1, Math.max(0, Math.round(p * (n - 1))))];
  let sum = 0;
  for (const v of vals) sum += v;
  const mean = sum / n;
  let sd = 0;
  for (const v of vals) sd += (v - mean) ** 2;
  const steep = slopes.length
    ? slopes.filter((v) => v > 0.12).length / slopes.length
    : 0;
  return {
    n,
    mean,
    min: vals[0],
    p5: q(0.05),
    p50: q(0.5),
    p95: q(0.95),
    max: vals[n - 1],
    relief: q(0.95) - q(0.05),
    sd: Math.sqrt(sd / n),
    steep,
    slopeMean: slopes.length ? slopes.reduce((a, b) => a + b, 0) / slopes.length : 0,
  };
}

function sampleProvince(ring) {
  const [x0, y0, x1, y1] = bboxOf(ring);
  const area = (x1 - x0) * (y1 - y0);
  const target = 1600;
  let step = Math.sqrt(area / target);
  if (step < 0.004) step = 0.004;
  const vals = [];
  const slopes = [];
  for (let y = y0 + step / 2; y < y1; y += step) {
    for (let x = x0 + step / 2; x < x1; x += step) {
      if (!inside(ring, x, y)) continue;
      const e = elevAt(x, y);
      if (e == null) continue;
      vals.push(e);
      if (vals.length % 3 === 0) {
        const sl = slopeAt(x, y);
        if (sl != null) slopes.push(sl);
      }
    }
  }
  return vals.length >= 8 ? summarise(vals, slopes) : null;
}

/* --------------------------------------------------------------------- 主流程 */

function tiles() {
  const map = JSON.parse(fs.readFileSync(MAP, "utf8"));
  const [w, s, e, n] = map.bbox;
  const m = 0.15;
  const out = [];
  for (let x = Math.floor(lonToGx(w - m, Z) / TILE_PX); x <= Math.floor(lonToGx(e + m, Z) / TILE_PX); x++) {
    for (let y = Math.floor(latToGy(n + m, Z) / TILE_PX); y <= Math.floor(latToGy(s - m, Z) / TILE_PX); y++) {
      out.push({
        z: Z,
        x,
        y,
        dir: path.join(TILES, String(Z), String(x)),
        file: tilePath(Z, x, y),
        url: `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/${Z}/${x}/${y}.png`,
      });
    }
  }
  process.stdout.write(JSON.stringify(out));
}

function run(write) {
  const map = JSON.parse(fs.readFileSync(MAP, "utf8"));
  const geo = fs.existsSync(GEO)
    ? JSON.parse(fs.readFileSync(GEO, "utf8")).polygons
    : {};

  const rows = [];
  for (const p of map.provinces) {
    const rings = geo[String(p.id)];
    const ring = rings && rings[0];
    const authored = p.terrain;
    let st = null;
    if (ring && ring.length >= 3) st = sampleProvince(ring);
    if (!st) {
      rows.push({ id: p.id, name: p.name, authored, next: authored, note: "无轮廓/无高程，保持原值" });
      continue;
    }
    const next =
      authored === "water" || authored === "urban" ? authored : classify(st);
    rows.push({ id: p.id, name: p.name, authored, next, st });
  }

  rows.sort((a, b) => a.id - b.id);
  // 中文算两格、ASCII 算一格，否则表头永远对不齐
  const width = (s) =>
    [...String(s)].reduce((a, ch) => a + (/[\u2e80-\uffff]/.test(ch) ? 2 : 1), 0);
  const pad = (s, n) => String(s) + " ".repeat(Math.max(0, n - width(s)));
  const num = (v, d = 0) => {
    const f = 10 ** d;
    return String(Math.round(v * f) / f).padStart(6);
  };
  process.stdout.write(
    pad("id", 4) + pad("省", 10) + pad("原", 8) + pad("新", 9) +
      "  海拔均值  p5  p95 高差  坡度  陡坡比\n"
  );
  for (const r of rows) {
    if (!r.st) {
      process.stdout.write(pad(r.id, 4) + pad(r.name, 10) + pad(r.authored, 8) + pad(r.next, 9) + "  " + r.note + "\n");
      continue;
    }
    const s = r.st;
    const mark = r.authored === r.next ? "  " : " *";
    process.stdout.write(
      pad(r.id, 4) + pad(r.name, 10) + pad(r.authored, 8) + pad(r.next, 9) + mark +
        num(s.mean) + num(s.p5) + num(s.p95) + num(s.relief) +
        num(s.slopeMean, 3) + num(s.steep, 2) + "\n"
    );
  }

  const hist = {};
  for (const r of rows) hist[r.next] = (hist[r.next] || 0) + 1;
  process.stdout.write("\n新地形分布: " + JSON.stringify(hist) + "\n");
  process.stdout.write("缺失瓦片取点: " + missCount + "\n");
  process.stdout.write(
    "变化: " + rows.filter((r) => r.authored !== r.next).length + " / " + rows.length + "\n"
  );

  if (write) {
    for (const r of rows) {
      const p = map.provinces.find((x) => x.id === r.id);
      p.terrain = r.next;
      if (r.st) {
        p.elev = Math.round(r.st.mean);
        p.relief = Math.round(r.st.relief);
      }
    }
    // 键序固定，diff 才读得懂
    const order = [
      "id", "name", "terrain", "elev", "relief", "owner", "supply_throughput",
      "neighbors", "lon", "lat", "fort", "supply_source", "prototype",
      "positions", "note",
    ];
    map.provinces = map.provinces.map((p) => {
      const out = {};
      for (const k of order) if (k in p) out[k] = p[k];
      for (const k of Object.keys(p)) if (!(k in out)) out[k] = p[k];
      return out;
    });
    fs.writeFileSync(MAP, JSON.stringify(map, null, 2) + "\n");
    const rep = path.join(__dirname, "_geo", "terrain_report.json");
    fs.writeFileSync(
      rep,
      JSON.stringify(
        rows.map((r) => ({
          id: r.id,
          name: r.name,
          authored: r.authored,
          terrain: r.next,
          ...(r.st || {}),
        })),
        null,
        1
      ) + "\n"
    );
    process.stdout.write("已写回 " + path.relative(ROOT, MAP) + "\n");
    process.stdout.write("报告 " + path.relative(ROOT, rep) + "\n");
  }
}

const cmd = process.argv[2] || "stat";
if (cmd === "tiles") tiles();
else if (cmd === "stat") run(false);
else if (cmd === "build") run(true);
else {
  process.stderr.write("usage: terrain.js [tiles|stat|build]\n");
  process.exit(2);
}
