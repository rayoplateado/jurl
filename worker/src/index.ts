// jurl.dev: static landing (../site) plus POST /api/try, the playground.
//
// A try goes: validate → Turnstile → per-minute limit → cache → daily budget
// and per-IP limits (Budget) → the real jurl binary in a container (Jurl) →
// settle the actual cost → cache → respond.

import { Container, getContainer } from "@cloudflare/containers";
import { DurableObject } from "cloudflare:workers";

export interface Env {
  ASSETS: Fetcher;
  JURL: DurableObjectNamespace<Jurl>;
  BUDGET: DurableObjectNamespace<Budget>;
  CACHE: KVNamespace;
  PER_MINUTE: RateLimit;
  DAILY_BUDGET_USD: string;
  PER_IP_DAILY: string;
  PER_IP_DAILY_FIND: string;
  TYPESAFE_API_KEY: string;
  CLOUDFLARE_AI_ACCOUNT_ID: string;
  CLOUDFLARE_AI_TOKEN: string;
  TURNSTILE_SECRET: string;
}

const MODES = ["gist", "ask", "code", "links", "images", "find"] as const;
type Mode = (typeof MODES)[number];
const NEEDS_Q: Mode[] = ["ask", "find"];
const MAX_Q = 200;
const CACHE_TTL = 3600;

// Charged up front so parallel tries can't overshoot the cap, then settled
// to the real cost. find looks at up to 80 images; a hedged Clef call is paid twice.
const ESTIMATE: Record<Mode, number> = { gist: 0.0015, ask: 0.0015, code: 0.0015, links: 0.0015, images: 0.0002, find: 0.004 };
const JEV_PER_TOKEN = 0.042 / 1e6;
const CLEF_PER_IMAGE = (255 * 0.09) / 1e6;

// ---------- the container ----------

export class Jurl extends Container<Env> {
  defaultPort = 8080;
  sleepAfter = "5m";

  constructor(ctx: DurableObjectState<{}>, env: Env) {
    super(ctx, env);
    this.envVars = {
      TYPESAFE_API_KEY: env.TYPESAFE_API_KEY,
      CLOUDFLARE_ACCOUNT_ID: env.CLOUDFLARE_AI_ACCOUNT_ID,
      CLOUDFLARE_AI_TOKEN: env.CLOUDFLARE_AI_TOKEN,
    };
  }
}

// ---------- the daily budget ----------

type Verdict = { ok: true } | { ok: false; reason: "budget" | "ip" };

// One instance for the whole site. Durable Objects run one request at a time,
// so reserve() is atomic.
export class Budget extends DurableObject<Env> {
  private async today(): Promise<string> {
    const day = new Date().toISOString().slice(0, 10);
    if ((await this.ctx.storage.get<string>("day")) !== day) {
      await this.ctx.storage.deleteAll();
      await this.ctx.storage.put("day", day);
    }
    return day;
  }

  async reserve(ip: string, mode: Mode): Promise<Verdict> {
    await this.today();
    const spent = (await this.ctx.storage.get<number>("spent")) ?? 0;
    if (spent + ESTIMATE[mode] > Number(this.env.DAILY_BUDGET_USD)) return { ok: false, reason: "budget" };
    const all = (await this.ctx.storage.get<number>(`ip:${ip}`)) ?? 0;
    const finds = (await this.ctx.storage.get<number>(`find:${ip}`)) ?? 0;
    if (all >= Number(this.env.PER_IP_DAILY)) return { ok: false, reason: "ip" };
    if (mode === "find" && finds >= Number(this.env.PER_IP_DAILY_FIND)) return { ok: false, reason: "ip" };
    await this.ctx.storage.put({
      spent: spent + ESTIMATE[mode],
      [`ip:${ip}`]: all + 1,
      ...(mode === "find" ? { [`find:${ip}`]: finds + 1 } : {}),
    });
    return { ok: true };
  }

  // Replace the estimate with what the call really cost.
  async settle(mode: Mode, actual: number): Promise<void> {
    await this.today();
    const spent = (await this.ctx.storage.get<number>("spent")) ?? 0;
    await this.ctx.storage.put("spent", Math.max(0, spent - ESTIMATE[mode] + actual));
  }
}

// ---------- the API ----------

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/api/try" && request.method === "POST") return tryIt(request, env);
    if (url.pathname.startsWith("/api/")) return json({ error: "not found" }, 404);
    return env.ASSETS.fetch(request);
  },
} satisfies ExportedHandler<Env>;

