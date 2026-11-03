// Copied unchanged (below this line) from an earlier project's wallet.js: the plan 1.F wallet fallback, loaded via document::eval if dioxus-solana does not sign within the hour.
// wallet.js: the wallet island (spec 4.3). Connects Phantom (window.phantom.solana or
// window.solana; Backpack and Solflare expose the same interface), signs in with a one-time
// signMessage challenge, and for every chain action (faucet, buy, sell, commit, claim) asks the
// server for an unsigned transaction, has the wallet sign and send it, then posts the signature
// back and reloads once the chain mirror has it. The server never sees a key, and the page
// never learns the server's RPC endpoint: the wallet sends through its own.
// Also: quantity selectors ([data-trade] > select[data-qty]: the option's server-computed
// label goes on the button), countdowns ([data-countdown] = unix ms) and auto-reload
// ([data-autoreload] = seconds), postponed while a sign-in or transaction is in flight.
// Only DOM text is written (textContent), never HTML.

const provider = () => window.phantom?.solana ?? window.solana ?? null;
const statusEl = () => document.getElementById("tx-status") ?? document.getElementById("wallet-status");
const say = (msg) => { const el = statusEl(); if (el) el.textContent = msg; };
const b64 = (bytes) => btoa(String.fromCharCode(...bytes));
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const usdc = (u) => `$${(Number(u) / 1e6).toFixed(2)}`;

async function api(path, body) {
  const res = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(body ?? {}),
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(json.error ?? `HTTP ${res.status}`);
  return json;
}

async function connect() {
  const wallet = provider();
  if (!wallet) { say("No Solana wallet found: install Phantom."); return null; }
  const { publicKey } = await wallet.connect();
  return { wallet, pubkey: publicKey.toString() };
}

// One-time challenge: the server composes the message, the wallet signs it, the server
// verifies the Ed25519 signature and sets the session cookie.
async function signIn() {
  const c = await connect();
  if (!c) return;
  say("Sign the message in your wallet…");
  const challenge = await api("/api/session/challenge", { pubkey: c.pubkey });
  const { signature } = await c.wallet.signMessage(new TextEncoder().encode(challenge.message), "utf8");
  await api("/api/session/verify", { nonce: challenge.nonce, signature: b64(signature) });
  location.reload();
}

// The request body of each transaction kind, from the button's data attributes.
function bodyFor(kind, el) {
  switch (kind) {
    case "buy":
    case "sell": {
      const qty = el.closest("[data-trade]")?.querySelector("select[data-qty]");
      // An empty select (nothing free to sell) has value "": fall back to 1, never send n = 0.
      const n = Number(qty?.value || el.dataset.n || 1);
      return { trader: el.dataset.trader, n };
    }
    case "claim":
      return { week_id: Number(el.dataset.weekId) };
    case "commit": {
      const form = document.getElementById(el.dataset.form ?? "lineup-form");
      const slots = [0, 1, 2, 3, 4].map((i) => form.elements[`s${i}`]?.value ?? "");
      const captain = Number(form.querySelector('input[name="captain"]:checked')?.value ?? 0);
      if (slots.some((s) => !s)) throw new Error("Fill all five slots first.");
      // The preview shard scores the same lineup server-side; refuse what it refuses
      // (ineligible slot, duplicate entity, over the 400 rating cap). The API checks again.
      const valid = document.querySelector("#lineup-preview [data-valid]");
      if (valid && valid.dataset.valid !== "true") throw new Error(`Not committed: ${valid.textContent.trim()}`);
      return { slots, captain };
    }
    default:
      return {};
  }
}

function done(kind, built) {
  const q = built.quote ?? {};
  switch (kind) {
    case "faucet": return `Received ${usdc(q.amount_u)} test USDC.`;
    case "buy": return `Bought ${q.n} for ${usdc(q.cost_u)}.`;
    case "sell": return `Sold ${q.n} for ${usdc(q.proceeds_u)}.`;
    case "commit": return `Lineup committed for week ${q.week_id}.`;
    case "claim": return `Claimed ${usdc(q.prize_u)}.`;
    default: return "Done.";
  }
}

