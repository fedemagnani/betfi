// 8.D (L8-3) headless check of "Continue with Google" via Privy, on a live (non-demo) build:
// the app boots without loading the Privy bundle, "Log in" renders in the app bar and on Account's
// wallet card and opens the log-in popup (1 Nov: Google first, then Phantom), and a tap on
// "Continue with Google" imports assets/privy.js, calls SchedinaPrivy.init with the app
// ID and produces Google's authorisation URL from Privy. The Google page itself is never loaded
// (the navigation is answered with a stub), so no login is completed. Requests off the app's
// origin are aborted except Privy's (auth.privy.io and its embedded-wallet iframe).
//
//   PLAYWRIGHT_CORE=… node scripts/privy_check.mjs <live-build-url>
//
// PRIVY_STUB_OAUTH=1 answers Privy's /api/v1/oauth/init with a stub Google URL instead of asking
// Privy (for when the dashboard does not allow Google yet: Privy answers 403 "Login with Google
// not allowed"); everything before and after that call is still real.
//
// Exits 1 on any failed step; prints one JSON line per step and a summary.
import { homedir } from "node:os";

const { chromium } = await import(process.env.PLAYWRIGHT_CORE ?? "playwright-core");
const base = process.argv[2];
if (!base) {
  console.error("usage: node scripts/privy_check.mjs <live-build-url>");
  process.exit(2);
}
const CHROME = process.env.CHROME_PATH
  ?? `${homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1208/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const origin = new URL(base).origin;
const privyHost = (h) => h === "privy.io" || h.endsWith(".privy.io");
const googleHost = (h) => h === "accounts.google.com";

const browser = await chromium.launch({ executablePath: CHROME });
const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, colorScheme: "dark", serviceWorkers: "block" });
const seen = { privyJs: [], privyApi: [], google: null };
await ctx.route("**/*", async (route) => {
  const u = new URL(route.request().url());
  if (u.origin === origin) {
    if (/\/assets\/privy[^/]*\.js$/.test(u.pathname)) seen.privyJs.push(u.pathname);
    return route.continue();
  }
  if (privyHost(u.hostname)) {
    if (process.env.PRIVY_STUB_OAUTH === "1" && u.pathname === "/api/v1/oauth/init") {
      const redirect = JSON.parse(route.request().postData() ?? "{}").redirect_to;
      const url = `https://accounts.google.com/o/oauth2/v2/auth?client_id=stub&redirect_uri=${encodeURIComponent("https://auth.privy.io/api/v1/oauth/callback")}&code_challenge_method=S256&state=stub&x_redirect_to=${encodeURIComponent(redirect ?? "")}`;
      return route.fulfill({ status: 200, contentType: "application/json", headers: { "access-control-allow-origin": origin, "access-control-allow-credentials": "true" }, body: JSON.stringify({ url }) });
    }
    return route.continue();
  }
  if (googleHost(u.hostname)) {
    seen.google = u.toString();
    return route.fulfill({ status: 200, contentType: "text/html", body: "<title>Google (stub)</title>stub" });
  }
  return route.abort();
});
// Record every SchedinaPrivy call (the bridge assigns window.SchedinaPrivy once).
const calls = [];
await ctx.exposeFunction("__privyLog", (fn, args) => { calls.push({ fn, args }); });
await ctx.addInitScript(() => {
  let bridge;
  Object.defineProperty(window, "SchedinaPrivy", {
    configurable: true,
    get: () => bridge,
    set: (b) => {
      bridge = Object.fromEntries(Object.entries(b).map(([k, f]) => [k, (...a) => {
        window.__privyLog(k, a.map((x) => (typeof x === "string" ? x : typeof x)));
        return f(...a);
      }]));
    },
  });
});
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("response", (r) => {
  const u = new URL(r.url());
  if (privyHost(u.hostname) && u.pathname.startsWith("/api/")) seen.privyApi.push(`${r.status()} ${u.pathname}`);
});

const results = [];
const step = async (name, fn) => {
  try {
    const detail = await fn();
    results.push({ name, ok: true, detail });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.message ?? e).split("\n")[0] });
  }
  console.log(JSON.stringify(results.at(-1)));
};

