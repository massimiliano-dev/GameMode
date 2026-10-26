// GameMode: controller navigation for Wand's remote web panel, a touch UI.
// GameMode turns the controller into keys (D-pad = arrows, A = Enter); this
// moves a visible focus between the panel's controls, clicks with Enter and
// adjusts sliders and number fields with left/right.
(() => {
  if (window.__gameModeNav) return;
  window.__gameModeNav = true;

  const style = document.createElement("style");
  style.textContent =
    ".gm-focus{outline:3px solid #3ddc97!important;outline-offset:3px!important;box-shadow:0 0 0 6px rgba(61,220,151,.25)!important}";
  const addStyle = () => (document.head ? document.head.appendChild(style) : requestAnimationFrame(addStyle));
  addStyle();

  const SELECTOR = [
    "button",
    "a[href]",
    "input:not([type=hidden])",
    "select",
    "textarea",
    "[role=button]",
    "[role=switch]",
    "[role=checkbox]",
    "[role=slider]",
    "[role=tab]",
    "[role=option]",
    "[tabindex]:not([tabindex='-1'])",
  ].join(",");

  let current = null;

  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 4 && r.height > 4 && !el.disabled && getComputedStyle(el).visibility !== "hidden";
  };
  // Innermost controls only: a clickable card around a button is one stop.
  const candidates = () =>
    [...document.querySelectorAll(SELECTOR)].filter((el) => visible(el) && !el.querySelector(SELECTOR));
  const center = (r) => ({ x: r.left + r.width / 2, y: r.top + r.height / 2 });

  const setCurrent = (el) => {
    current?.classList.remove("gm-focus");
    current = el ?? null;
    if (!current) return;
    current.classList.add("gm-focus");
    current.focus?.({ preventScroll: true });
    current.scrollIntoView({ block: "nearest", inline: "nearest" });
  };

  const move = (dir) => {
    const list = candidates();
    if (!current || !document.contains(current) || !visible(current)) return setCurrent(list[0]);
    const from = center(current.getBoundingClientRect());
    let best = null;
    let bestScore = Infinity;
    for (const el of list) {
      if (el === current) continue;
      const c = center(el.getBoundingClientRect());
      const dx = c.x - from.x;
      const dy = c.y - from.y;
      const ahead = { left: -dx, right: dx, up: -dy, down: dy }[dir];
      if (ahead <= 1) continue;
      const aside = dir === "left" || dir === "right" ? Math.abs(dy) : Math.abs(dx);
      const score = ahead + aside * 2;
      if (score < bestScore) {
        bestScore = score;
        best = el;
      }
    }
    if (best) setCurrent(best);
  };

  const fire = (el) => {
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  };

  /** Left/right on a slider or number field changes its value. */
  const adjust = (el, step) => {
    if (el.matches("input[type=range], input[type=number]")) {
      step > 0 ? el.stepUp() : el.stepDown();
      fire(el);
      return true;
    }
    if (el.getAttribute("role") === "slider") {
      const key = step > 0 ? "ArrowRight" : "ArrowLeft";
      el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
      return true;
    }
    return false;
  };

  const DIRS = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

  window.addEventListener(
    "keydown",
    (e) => {
      // Synthetic keys (sent to sliders above) are the page's business.
      if (!e.isTrusted) return;
      const dir = DIRS[e.key];
      if (dir) {
        e.preventDefault();
        e.stopPropagation();
        const horizontal = dir === "left" || dir === "right";
        if (horizontal && current && adjust(current, dir === "right" ? 1 : -1)) return;
        move(dir);
        return;
      }
      if (e.key === "Enter" || e.key === " ") {
        if (current?.matches("input[type=text], input[type=search], textarea")) return;
        e.preventDefault();
        e.stopPropagation();
        if (current && document.contains(current)) current.click();
        else move("down");
      }
    },
    true,
  );

  // Something focused from the start, once the panel has rendered.
  const start = () => (candidates().length ? setCurrent(candidates()[0]) : setTimeout(start, 300));
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
