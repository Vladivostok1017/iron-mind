(async () => {
  const nap = (ms) => new Promise(r => setTimeout(r, ms));
  const btn = (t) => Array.from(document.querySelectorAll("button")).find(e => (e.textContent||"").includes(t));
  const c = document.getElementById("hm-continue");
  if (c && !c.disabled) c.click(); else { document.getElementById("hm-new").click(); await nap(800); (btn("作为第九战区")||btn("开始"))?.click(); }
  await nap(2400);
  (btn("跳过引导")||btn("跳过"))?.click(); await nap(300);
  const ok = btn("已阅") || btn("知道了"); if (ok) ok.click(); await nap(300);
  const b = document.getElementById("bottom");
  const vis = Array.from(b.children).filter(e => getComputedStyle(e).display !== "none").map(e => e.id || e.className);
  const r = b.getBoundingClientRect();
  const rows = [...new Set(Array.from(b.querySelectorAll("#modes, .btn")).filter(e=>e.offsetParent).map(e => Math.round(e.getBoundingClientRect().top)))];
  return JSON.stringify({
    visible: vis, bottomH: Math.round(r.height), overflowX: b.scrollWidth - b.clientWidth,
    rows: rows.length, modSec: getComputedStyle(document.getElementById("mod-sec")).display
  });
})()
