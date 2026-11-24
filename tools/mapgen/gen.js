#!/usr/bin/env node
// 战役地图生成器（模组作者工具，不参与模拟）
// ---------------------------------------------------------------------------
// 输入：一份"地名 + 经纬度 + 地形归属"的清单（下面的 PLACES）。
// 输出：map.json，邻接由 Delaunay 三角剖分自动生成。
//
// 为什么用 Delaunay：前端画省份多边形用的是 Voronoi（docs/04 附录 B）。
// Voronoi 的邻接关系**恰好**就是 Delaunay 的边。所以只要邻接表来自 Delaunay，
// 地图上"看起来贴着"的两块，在模拟里就是"走得通"的两块——视觉和规则天然一致。
// 手工写邻接表做不到这一点，而且 A 写了 B、B 忘了 A 就会触发 map.asymmetric_neighbor。
//
// 用法：
//   node tools/mapgen/gen.js            # 生成并写入模块目录
//   node tools/mapgen/gen.js --dry      # 只打印统计，不写文件
//
// 注意：这是**一次性引导工具**。生成之后 map.json 就是唯一真相，
// 后续手改省份不必再跑它（跑一次会覆盖手改）。要加省，直接改 map.json 更快。

const fs = require("fs");
const path = require("path");

// --- 地名清单 ---------------------------------------------------------------
// n 名称 / t 地形 / lon lat 经纬度 / own 初始归属 / f 工事 0-5
// sup 补给源 / th 补给吞吐 0-1 / p 地形原型 / note 出处备注
const PLACES = [
  // ===== 南浔铁路走廊：九江 → 南昌 =====
  { n: "九江", t: "urban", lon: 115.99, lat: 29.71, own: "jp", f: 3, sup: 1, th: 1.0, note: "第11军前进基地，长江与南浔铁路交点" },
  { n: "沙河", t: "hill", lon: 115.94, lat: 29.63, own: "jp", th: 0.9, note: "南浔铁路车站" },
  { n: "黄老门", t: "hill", lon: 115.89, lat: 29.55, own: "jp", th: 0.8 },
  { n: "马回岭", t: "hill", lon: 115.84, lat: 29.47, own: "jp", th: 0.8, note: "薛岳兵团反攻出发地" },
  { n: "德安", t: "urban", lon: 115.76, lat: 29.32, own: "cn", f: 2, th: 0.9, note: "南浔线中段枢纽，万家岭战役的支点",
    pos: [["城垣", "urban", 0.5], ["车站", "plain", 0.8], ["北郊", "hill", 1.0]] },
  { n: "永修", t: "plain", lon: 115.81, lat: 29.02, own: "cn", th: 0.8 },
  { n: "涂家埠", t: "marsh", lon: 115.76, lat: 28.93, own: "cn", th: 0.7, note: "修水与赣江交汇，铁路桥" },
  { n: "南昌", t: "urban", lon: 115.89, lat: 28.68, own: "cn", f: 2, sup: 1, th: 1.0, note: "第9战区后方枢纽",
    pos: [["城垣", "urban", 0.5], ["赣江渡口", "marsh", 0.7]] },
  { n: "吴城", t: "marsh", lon: 115.99, lat: 29.18, own: "cn", th: 0.6, note: "赣江入鄱阳湖口，水陆要冲" },
  { n: "柘林", t: "marsh", lon: 115.48, lat: 29.04, own: "cn", th: 0.5 },
  { n: "白槎", t: "hill", lon: 115.60, lat: 29.10, own: "cn", th: 0.6 },
  { n: "虬津", t: "plain", lon: 115.67, lat: 29.00, own: "cn", th: 0.7 },

  // ===== 庐山与星子方向 =====
  { n: "姑塘", t: "marsh", lon: 116.14, lat: 29.60, own: "jp", th: 0.8, p: "beachhead", note: "日军登陆点，鄱阳湖西岸" },
  { n: "星子", t: "hill", lon: 116.04, lat: 29.45, own: "cn", f: 1, th: 0.8, note: "第101师团沿湖进攻的起点" },
  { n: "隘口", t: "mountain", lon: 115.96, lat: 29.41, own: "cn", p: "ridge_line", note: "庐山南麓隘路" },
  { n: "东牯岭", t: "mountain", lon: 116.02, lat: 29.34, own: "cn", p: "isolated_height" },
  { n: "牯岭", t: "mountain", lon: 115.98, lat: 29.56, own: "cn", p: "isolated_height", note: "庐山山城，俯瞰九江" },

  // ===== 万家岭包围圈 =====
  { n: "金官桥", t: "hill", lon: 115.84, lat: 29.37, own: "cn", th: 0.7 },
  // 万家岭：一个省，三个阵地。这就是"加层"——
  // 张古山、扁担山、哔叽岭相距不到 8 公里，做成三个省会打不出"反复易手"，
  // 做成一个省的三个阵地才有战场宽度可言（窄阵地投不进多余的兵）。
  { n: "万家岭", t: "mountain", lon: 115.66, lat: 29.31, own: "cn", p: "encircle_gap", note: "第106师团被围处",
    pos: [["张古山", "mountain", 0.4], ["扁担山", "mountain", 0.6], ["哔叽岭", "mountain", 0.5]] },
  { n: "麒麟峰", t: "mountain", lon: 115.75, lat: 29.24, own: "cn", p: "ridge_line" },
  { n: "甘木关", t: "mountain", lon: 115.55, lat: 29.28, own: "cn", p: "river_ridge" },
  { n: "箬溪", t: "hill", lon: 115.49, lat: 29.21, own: "cn" },
  { n: "跑马岭", t: "hill", lon: 115.56, lat: 29.14, own: "cn" },
  { n: "白水街", t: "plain", lon: 115.61, lat: 29.19, own: "cn", p: "village_node" },

  // ===== 西翼：瑞昌 → 阳新 → 大冶 → 鄂城 =====
  { n: "瑞昌", t: "hill", lon: 115.70, lat: 29.67, own: "cn", f: 1, th: 0.9, p: "city_gate", note: "第9师团西进的门户，8月下旬才失守",
    pos: [["城垣", "urban", 0.4], ["东门", "hill", 0.6]] },
  { n: "码头镇", t: "plain", lon: 115.58, lat: 29.71, own: "cn", th: 0.8, note: "长江南岸码头" },
  { n: "富池口", t: "hill", lon: 115.43, lat: 29.84, own: "cn", th: 0.7, note: "富水入江口" },
  { n: "阳新", t: "urban", lon: 115.21, lat: 29.84, own: "cn", f: 1, th: 0.8 },
  { n: "木石港", t: "hill", lon: 115.32, lat: 29.74, own: "cn", th: 0.6 },
  { n: "大冶", t: "urban", lon: 114.99, lat: 30.10, own: "cn", f: 1, th: 0.8, note: "铁矿" },
  { n: "黄石港", t: "urban", lon: 115.06, lat: 30.21, own: "cn", th: 0.8, note: "长江港口" },
  { n: "金牛", t: "hill", lon: 114.56, lat: 30.09, own: "cn", th: 0.6 },
  { n: "鄂城", t: "urban", lon: 114.90, lat: 30.40, own: "cn", th: 0.8 },
  { n: "金口", t: "plain", lon: 114.15, lat: 30.31, own: "cn", th: 0.7 },

  // ===== 粤汉铁路：咸宁 → 武昌 =====
  { n: "咸宁", t: "hill", lon: 114.33, lat: 29.87, own: "cn", f: 1, th: 0.8 },
  { n: "汀泗桥", t: "plain", lon: 114.29, lat: 29.96, own: "cn", th: 0.8, note: "北伐旧战场" },
  { n: "贺胜桥", t: "plain", lon: 114.36, lat: 30.04, own: "cn", th: 0.8, p: "road_cut", note: "公路与铁路并行，纵队易被横切" },
  { n: "蒲圻", t: "hill", lon: 113.91, lat: 29.73, own: "cn", th: 0.7 },
  { n: "嘉鱼", t: "marsh", lon: 113.95, lat: 29.98, own: "cn", th: 0.6 },
  { n: "崇阳", t: "hill", lon: 114.05, lat: 29.56, own: "cn", th: 0.6 },
  { n: "通山", t: "mountain", lon: 114.49, lat: 29.61, own: "cn", th: 0.5 },
  { n: "通城", t: "mountain", lon: 113.83, lat: 29.26, own: "cn", th: 0.5 },

  // ===== 武汉三镇 =====
  { n: "武昌", t: "urban", lon: 114.31, lat: 30.55, own: "cn", f: 4, sup: 1, th: 0.9, note: "行营与第9战区指挥部" },
  { n: "汉阳", t: "urban", lon: 114.23, lat: 30.54, own: "cn", f: 3, th: 0.9, note: "兵工厂" },
  { n: "汉口", t: "urban", lon: 114.28, lat: 30.60, own: "cn", f: 3, th: 0.9, note: "租界与外贸枢纽" },

  // ===== 江北战场：黄梅 → 田家镇 → 潜山 =====
  { n: "黄梅", t: "plain", lon: 115.94, lat: 30.08, own: "jp", sup: 1, th: 0.7, note: "第6师团登陆后集结，江北桥头堡" },
  { n: "梅川", t: "hill", lon: 115.62, lat: 30.07, own: "cn", th: 0.7, note: "广济县城" },
  { n: "武穴", t: "urban", lon: 115.56, lat: 29.86, own: "cn", th: 0.8, p: "beachhead" },
  { n: "龙坪", t: "marsh", lon: 115.62, lat: 29.94, own: "cn", th: 0.6 },
  { n: "宿松", t: "hill", lon: 116.13, lat: 30.16, own: "jp", th: 0.7 },
  { n: "太湖", t: "hill", lon: 116.31, lat: 30.45, own: "jp", th: 0.6 },
  { n: "潜山", t: "hill", lon: 116.57, lat: 30.63, own: "jp", th: 0.6 },
  { n: "望江", t: "marsh", lon: 116.70, lat: 30.13, own: "jp", th: 0.5 },

  // ===== 南翼：修水 / 武宁 / 铜鼓 / 奉新 =====
  { n: "修水", t: "hill", lon: 114.56, lat: 29.03, own: "cn", f: 1, th: 0.6, note: "第30集团军侧击出发地" },
  { n: "武宁", t: "hill", lon: 115.10, lat: 29.26, own: "cn", th: 0.7, note: "第30集团军与第1兵团结合部" },
  { n: "铜鼓", t: "mountain", lon: 114.38, lat: 28.53, own: "cn", th: 0.4 },
  { n: "靖安", t: "hill", lon: 115.36, lat: 28.86, own: "cn", th: 0.6 },
  { n: "奉新", t: "plain", lon: 115.39, lat: 28.69, own: "cn", th: 0.7 },
  { n: "安义", t: "plain", lon: 115.56, lat: 28.85, own: "cn", th: 0.7 },
  { n: "高安", t: "plain", lon: 115.38, lat: 28.42, own: "cn", f: 1, sup: 1, th: 0.8, note: "赣西粮道" },
  { n: "上高", t: "hill", lon: 114.93, lat: 28.24, own: "cn", th: 0.6 },
  { n: "万载", t: "hill", lon: 114.45, lat: 28.10, own: "cn", th: 0.5 },
  { n: "宜丰", t: "hill", lon: 114.79, lat: 28.39, own: "cn", th: 0.5 },
  { n: "樟树", t: "plain", lon: 115.55, lat: 28.05, own: "cn", th: 0.7 },
  { n: "丰城", t: "plain", lon: 115.78, lat: 28.16, own: "cn", th: 0.7 },

  // ===== 赣东：鄱阳湖东岸 =====
  { n: "都昌", t: "marsh", lon: 116.21, lat: 29.27, own: "cn", th: 0.5 },
  { n: "鄱阳", t: "marsh", lon: 116.71, lat: 29.00, own: "cn", th: 0.6 },
  { n: "余干", t: "marsh", lon: 116.70, lat: 28.70, own: "cn", th: 0.6 },
  { n: "万年", t: "hill", lon: 117.06, lat: 28.69, own: "cn", th: 0.5 },
  { n: "景德镇", t: "hill", lon: 117.21, lat: 29.28, own: "cn", th: 0.6 },
  { n: "乐平", t: "hill", lon: 117.14, lat: 28.97, own: "cn", th: 0.5 },
  { n: "抚州", t: "plain", lon: 116.36, lat: 27.95, own: "cn", th: 0.7 },
  { n: "进贤", t: "plain", lon: 116.25, lat: 28.37, own: "cn", th: 0.6 },
  { n: "东乡", t: "hill", lon: 116.62, lat: 28.25, own: "cn", th: 0.5 },

  // ===== 湘北：岳阳方向 =====
  { n: "临湘", t: "hill", lon: 113.46, lat: 29.48, own: "cn", th: 0.6 },

  // ===== 长江水道（water）=====
  { n: "长江·湖口", t: "water", lon: 116.24, lat: 29.76, own: "jp", th: 1.0 },
  { n: "长江·九江", t: "water", lon: 116.02, lat: 29.79, own: "jp", th: 1.0 },
  { n: "长江·瑞昌", t: "water", lon: 115.68, lat: 29.79, own: "jp", th: 1.0 },
  { n: "长江·富池", t: "water", lon: 115.42, lat: 29.93, own: "jp", th: 1.0 },
  { n: "长江·田家镇", t: "water", lon: 115.44, lat: 30.02, own: "jp", th: 1.0 },
  { n: "长江·武穴", t: "water", lon: 115.57, lat: 30.04, own: "jp", th: 1.0 },
  { n: "长江·黄石", t: "water", lon: 115.10, lat: 30.27, own: "jp", th: 1.0 },
  { n: "长江·鄂城", t: "water", lon: 114.96, lat: 30.46, own: "jp", th: 1.0 },
  { n: "长江·武昌", t: "water", lon: 114.36, lat: 30.63, own: "jp", th: 1.0 },
  { n: "鄱阳湖·北口", t: "water", lon: 116.18, lat: 29.58, own: "jp", th: 1.0 },
  { n: "鄱阳湖·中", t: "water", lon: 116.33, lat: 29.40, own: "jp", th: 1.0 },
  { n: "鄱阳湖·南", t: "water", lon: 116.39, lat: 29.10, own: "jp", th: 1.0 },
];

