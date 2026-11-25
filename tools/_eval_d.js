(() => {
  const b = document.getElementById("bottom");
  return JSON.stringify({
    visible: Array.from(b.children).filter(e => getComputedStyle(e).display !== "none").map(e => e.id || e.className),
    overflow: b.scrollWidth - b.clientWidth, h: Math.round(b.getBoundingClientRect().height),
    modSec: getComputedStyle(document.getElementById("mod-sec")).display
  });
})()