async function tryIt(request: Request, env: Env): Promise<Response> {
  const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
  let body: { mode?: string; url?: string; q?: string; token?: string };
  try {
    body = await request.json();
  } catch {
    return json({ error: "bad request" }, 400);
  }

  const mode = body.mode as Mode;
  if (!MODES.includes(mode)) return json({ error: "Pick what you want jurl to find." }, 400);
  const q = (body.q ?? "").trim();
  if (q.length > MAX_Q) return json({ error: `Keep the question under ${MAX_Q} characters.` }, 400);
  if (NEEDS_Q.includes(mode) && !q) return json({ error: mode === "find" ? "Describe the photo you want." : "Ask a question." }, 400);
  const target = await checkUrl(body.url ?? "");
  if (typeof target === "string") return json({ error: target }, 400);

  if (!(await turnstileOk(body.token ?? "", ip, env))) return json({ error: "Couldn't verify you're human. Reload and try again." }, 403);
  if (!(await env.PER_MINUTE.limit({ key: ip })).success) return json({ error: "Easy there. Try again in a minute." }, 429);

  const key = await cacheKey(mode, target.href, q);
  const hit = await env.CACHE.get(key, "json");
  if (hit) return json({ ...(hit as object), cached: true });

  const budget = env.BUDGET.get(env.BUDGET.idFromName("global"));
  const verdict = await budget.reserve(ip, mode);
  if (!verdict.ok) {
    return verdict.reason === "budget"
      ? json({ error: "The playground has used up today's budget. Back tomorrow — or install jurl, it takes two lines." }, 503)
      : json({ error: "That's today's share of free tries. Install jurl to keep going: it takes two lines." }, 429);
  }

  let run: { code: number; stdout: string; stderr: string };
  try {
    const res = await getContainer(env.JURL, "jurl").fetch(
      new Request("http://jurl/run", { method: "POST", body: JSON.stringify({ mode, url: target.href, q }) }),
    );
    if (!res.ok) throw new Error(`container ${res.status}`);
    run = await res.json();
  } catch (e) {
    await budget.settle(mode, 0);
    console.error("container", String(e));
    return json({ error: "jurl is waking up. Give it a few seconds and try again." }, 502);
  }

  const out = shape(mode, run);
  await budget.settle(mode, out.cost ?? ESTIMATE[mode]);
  console.log(JSON.stringify({ mode, host: target.hostname, code: run.code, cost: out.cost, ms: out.ms }));

  const payload = { mode, url: target.href, q, result: out.result, message: out.message, timing: out.timing };
  if (out.cacheable) await env.CACHE.put(key, JSON.stringify(payload), { expirationTtl: CACHE_TTL });
  return json({ ...payload, cached: false });
}

// jurl prints JSON on stdout; on stderr a timing line (⏱ …) and `jurl: …`
// messages, some of which are results ("no image … looks like …").
function shape(mode: Mode, run: { code: number; stdout: string; stderr: string }) {
  const lines = run.stderr.split("\n").map((l) => l.trim()).filter(Boolean);
  const timing = lines.find((l) => l.startsWith("⏱"))?.replace(/^⏱\s*/, "") ?? null;
  const notes = lines.filter((l) => l.startsWith("jurl:") && !/clef skipped|rendering with Lightpanda/.test(l));
  let result: unknown = null;
  try {
    result = run.stdout ? JSON.parse(run.stdout) : null;
  } catch {
    result = null;
  }
  const noMatch = mode === "find" && notes.some((l) => l.includes("looks like"));
  const message = result ? null : (notes.at(-1)?.replace(/^jurl:\s*/, "") ?? (run.code === -1 ? "timed out" : "jurl couldn't read that page."));

  let cost: number | null = null;
  let ms: number | null = null;
  if (timing) {
    const tok = Number(/(\d+)\s*tok/.exec(timing)?.[1] ?? 0);
    const img = Number(/(\d+)\s*img/.exec(timing)?.[1] ?? 0);
    cost = tok * JEV_PER_TOKEN + img * CLEF_PER_IMAGE * 1.3;
    ms = Number(/total\s*(\d+)ms/.exec(timing)?.[1] ?? 0) || null;
  }
  return { result, message, timing, cost, ms, cacheable: Boolean(result) || noMatch };
}

// ---------- guards ----------

// Public http(s) pages only: no IP literals, no local names, nothing that
// resolves to a private, loopback or link-local address.
async function checkUrl(raw: string): Promise<URL | string> {
  let u: URL;
  try {
    u = new URL(/^https?:\/\//i.test(raw.trim()) ? raw.trim() : `https://${raw.trim()}`);
  } catch {
    return "That doesn't look like a URL.";
  }
  if (!["http:", "https:"].includes(u.protocol) || u.username || u.password) return "Only public http(s) pages.";
  if (u.port && !["80", "443"].includes(u.port)) return "Only public http(s) pages.";
  const host = u.hostname.toLowerCase();
  if (/^[\d.]+$/.test(host) || host.includes(":") || host.startsWith("[")) return "Use a domain name, not an IP address.";
  if (!host.includes(".") || /(^|\.)(localhost|local|internal|lan|home|arpa)$/.test(host)) return "Only public http(s) pages.";
  const addrs = [...(await resolve(host, "A")), ...(await resolve(host, "AAAA"))];
  if (addrs.length === 0) return `Couldn't find ${host}.`;
  if (addrs.some(isPrivate)) return "Only public http(s) pages.";
  u.hash = "";
  return u;
}

async function resolve(host: string, type: "A" | "AAAA"): Promise<string[]> {
  const res = await fetch(`https://cloudflare-dns.com/dns-query?name=${encodeURIComponent(host)}&type=${type}`, {
    headers: { accept: "application/dns-json" },
  });
  if (!res.ok) return [];
  const data: { Answer?: { type: number; data: string }[] } = await res.json();
  const want = type === "A" ? 1 : 28;
  return (data.Answer ?? []).filter((a) => a.type === want).map((a) => a.data);
}

function isPrivate(ip: string): boolean {
  if (ip.includes(":")) {
    const v6 = ip.toLowerCase();
    return v6 === "::1" || v6 === "::" || /^f[cd]/.test(v6) || /^fe[89ab]/.test(v6) || v6.startsWith("::ffff:");
  }
  const [a, b] = ip.split(".").map(Number);
  return (
    a === 0 || a === 10 || a === 127 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127) || a >= 224
  );
}

async function turnstileOk(token: string, ip: string, env: Env): Promise<boolean> {
  if (!token || !env.TURNSTILE_SECRET) return false;
  const form = new FormData();
  form.append("secret", env.TURNSTILE_SECRET);
  form.append("response", token);
  form.append("remoteip", ip);
  const res = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", { method: "POST", body: form });
  const data: { success?: boolean } = await res.json();
  return data.success === true;
}

async function cacheKey(mode: Mode, url: string, q: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(JSON.stringify([mode, url, q.toLowerCase()])));
  return "try:" + [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function json(obj: unknown, status = 200): Response {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}