// --- 邻接参数 ---------------------------------------------------------------
const MAX_EDGE_KM = 62; // 超过这个距离的 Delaunay 边不算相邻（防止跨界长边）
const MIN_PAIR_KM = 3; // 两点过近会切出退化多边形，报警（密集山头允许到 3km）

// --- 几何 -------------------------------------------------------------------
const KMLAT = 111.0;
function kmPerLon(lat) {
  return 111.0 * Math.cos((lat * Math.PI) / 180);
}
// 投影到平面（经度按中纬度压缩），供三角剖分使用
const midLat = PLACES.reduce((s, p) => s + p.lat, 0) / PLACES.length;
const kx = kmPerLon(midLat);
const pts = PLACES.map((p) => ({ x: (p.lon - 115.5) * kx, y: (p.lat - 29.2) * KMLAT }));

function distKm(i, j) {
  const dx = pts[i].x - pts[j].x;
  const dy = pts[i].y - pts[j].y;
  return Math.sqrt(dx * dx + dy * dy);
}

function circumcircle(V, a, b, c) {
  const ax = V[a].x, ay = V[a].y;
  const bx = V[b].x, by = V[b].y;
  const cx = V[c].x, cy = V[c].y;
  const d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
  if (Math.abs(d) < 1e-12) {
    // 共线：给一个巨大的外接圆，让这个三角形自然被淘汰
    return { x: 0, y: 0, r: 1e9 };
  }
  const ux =
    ((ax * ax + ay * ay) * (by - cy) +
      (bx * bx + by * by) * (cy - ay) +
      (cx * cx + cy * cy) * (ay - by)) / d;
  const uy =
    ((ax * ax + ay * ay) * (cx - bx) +
      (bx * bx + by * by) * (ax - cx) +
      (cx * cx + cy * cy) * (bx - ax)) / d;
  const r = Math.sqrt((ax - ux) ** 2 + (ay - uy) ** 2);
  return { x: ux, y: uy, r };
}

