// 生成"真实地理底图"：从 Natural Earth（公有领域）取海岸线/河流/湖泊，
// 裁到战区范围、降精度，输出一份小 JSON 给前端当底图画。
//
// 为什么可以内置：按 docs/04 的分层，G0 全球底图（海岸线/主要水系/粗地形）归**引擎**，
// 战役省份归**模块**。所以这张底图放服务端，任何模块都能用。
//
// 用法：node tools/mapgen/basemap.js
const fs = require("fs");
const path = require("path");

const BBOX = [112.5, 27.0, 118.6, 31.6]; // 西 南 东 北（比战区外扩一圈）
const BASE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/";
const LAYERS = [
  { name: "coast", file: "ne_50m_coastline.geojson", kind: "line" },
  { name: "rivers", file: "ne_50m_rivers_lake_centerlines.geojson", kind: "line" },
  { name: "lakes", file: "ne_50m_lakes.geojson", kind: "poly" },
];

const inBox = ([x, y]) =>
  x >= BBOX[0] - 0.5 && x <= BBOX[2] + 0.5 && y >= BBOX[1] - 0.5 && y <= BBOX[3] + 0.5;

// 保留 3 位小数（约 100 米），并按最小距离抽稀，去掉视觉上看不出来的点
function simplify(pts, minD = 0.008) {
  const out = [];
  let last = null;
  for (const p of pts) {
    const q = [Math.round(p[0] * 1000) / 1000, Math.round(p[1] * 1000) / 1000];
    if (last && Math.abs(q[0] - last[0]) < minD && Math.abs(q[1] - last[1]) < minD) continue;
    out.push(q);
    last = q;
  }
  return out;
}

function walk(geom, kind) {
  const out = [];
  const push = (coords) => {
    const pts = coords.filter(inBox);
    if (pts.length < 2) return;
    const s = simplify(pts);
    if (s.length >= 2) out.push(s);
  };
  if (geom.type === "LineString") {
    push(geom.coordinates);
  } else if (geom.type === "MultiLineString") {
    for (const l of geom.coordinates) push(l);
  } else if (geom.type === "Polygon") {
    for (const ring of geom.coordinates) push(ring);
  } else if (geom.type === "MultiPolygon") {
    for (const poly of geom.coordinates) for (const ring of poly) push(ring);
  }
  return out;
}

async function main() {
  const result = { bbox: BBOX, source: "Natural Earth 1:50m (public domain)", layers: {} };
  for (const layer of LAYERS) {
    // 优先读本地缓存（tools/mapgen/_geo），没有才联网。
    // 为什么：Node 的 fetch 在这台机器上会被重置，用 PowerShell 下好的文件更稳。
    const local = path.join(__dirname, "_geo", layer.file);
    process.stdout.write(`读取 ${layer.name} … `);
    let gj;
    if (fs.existsSync(local)) {
      gj = JSON.parse(fs.readFileSync(local, "utf8"));
    } else {
      const res = await fetch(BASE + layer.file);
      if (!res.ok) {
        console.log("失败 " + res.status);
        continue;
      }
      gj = await res.json();
    }
    let lines = [];
    for (const f of gj.features) {
      lines = lines.concat(walk(f.geometry, layer.kind));
    }
    result.layers[layer.name] = lines;
    console.log(`${lines.length} 条折线`);
  }

  const out = path.join(__dirname, "..", "..", "crates", "sim-server", "web", "basemap.json");
  const text = JSON.stringify(result);
  fs.writeFileSync(out, text);
  console.log(
    `已写入 ${path.relative(process.cwd(), out)}：${(text.length / 1024).toFixed(0)} KB`
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
