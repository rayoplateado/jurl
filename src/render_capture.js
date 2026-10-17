// Run in Lightpanda before a page's own scripts (fetch.rs dom_of). It keeps the JSON the page fetches from its own site, each
// response as a <script type="application/json" id="fetch:PATH"> in the head, so the rendered DOM carries the data the page was
// built from, and --with-sources and --recipe read it as `json:fetch:PATH` (extract/sources.rs). Up to MAX_RESPONSES responses
// and MAX_CHARS characters in all; the first response for a path is kept, whatever its query. A response that lands after the
// render is dumped is not kept. It never changes what the page gets.
(() => {
  const MAX_CHARS = 1000000, MAX_RESPONSES = 8;
  let chars = 0, kept = 0;
  const keep = (url, text) => {
    try {
      const u = new URL(url, location.href);
      const id = "fetch:" + u.pathname;
      if (u.origin !== location.origin || kept >= MAX_RESPONSES || chars + text.length > MAX_CHARS || document.getElementById(id)) return;
      JSON.parse(text);
      // "</" and "<!--" can only be inside a JSON string, where "\/" and "\u0021" say the same: the element's text then cannot end it.
      text = text.replace(/<\//g, "<\\/").replace(/<!--/g, "<\\u0021--");
      const el = document.createElement("script");
      el.type = "application/json";
      el.id = id;
      el.textContent = text;
      (document.head || document.documentElement).appendChild(el);
      kept += 1;
      chars += text.length;
    } catch (e) {}
  };
  const json = (type) => (type || "").toLowerCase().includes("json");
  // A response that says it is longer than what is left is not read at all.
  const fits = (length) => !(Number(length) > MAX_CHARS - chars);
  const fetch0 = window.fetch;
  if (fetch0) {
    window.fetch = function (...args) {
      return fetch0.apply(this, args).then((r) => {
        try {
          const asked = typeof args[0] === "string" ? args[0] : args[0] && args[0].url;
          if (json(r.headers.get("content-type")) && fits(r.headers.get("content-length"))) r.clone().text().then((t) => keep(r.url || asked, t), () => {});
        } catch (e) {}
        return r;
      });
    };
  }
  const XHR = window.XMLHttpRequest;
  if (XHR) {
    const open0 = XHR.prototype.open;
    XHR.prototype.open = function (method, url, ...rest) {
      this.addEventListener("load", () => {
        try {
          if (!json(this.getResponseHeader("content-type"))) return;
          if (this.responseType === "" || this.responseType === "text") keep(this.responseURL || url, this.responseText);
          else if (this.responseType === "json" && this.response !== null) keep(this.responseURL || url, JSON.stringify(this.response));
        } catch (e) {}
      });
      return open0.call(this, method, url, ...rest);
    };
  }
})();
