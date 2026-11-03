// Schedina service worker (plan 1.F; the cut rule allows dropping it). Minimal on purpose:
// it makes the PWA installable and keeps the last app shell for a cold start without network.
// Only same-origin page navigations are handled (network first, cached shell as fallback).
// Desk API calls, the /stream socket, Solana RPC and wallet traffic are never intercepted, and
// the hashed wasm/JS/CSS bundles are left to the browser's HTTP cache.
const CACHE = "schedina-shell-v2";
const SHELL = ["/", "/manifest.webmanifest", "/icon.svg", "/icon-192.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET" || req.mode !== "navigate") return;
  if (new URL(req.url).origin !== self.location.origin) return;
  event.respondWith(
    fetch(req)
      .then((res) => {
        if (res.ok) {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put("/", copy));
        }
        return res;
      })
      .catch(() => caches.match("/").then((hit) => hit ?? Response.error())),
  );
});
