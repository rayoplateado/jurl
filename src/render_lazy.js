// Run in Lightpanda before a page's own scripts (fetch.rs dom_of). A page that adds its cards or its images as they scroll into
// view (an IntersectionObserver on them, or a scroll listener) shows only the first of them to a browser that never scrolls, and
// Lightpanda has no screen to scroll. So every TICK_MS, from START_MS on, each element the page observes is told it is in view,
// and the window scrolls further and says so. Once the page's element count holds for STILL_TICKS ticks in a row, or after
// MAX_TICKS, it sets window.__jurlLazy.done, which the render waits for before its dump. It never changes what the page fetches
// by itself: the page loads only what it would load for a person scrolling to its end.
(() => {
  const START_MS = 500, TICK_MS = 250, STILL_TICKS = 3, MAX_TICKS = 20;
  const state = { done: false };
  window.__jurlLazy = state;
  const watchers = new Set();
  const Native = window.IntersectionObserver;
  if (typeof Native === "function") {
    const Watched = function (callback, options) {
      const observer = new Native(callback, options);
      const watcher = { callback, observer, targets: new Set() };
      watchers.add(watcher);
      const observe = observer.observe.bind(observer), unobserve = observer.unobserve.bind(observer), disconnect = observer.disconnect.bind(observer);
      observer.observe = (target) => (watcher.targets.add(target), observe(target));
      observer.unobserve = (target) => (watcher.targets.delete(target), unobserve(target));
      observer.disconnect = () => (watcher.targets.clear(), disconnect());
      return observer;
    };
    Watched.prototype = Native.prototype;
    window.IntersectionObserver = Watched;
  }
  const inView = (target) => ({ target, isIntersecting: true, intersectionRatio: 1, time: performance.now(), boundingClientRect: {}, intersectionRect: {}, rootBounds: null });
  let ticks = 0, still = 0, last = -1;
  const tick = () => {
    ticks += 1;
    for (const watcher of watchers) {
      if (watcher.targets.size > 0) try { watcher.callback([...watcher.targets].map(inView), watcher.observer); } catch (e) {}
    }
    try { window.scrollTo(0, ticks * 10000); } catch (e) {}
    try { window.dispatchEvent(new Event("scroll")); document.dispatchEvent(new Event("scroll")); } catch (e) {}
    const size = document.getElementsByTagName("*").length;
    still = size === last ? still + 1 : 0;
    last = size;
    if (still >= STILL_TICKS || ticks >= MAX_TICKS) state.done = true;
    else setTimeout(tick, TICK_MS);
  };
  setTimeout(tick, START_MS);
})();