async function transact(el) {
  const kind = el.dataset.walletTx;
  const c = await connect();
  if (!c) return;
  if (document.body.dataset.session !== c.pubkey) { await signIn(); return; }
  if (!window.solanaWeb3) throw new Error("@solana/web3.js did not load");
  const body = bodyFor(kind, el);
  say("Building transaction…");
  const built = await api(`/api/tx/${kind}`, body);
  const tx = window.solanaWeb3.VersionedTransaction.deserialize(unb64(built.transaction));
  say("Approve the transaction in your wallet…");
  const { signature } = await c.wallet.signAndSendTransaction(tx);
  say(`Sent ${signature}, confirming…`);
  const res = await api("/api/tx/confirm", { signature, wait_mirror: kind !== "faucet" });
  if (res.status === "failed") throw new Error(`transaction failed: ${JSON.stringify(res.err)}`);
  if (res.status === "pending") { say(`Not confirmed yet: ${signature}. Check it in an explorer before retrying.`); return; }
  say(`${done(kind, built)} ${signature}`);
  document.body.dataset.lastTx = signature;
  setTimeout(() => location.reload(), res.mirrored || kind === "faucet" ? 600 : 3000);
}

// Wallet flows in flight, and when the last one ended: the auto-reload waits for both.
let busy = 0;
let lastFlowEnd = 0;

async function run(action, el) {
  el.disabled = true;
  busy += 1;
  try { await action(el); } catch (e) { say(e?.message ?? String(e)); } finally {
    el.disabled = false;
    busy -= 1;
    lastFlowEnd = Date.now();
  }
}

// Quantity selectors: the button shows the quote the server rendered for that many copies.
// Also resynced on pageshow, in case the browser restored a selection (back/forward cache).
function syncQty(sel) {
  const btn = sel.closest("[data-trade]")?.querySelector("[data-wallet-tx]");
  const label = sel.selectedOptions[0]?.dataset.label;
  if (btn && label) { btn.textContent = label; btn.dataset.n = sel.value; }
}
document.addEventListener("change", (event) => {
  const sel = event.target.closest?.("[data-trade] select[data-qty]");
  if (sel) syncQty(sel);
});
window.addEventListener("pageshow", () => document.querySelectorAll("[data-trade] select[data-qty]").forEach(syncQty));

document.addEventListener("click", (event) => {
  const el = event.target.closest("[data-wallet-connect],[data-wallet-tx],[data-wallet-logout]");
  if (!el) return;
  event.preventDefault();
  if (el.hasAttribute("data-wallet-connect")) run(signIn, el);
  else if (el.hasAttribute("data-wallet-tx")) run(transact, el);
  else run(async () => { await api("/api/session/logout"); location.reload(); }, el);
});

// Countdowns: "in 1h 02m 03s", then "now".
function tick() {
  const now = Date.now();
  for (const el of document.querySelectorAll("[data-countdown]")) {
    let s = Math.round((Number(el.dataset.countdown) - now) / 1000);
    if (s <= 0) { el.textContent = "now"; continue; }
    const h = Math.floor(s / 3600); s -= h * 3600;
    const m = Math.floor(s / 60); s -= m * 60;
    const pad = (n) => String(n).padStart(2, "0");
    el.textContent = `in ${h > 0 ? `${h}h ` : ""}${pad(m)}m ${pad(s)}s`;
  }
}
tick();
setInterval(tick, 1000);

// Live pages reload themselves (points are republished every 10 minutes), but never in the
// middle of a wallet flow, nor within 30 s of one ending (its message stays readable).
const auto = document.querySelector("[data-autoreload]");
function autoReload() {
  if (busy > 0 || Date.now() - lastFlowEnd < 30_000) { setTimeout(autoReload, 5_000); return; }
  location.reload();
}
if (auto) setTimeout(autoReload, Number(auto.dataset.autoreload) * 1000);