await step("boot, Log in in the app bar; its popup lists Google first, then Phantom", async () => {
  await page.goto(base, { waitUntil: "load" });
  const btn = page.locator("#login-bar");
  await btn.waitFor({ state: "visible", timeout: 30000 });
  await btn.click();
  await page.waitForSelector("#login-dialog");
  const ids = await page.locator("#login-dialog .login-option").evaluateAll((os) => os.map((o) => o.id));
  const focused = await page.evaluate(() => document.activeElement?.id);
  if (ids[0] !== "login-google" || ids[1] !== "login-phantom") throw new Error(`options ${JSON.stringify(ids)}`);
  if (focused !== "login-google") throw new Error(`focus on ${focused}`);
  await page.keyboard.press("Escape");
  await page.waitForSelector("#login-dialog", { state: "detached" });
  return { options: ids, focused };
});
await step("the Privy bundle is not loaded before a tap", async () => {
  await page.waitForTimeout(1000);
  if (seen.privyJs.length) throw new Error(`loaded early: ${seen.privyJs}`);
  return "not loaded";
});
await step("Account's wallet card has Log in", async () => {
  await page.locator("#bottom-nav a[href='/account']").first().click();
  const btn = page.locator("#login-account");
  await btn.waitFor({ state: "visible", timeout: 15000 });
  return { text: (await btn.innerText()).trim() };
});
await step("a tap calls SchedinaPrivy.init and yields a Google auth URL", async () => {
  await page.locator("#login-account").click();
  await page.locator("#login-google").click();
  const deadline = Date.now() + 45000;
  while (!seen.google && Date.now() < deadline) {
    const alert = await page.locator("#login-error, #account-wallet [role=alert]").first().textContent({ timeout: 200 }).catch(() => null);
    if (alert) {
      const hint = seen.privyApi.includes("403 /api/v1/oauth/init")
        ? "; Privy refused: enable Google login and allow this origin in the Privy dashboard (or PRIVY_STUB_OAUTH=1)"
        : "";
      throw new Error(`${alert} (Privy API: ${seen.privyApi.join(", ") || "none"})${hint}`);
    }
    await page.waitForTimeout(250);
  }
  if (!seen.google) throw new Error(`no Google URL (Privy API: ${seen.privyApi.join(", ") || "none"})`);
  const g = new URL(seen.google);
  return {
    bundle: seen.privyJs[0],
    google: `${g.origin}${g.pathname}`,
    client_id: g.searchParams.get("client_id"),
    redirect_uri: g.searchParams.get("redirect_uri"),
    pkce: g.searchParams.get("code_challenge_method"),
    stubbed: process.env.PRIVY_STUB_OAUTH === "1",
    redirect_to: g.searchParams.get("x_redirect_to") ?? undefined,
    privyApi: seen.privyApi,
  };
});
await step("init got the configured app ID", async () => {
  const init = calls.find((c) => c.fn === "init");
  if (!init) throw new Error(`no init call in ${JSON.stringify(calls)}`);
  const want = process.env.SCHEDINA_PRIVY_APP_ID || "cmvwcynao00xn0hlbq5t6gtnz";
  if (init.args[0] !== want) throw new Error(`init got ${init.args[0]}, want ${want}`);
  if (!calls.some((c) => c.fn === "startGoogleLogin")) throw new Error("startGoogleLogin not called");
  return { appId: init.args[0], order: calls.map((c) => c.fn) };
});
await step("back from Google: the bundle loads at boot, the code is tried, the params stripped", async () => {
  calls.length = 0;
  seen.privyJs.length = 0;
  const p = await ctx.newPage();
  await p.goto(`${base}/account?privy_oauth_code=bogus&privy_oauth_state=bogus&privy_oauth_provider=google`, { waitUntil: "load" });
  const deadline = Date.now() + 30000;
  while (!calls.some((c) => c.fn === "completeLoginIfRedirected") && Date.now() < deadline) await p.waitForTimeout(250);
  if (!seen.privyJs.length) throw new Error(`bundle not loaded at boot (calls: ${JSON.stringify(calls)}, url: ${p.url()})`);
  const alert = p.locator("#account-wallet [role=alert]").first();
  await alert.waitFor({ timeout: 30000 });
  const search = await p.evaluate(() => location.search);
  if (search.includes("privy_oauth")) throw new Error(`params kept: ${search}`);
  const text = (await alert.textContent()).trim();
  await p.close();
  return { order: calls.map((c) => c.fn), search, shown: text };
});
await step("a stored Privy session loads the bundle at boot (and no session connects nothing)", async () => {
  calls.length = 0;
  seen.privyJs.length = 0;
  const p = await ctx.newPage();
  await p.goto(`${base}/account`, { waitUntil: "load" });
  await p.evaluate(() => localStorage.setItem("privy:refresh_token", JSON.stringify("bogus")));
  await p.reload({ waitUntil: "load" });
  const deadline = Date.now() + 30000;
  while (!calls.some((c) => c.fn === "solanaAddress") && Date.now() < deadline) await p.waitForTimeout(250);
  if (!calls.some((c) => c.fn === "solanaAddress")) throw new Error(`no solanaAddress call: ${JSON.stringify(calls)}`);
  await p.waitForTimeout(1500);
  const stillButton = await p.locator("#login-account").isVisible();
  await p.evaluate(() => localStorage.removeItem("privy:refresh_token"));
  await p.close();
  return { order: calls.map((c) => c.fn), buttonStillShown: stillButton };
});
if (errors.length) results.push({ name: "page errors", ok: false, error: errors.join(" | ") });
await browser.close();
const failed = results.filter((r) => !r.ok);
console.log(`privy check: ${results.length - failed.length}/${results.length} ok${failed.length ? `; failed: ${failed.map((f) => f.name).join("; ")}` : ""}`);
process.exit(failed.length ? 1 : 0);
