// Run in Lightpanda before a page's own scripts (fetch.rs dom_of). A page that adds its cards or its images as they scroll into
// view (an IntersectionObserver on them, or a scroll listener) shows only the first of them to a browser that never scrolls, and
// Lightpanda has no screen to scroll. So every TICK_MS, from START_MS on, each element the page observes is told it is in view,
// and the window scrolls further and says so, as a person scrolling to the end would make it. Once the page's element count holds
// for STILL_TICKS ticks with none of its requests in flight, or after MAX_TICKS, it sets window.__jurlLazy.done, which the render
// waits for before its dump. A page that watches no element and listens for no scroll is done at the first tick.
(() => {
  const START_MS = 300, TICK_MS = 250, STILL_TICKS = 3, MAX_TICKS = 12;
  const state = { done: false };
  window.__jurlLazy = state;
  const watchers = new Set();
  let scrollListeners = 0, pending = 0;

  const Native = window.IntersectionObserver;
  if (typeof Native === "function") {
    const Watched = class extends Native {
      constructor(callback, options) {
        super(callback, options);
        this.__jurl = { callback, targets: new Set() };
        watchers.add(this);
      }
      observe(target) { this.__jurl.targets.add(target); return super.observe(target); }
      unobserve(target) { this.__jurl.targets.delete(target); return super.unobserve(target); }
      disconnect() { this.__jurl.targets.clear(); return super.disconnect(); }
    };
    Object.defineProperty(Watched, "name", { value: "IntersectionObserver" });
    window.IntersectionObserver = Watched;
  }
  for (const target of [window, document]) {
    const add = target.addEventListener;
    target.addEventListener = function (type, ...rest) {
      if (type === "scroll" || type === "wheel") scrollListeners += 1;
      return add.call(this, type, ...rest);
    };
  }
  // The page's own requests: a tick is still only when none is in flight, so cards that come back slowly are waited for.
  const fetch0 = window.fetch;
  if (fetch0) {
    window.fetch = function (...args) {
      pending += 1;
      return fetch0.apply(this, args).finally(() => { pending -= 1; });
    };
  }
  const XHR = window.XMLHttpRequest;
  if (XHR) {
    const send = XHR.prototype.send;
    XHR.prototype.send = function (...args) {
      pending += 1;
      this.addEventListener("loadend", () => { pending -= 1; });
      return send.apply(this, args);
    };
  }

  const rect = (target) => { try { return target.getBoundingClientRect(); } catch (e) { return { top: 0, left: 0, width: 0, height: 0 }; } };
  const inView = (target) => {
    const box = rect(target);
    return { target, isIntersecting: true, isVisible: true, intersectionRatio: 1, time: performance.now(), boundingClientRect: box, intersectionRect: box, rootBounds: null };
  };
  const watched = () => [...watchers].some((watcher) => watcher.__jurl.targets.size > 0);
  let ticks = 0, still = 0, last = -1;
  const tick = () => {
    ticks += 1;
    if (ticks === 1 && !watched() && scrollListeners === 0 && window.onscroll == null && document.onscroll == null) {
      state.done = true;
      return;
    }
    for (const watcher of watchers) {
      const { callback, targets } = watcher.__jurl;
      if (targets.size > 0) try { callback([...targets].map(inView), watcher); } catch (e) {}
    }
    try { window.scrollTo(0, ticks * 10000); } catch (e) {}
    try { window.dispatchEvent(new Event("scroll")); document.dispatchEvent(new Event("scroll")); } catch (e) {}
    const size = document.getElementsByTagName("*").length;
    still = size === last && pending === 0 ? still + 1 : 0;
    last = size;
    if (still >= STILL_TICKS || ticks >= MAX_TICKS) state.done = true;
    else setTimeout(tick, TICK_MS);
  };
  setTimeout(tick, START_MS);
})();