// --- Bowyer-Watson ----------------------------------------------------------
function delaunay() {
  const n = pts.length;
  // 超级三角形
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const p of pts) {
    minX = Math.min(minX, p.x); maxX = Math.max(maxX, p.x);
    minY = Math.min(minY, p.y); maxY = Math.max(maxY, p.y);
  }
  const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2;
  const dm = Math.max(maxX - minX, maxY - minY) * 20 + 10;
  const verts = pts.concat([
    { x: cx - dm, y: cy - dm },
    { x: cx + dm, y: cy - dm },
    { x: cx, y: cy + dm },
  ]);
  const superIdx = [n, n + 1, n + 2];
  let tris = [[n, n + 1, n + 2]];

  for (let i = 0; i < n; i++) {
    const p = verts[i];
    const kept = [];
    const edgeCount = new Map();
    for (const t of tris) {
      const cc = circumcircle(verts, t[0], t[1], t[2]);
      const inside =
        (p.x - cc.x) ** 2 + (p.y - cc.y) ** 2 < cc.r * cc.r - 1e-9;
      if (inside) {
        const es = [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]];
        for (const [u, v] of es) {
          const k = u < v ? `${u},${v}` : `${v},${u}`;
          edgeCount.set(k, (edgeCount.get(k) || 0) + 1);
        }
      } else {
        kept.push(t);
      }
    }
    tris = kept;
    for (const [k, c] of edgeCount) {
      if (c === 1) {
        const [u, v] = k.split(",").map(Number);
        tris.push([u, v, i]);
      }
    }
  }
  // 去掉含超级顶点的三角形
  return tris.filter((t) => !t.some((v) => superIdx.includes(v)));
}

