#!/usr/bin/env node
/**
 * shot.js —— 无头截图 + 在页面里跑一小段 JS 的验证工具。
 *
 * 为什么要有它：这是一个"看的多、读的少"的项目。地图/面板/邮箱到底长什么样，
 * 只能看。手工开浏览器截图既慢又容易看到**缓存的旧页面**（已经栽过两次：
 * 检查了旧 DOM，得出"没生效"的错误结论）。这里每次都是新进程、新 profile、
 * 强制 no-cache，截完即杀。
 *
 * 用法：
 *   node tools/shot.js --url http://127.0.0.1:8899/ --out tools/_shots/terrain.png \
 *        --click "[data-view=terrain]" --wait 800
 *   node tools/shot.js --url ... --eval "JSON.stringify(S.provinces.length)" --out x.png
 *
 * 输出：PNG 文件 + stdout 上的 `eval:` 结果（若有 --eval）。
 */

"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const argv = process.argv.slice(2);
function arg(name, dflt) {
  const i = argv.indexOf("--" + name);
  return i >= 0 ? argv[i + 1] : dflt;
}
const url = arg("url", "http://127.0.0.1:8899/");
const out = arg("out", path.join(__dirname, "_shots", "shot.png"));
const clickSel = arg("click", null);
// --eval-file 存在的唯一理由：PowerShell 把多行/带引号的 --eval 参数吃掉一半，
// 报出来的错（Unexpected end of input、Invalid regular expression flags）
// 看着像页面有问题，其实是命令行转义。要跑一段真代码就写进文件。
const evalFile = arg("eval-file", null);
const evalJs = evalFile ? fs.readFileSync(evalFile, "utf8").trim() : arg("eval", null);
const waitMs = Number(arg("wait", 900));
const size = arg("size", "1600,1000").split(",").map(Number);
// `--metrics 904x407` 或 `--metrics 904x407@3`：用 CDP 覆盖**页面视口**。
//
// 为什么需要它：`--size` 只改浏览器窗口，页面 CSS 视口不一定跟着变
// （实测传 900x420，页面里 innerWidth 只有 500）。而手机横屏的布局
// 是靠 @media (max-width:820px) / (max-height:560px) 判的，视口不对就验了个假的。
// 真机 Redmi 24117RK2CC：物理 1220x2712、DPR≈3 → 横屏 CSS 视口 904x407。
const metrics = arg("metrics", null);
const port = Number(arg("port", 9333));

const CANDIDATES = [
  // macOS（同一套 CDP，Chromium 内核的 Edge / Chrome 都行）
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
];
const exe = CANDIDATES.find((p) => fs.existsSync(p));
if (!exe) {
  console.error("no browser found");
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), "shot-"));
const proc = spawn(
  exe,
  [
    "--headless=new",
    "--remote-debugging-port=" + port,
    "--user-data-dir=" + profile,
    "--window-size=" + size.join(","),
    "--hide-scrollbars",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-features=Translate",
    "about:blank",
  ],
  { stdio: "ignore" }
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function targets() {
  const r = await fetch(`http://127.0.0.1:${port}/json/list`);
  return r.json();
}

async function main() {
  let page = null;
  for (let i = 0; i < 60 && !page; i++) {
    await sleep(200);
    try {
      const list = await targets();
      page = list.find((t) => t.type === "page");
    } catch (e) {
      /* 端口还没起来 */
    }
  }
  if (!page) throw new Error("browser did not expose a page target");

  const ws = new WebSocket(page.webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  const send = (method, params) =>
    new Promise((res, rej) => {
      const n = ++id;
      pending.set(n, { res, rej });
      ws.send(JSON.stringify({ id: n, method, params: params || {} }));
    });
  const events = [];
  ws.addEventListener("message", (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      msg.error ? p.rej(new Error(msg.error.message)) : p.res(msg.result);
    } else if (msg.method) {
      events.push(msg);
    }
  });
  await new Promise((r) => ws.addEventListener("open", r));

  await send("Page.enable");
  await send("Network.enable");
  await send("Network.setCacheDisabled", { cacheDisabled: true });
  if (metrics) {
    const [wh, dpr] = metrics.split("@");
    const [mw, mh] = wh.split("x").map(Number);
    await send("Emulation.setDeviceMetricsOverride", {
      width: mw,
      height: mh,
      deviceScaleFactor: Number(dpr || 1),
      mobile: true,
    });
    await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  }
  await send("Page.navigate", { url });
  // 等 load；不等也会被下面的 sleep 兜住
  for (let i = 0; i < 100; i++) {
    if (events.some((e) => e.method === "Page.loadEventFired")) break;
    await sleep(100);
  }
  await sleep(waitMs);

  const evaluate = async (expr) => {
    const r = await send("Runtime.evaluate", {
      expression: expr,
      returnByValue: true,
      awaitPromise: true,
    });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + (r.result && r.result.description));
    return r.result.value;
  };

  if (clickSel) {
    const ok = await evaluate(
      `(() => { const el = document.querySelector(${JSON.stringify(clickSel)});
        if (!el) return "not found: " + ${JSON.stringify(clickSel)};
        el.click(); return "clicked"; })()`
    );
    console.log("click:", ok);
    await sleep(400);
  }

  if (evalJs) {
    console.log("eval:", await evaluate(evalJs));
  }

  const shot = await send("Page.captureScreenshot", { format: "png" });
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, Buffer.from(shot.data, "base64"));
  console.log("saved:", out, fs.statSync(out).size, "bytes");

  // 页面里的报错也带回来——否则"白屏"只能靠猜
  const errs = events
    .filter((e) => e.method === "Runtime.exceptionThrown")
    .map((e) => e.params.exceptionDetails.text);
  if (errs.length) console.log("page errors:", errs.slice(0, 5).join(" | "));

  ws.close();
  proc.kill();
}

main()
  .catch((e) => {
    console.error("FAILED:", e.message);
    process.exitCode = 1;
  })
  .finally(async () => {
    await sleep(200);
    try {
      proc.kill();
    } catch (e) {}
  });
