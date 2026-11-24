// 用真实行政区划边界替换 Voronoi 色块。
//
// 数据：阿里云 DataV 行政区划（省 → 地级市 → 县区，GeoJSON），只取战区范围内的县。
// 做法：
//   1. 拉江西省全部地级市，再拉每个市下辖的县区；同样拉湖北东南与安徽安庆
//   2. 每个模块省份点落到哪个县，就用那个县的**真实边界**
//   3. 一个县里有多个省份点（比如万家岭/张古山/扁担山都在德安），
//      就在该县内部再做一次 Voronoi 细分——外轮廓是真的，内部按我们的划分切
//   4. 水系省份（长江段、鄱阳湖）不给面，前端仍按"水道带"画
//
// 用法：node tools/mapgen/boundaries.js
const fs = require("fs");
const path = require("path");

const MOD = path.join(__dirname, "..", "..", "content", "scenarios", "nanxun_1938.imod");
const CACHE = path.join(__dirname, "_geo");
const BBOX = [113.0, 27.5, 117.8, 31.0]; // 比战区略外扩
const SIMPLIFY = 0.008; // 度，约 900 米

// 战区内的地级市（adcode）
const PREFECTURES = [
  360100, 360200, 360300, 360400, 360500, 360600, 360700, 360800, 360900, 361000, 361100, // 江西
  420100, 420200, 420700, 421100, 421200, // 鄂东
  340800, // 安庆
  430600, // 岳阳（临湘在这）
];

const cachePath = (u) => path.join(CACHE, u.replace(/[^0-9a-z_.]/gi, "_"));
async function getJSON(u) {
  const f = cachePath(u);
  if (fs.existsSync(f)) return JSON.parse(fs.readFileSync(f, "utf8"));
  const res = await fetch(u);
  if (!res.ok) throw new Error("HTTP " + res.status + " " + u);
  const j = await res.json();
  fs.mkdirSync(CACHE, { recursive: true });
  fs.writeFileSync(f, JSON.stringify(j));
  return j;
}