// --- 主流程 -----------------------------------------------------------------
const tris = delaunay();
const edgeSet = new Set();
for (const t of tris) {
  for (const [u, v] of [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]]) {
    const k = u < v ? `${u},${v}` : `${v},${u}`;
    edgeSet.add(k);
  }
}

let edges = [...edgeSet].map((k) => k.split(",").map(Number));
const totalEdges = edges.length;
edges = edges.filter(([i, j]) => distKm(i, j) <= MAX_EDGE_KM);
const prunedByLength = totalEdges - edges.length;

// 水道串成链：同一组（`·` 前的地名）的水域省按清单顺序依次相连。
// 长江是一条线，Delaunay 会在拐弯处把中间的省和岸上的陆省连起来、却断开相邻两段水，
// 结果舰队走不到下游。按作者给的顺序补边，比事后猜哪两段该连可靠。
function addWaterChain(es) {
  const has = new Set(es.map(([a, b]) => (a < b ? `${a},${b}` : `${b},${a}`)));
  let added = 0;
  let prev = null;
  let prevGroup = null;
  for (let i = 0; i < PLACES.length; i++) {
    const p = PLACES[i];
    if (p.t !== "water") {
      prev = null;
      prevGroup = null;
      continue;
    }
    const group = p.n.split("·")[0];
    if (prev !== null && group === prevGroup) {
      const k = prev < i ? `${prev},${i}` : `${i},${prev}`;
      if (!has.has(k) && distKm(prev, i) <= 120) {
        es.push([prev, i]);
        has.add(k);
        added++;
      }
    }
    prev = i;
    prevGroup = group;
  }
  return added;
}
const waterChainEdges = addWaterChain(edges);

// 连通性修复：把割裂的分量用最短边接回来
function components(n, es) {
  const parent = [...Array(n).keys()];
  const find = (x) => (parent[x] === x ? x : (parent[x] = find(parent[x])));
  for (const [i, j] of es) {
    const a = find(i), b = find(j);
    if (a !== b) parent[a] = b;
  }
  const map = new Map();
  for (let i = 0; i < n; i++) {
    const r = find(i);
    if (!map.has(r)) map.set(r, []);
    map.get(r).push(i);
  }
  return [...map.values()];
}
let repaired = 0;
for (;;) {
  const comps = components(PLACES.length, edges);
  if (comps.length <= 1) break;
  // 找连接分量 0 与其它分量的最短点对
  const main = comps[0];
  let best = null;
  for (let c = 1; c < comps.length; c++) {
    for (const i of main) {
      for (const j of comps[c]) {
        const d = distKm(i, j);
        if (!best || d < best.d) best = { i, j, d };
      }
    }
  }
  edges.push([best.i, best.j]);
  repaired++;
}

// 邻接表
const nb = Array.from({ length: PLACES.length }, () => []);
for (const [i, j] of edges) {
  if (!nb[i].includes(j)) nb[i].push(j);
  if (!nb[j].includes(i)) nb[j].push(i);
}