// ---- 几何小工具 ----
function ringsOf(geom) {
  if (!geom) return [];
  if (geom.type === "Polygon") return [geom.coordinates];
  if (geom.type === "MultiPolygon") return geom.coordinates;
  return [];
}
function inBox([x, y], b = BBOX) {
  return x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3];
}
function pointInRing(pt, ring) {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j];
    if (yi > pt[1] !== yj > pt[1] && pt[0] < ((xj - xi) * (pt[1] - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }
  return inside;
}
function pointInPolygon(pt, poly) {
  if (!pointInRing(pt, poly[0])) return false;
  for (let i = 1; i < poly.length; i++) if (pointInRing(pt, poly[i])) return false;
  return true;
}
// Douglas-Peucker
function dp(points, eps) {
  if (points.length < 3) return points;
  let maxD = 0, idx = 0;
  const [x1, y1] = points[0], [x2, y2] = points[points.length - 1];
  const dx = x2 - x1, dy = y2 - y1, len = Math.hypot(dx, dy) || 1e-9;
  for (let i = 1; i < points.length - 1; i++) {
    const d = Math.abs(dy * points[i][0] - dx * points[i][1] + x2 * y1 - y2 * x1) / len;
    if (d > maxD) { maxD = d; idx = i; }
  }
  if (maxD <= eps) return [points[0], points[points.length - 1]];
  return dp(points.slice(0, idx + 1), eps).slice(0, -1).concat(dp(points.slice(idx), eps));
}
function simplifyRing(ring, eps) {
  const s = dp(ring, eps).map((p) => [Math.round(p[0] * 1e4) / 1e4, Math.round(p[1] * 1e4) / 1e4]);
  return s.length >= 3 ? s : ring.map((p) => [Math.round(p[0] * 1e4) / 1e4, Math.round(p[1] * 1e4) / 1e4]);
}
// 半平面裁剪（Voronoi 用）
function clipHalf(poly, a, b) {
  const mx = (a[0] + b[0]) / 2, my = (a[1] + b[1]) / 2;
  const nx = b[0] - a[0], ny = b[1] - a[1];
  const f = (p) => (p[0] - mx) * nx + (p[1] - my) * ny; // <0 = 靠近 a
  const out = [];
  for (let i = 0; i < poly.length; i++) {
    const cur = poly[i], prev = poly[(i + poly.length - 1) % poly.length];
    const fc = f(cur), fp = f(prev);
    if (fc <= 0) {
      if (fp > 0) {
        const t = fp / (fp - fc);
        out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
      }
      out.push(cur);
    } else if (fp <= 0) {
      const t = fp / (fp - fc);
      out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
    }
  }
  return out;
}

async function main() {
  const map = JSON.parse(fs.readFileSync(path.join(MOD, "map.json"), "utf8"));
  const points = map.provinces.filter((p) => p.terrain !== "water");

  // 1. 收集战区内的县
  const counties = [];
  for (const ad of PREFECTURES) {
    const pref = await getJSON(
      `https://geo.datav.aliyun.com/areas_v3/bound/${ad}_full.json`
    );
    for (const f of pref.features || []) {
      const name = f.properties.name;
      for (const poly of ringsOf(f.geometry)) {
        if (!poly[0].some((pt) => inBox(pt))) continue;
        counties.push({ name, poly });
      }
    }
    process.stdout.write(`.`);
  }
  console.log(`\n得到 ${counties.length} 个县级多边形`);

  // 2. 每个省点落进哪个县
  const byCounty = new Map();
  const orphan = [];
  for (const p of points) {
    const pt = [p.lon, p.lat];
    let hit = null;
    for (const c of counties) {
      if (pointInPolygon(pt, c.poly)) { hit = c; break; }
    }
    if (!hit) {
      // 容错：找不到包含它的县，就取最近县中心
      let best = null, bestD = Infinity;
      for (const c of counties) {
        const cc = c.poly[0][0];
        const d = (cc[0] - pt[0]) ** 2 + (cc[1] - pt[1]) ** 2;
        if (d < bestD) { bestD = d; best = c; }
      }
      if (!best) { orphan.push(p.name); continue; }
      hit = best;
      orphan.push(p.name + "→" + best.name);
    }
    const key = hit.name + "#" + hit.poly[0].length;
    if (!byCounty.has(key)) byCounty.set(key, { county: hit, members: [] });
    byCounty.get(key).members.push(p);
  }
  if (orphan.length) console.log("靠最近县兜底的：", orphan.join("、"));

  // 3. 生成每个省的真实轮廓
  const geo = {};
  let shared = 0;
  for (const { county, members } of byCounty.values()) {
    const outer = simplifyRing(county.poly[0], SIMPLIFY);
    if (members.length === 1) {
      geo[members[0].id] = [outer];
      continue;
    }
    // 同县多省：在县内做 Voronoi 细分
    shared++;
    for (const m of members) {
      let cell = outer;
      for (const other of members) {
        if (other === m) continue;
        cell = clipHalf(cell, [m.lon, m.lat], [other.lon, other.lat]);
        if (cell.length < 3) break;
      }
      if (cell.length >= 3) {
        geo[m.id] = [cell.map((q) => [Math.round(q[0] * 1e4) / 1e4, Math.round(q[1] * 1e4) / 1e4])];
      } else {
        geo[m.id] = [outer];
      }
    }
  }

  const missing = points.filter((p) => !geo[p.id]).map((p) => p.name);
  const out = path.join(MOD, "provinces_geo.json");
  const text = JSON.stringify({ source: "DataV 行政区划（县界）", simplify: SIMPLIFY, polygons: geo });
  fs.writeFileSync(out, text);
  console.log(
    `已写入 ${path.basename(out)}：${Object.keys(geo).length} 个省有真实轮廓，` +
      `${shared} 个县被多省细分，${missing.length} 个缺失` +
      (missing.length ? "（" + missing.join("、") + "）" : "") +
      `，${(text.length / 1024).toFixed(0)} KB`
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