// --- 自检 -------------------------------------------------------------------
const problems = [];
const warns = [];
for (let i = 0; i < PLACES.length; i++) {
  if (nb[i].length === 0) problems.push(`${PLACES[i].n} 没有任何邻接`);
  if (nb[i].length > 12) warns.push(`${PLACES[i].n} 有 ${nb[i].length} 个邻接（>12 会告警）`);
}
for (let i = 0; i < PLACES.length; i++) {
  for (let j = i + 1; j < PLACES.length; j++) {
    const d = distKm(i, j);
    if (d < MIN_PAIR_KM) {
      warns.push(`${PLACES[i].n} 与 ${PLACES[j].n} 相距 ${d.toFixed(1)}km，多边形可能退化`);
    }
  }
}
const names = new Set();
for (const p of PLACES) {
  if (names.has(p.n)) problems.push(`重复地名：${p.n}`);
  names.add(p.n);
}
if (!PLACES.some((p) => p.own === "jp" && p.sup)) problems.push("jp 没有补给源");
if (!PLACES.some((p) => p.own === "cn" && p.sup)) problems.push("cn 没有补给源");

// 度数分布
const deg = nb.map((a) => a.length);
const avgDeg = (edges.length * 2) / PLACES.length;

// --- 输出 -------------------------------------------------------------------
const lons = PLACES.map((p) => p.lon);
const lats = PLACES.map((p) => p.lat);
const pad = 0.08;
const map = {
  bbox: [
    +(Math.min(...lons) - pad).toFixed(2),
    +(Math.min(...lats) - pad).toFixed(2),
    +(Math.max(...lons) + pad).toFixed(2),
    +(Math.max(...lats) + pad).toFixed(2),
  ],
  projection: "wgs84",
  provinces: PLACES.map((p, id) => {
    const o = {
      id,
      name: p.n,
      terrain: p.t,
      owner: p.own,
      supply_throughput: p.th ?? 1.0,
      neighbors: nb[id].slice().sort((a, b) => a - b),
      lon: p.lon,
      lat: p.lat,
    };
    if (p.f) o.fort = p.f;
    if (p.sup) o.supply_source = true;
    if (p.p) o.prototype = p.p;
    if (p.note) o.note = p.note;
    // 阵地。不写就由引擎自动生成一个（省名 + 省地形 + 全宽度）。
    if (p.pos) {
      o.positions = p.pos.map(([name, terrain, width]) => ({ name, terrain, width, fort: 0 }));
    }
    return o;
  }),
};

console.log(`省份 ${PLACES.length} 个，边 ${edges.length} 条（剪枝 ${prunedByLength} 条，水道补边 ${waterChainEdges} 条，连通修复 ${repaired} 条）`);
console.log(`平均邻接 ${avgDeg.toFixed(2)}，最大 ${Math.max(...deg)}，最小 ${Math.min(...deg)}`);
console.log(`bbox ${map.bbox.join(", ")}`);
const byTerrain = {};
for (const p of PLACES) byTerrain[p.t] = (byTerrain[p.t] || 0) + 1;
console.log("地形分布 " + Object.entries(byTerrain).map(([k, v]) => `${k}:${v}`).join(" "));
const byOwner = {};
for (const p of PLACES) byOwner[p.own] = (byOwner[p.own] || 0) + 1;
console.log("归属 jp:" + byOwner.jp + " cn:" + byOwner.cn);
if (warns.length) console.log("警告：\n  " + warns.join("\n  "));
if (problems.length) {
  console.error("错误：\n  " + problems.join("\n  "));
  process.exit(1);
}

if (!process.argv.includes("--dry")) {
  const out = path.join(
    __dirname,
    "..",
    "..",
    "content",
    "scenarios",
    "nanxun_1938.imod",
    "map.json"
  );
  fs.writeFileSync(out, JSON.stringify(map, null, 2) + "\n", "utf8");
  console.log("已写入 " + path.relative(path.join(__dirname, "..", ".."), out));
}
