// window.SchedinaPrivy (step 8.D, contract L8-3): "Continue with Google" through Privy, and the
// user's Privy embedded Solana wallet as a signer. Bundled by `just privy-bundle` into
// app/assets/privy.js (an ES module); the app imports it lazily (app/src/privy.rs) on the first
// Google tap, or at boot when the URL carries Privy's OAuth return or a Privy session is stored.
//
// Every method returns a Promise. Bytes cross the bridge as Uint8Array. One Privy client per page.
// The public app ID is all this needs: there is no client secret anywhere.

import Privy, {
  LocalStorage,
  getEntropyDetailsFromUser,
  getUserEmbeddedEthereumWallet,
  getUserEmbeddedSolanaWallet,
} from "@privy-io/js-sdk-core";

/** The query parameters Privy appends to the redirect URI after the Google round trip. */
const OAUTH_PARAMS = ["privy_oauth_code", "privy_oauth_state", "privy_oauth_provider"];

let privy = null;
let ready = null;
let iframe = null;
let onWindowMessage = null;
// The Solana provider for the current user's embedded wallet, built once per address.
let provider = null;
let providerAddress = null;

const toB64 = (bytes) => {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(s);
};
const fromB64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const asBytes = (b) => (b instanceof Uint8Array ? new Uint8Array(b) : Uint8Array.from(b));

function client() {
  if (!privy) throw new Error("Privy is not initialised");
  return privy;
}

/** Create the one client, the hidden wallet iframe, and initialise. Idempotent. */
function init(appId) {
  if (ready) return ready;
  ready = (async () => {
    if (!appId) throw new Error("no Privy app ID");
    privy = new Privy({ appId, storage: new LocalStorage() });
    iframe = document.createElement("iframe");
    iframe.src = privy.embeddedWallet.getURL();
    iframe.title = "Privy wallet";
    iframe.setAttribute("aria-hidden", "true");
    iframe.tabIndex = -1;
    iframe.style.display = "none";
    document.body.appendChild(iframe);
    privy.setMessagePoster(iframe.contentWindow);
    onWindowMessage = (e) => {
      if (!iframe || e.source !== iframe.contentWindow) return;
      let data = e.data;
      if (typeof data === "string") {
        try { data = JSON.parse(data); } catch { return; }
      }
      privy.embeddedWallet.onMessage(data);
    };
    window.addEventListener("message", onWindowMessage);
    await privy.initialize();
  })();
  // A failed start (offline, storage blocked) may be retried by the next tap.
  ready.catch(() => {
    if (onWindowMessage) window.removeEventListener("message", onWindowMessage);
    if (iframe) iframe.remove();
    privy = ready = iframe = onWindowMessage = null;
  });
  return ready;
}

/** Ask Privy for Google's authorisation URL and go there. Resolves to the URL. */
async function startGoogleLogin(redirectUri) {
  const { url } = await client().auth.oauth.generateURL("google", redirectUri);
  if (!url) throw new Error("Privy returned no Google URL");
  window.location.assign(url);
  return url;
}

/** Back from Google: log in with the returned code, then strip Privy's query parameters. True
 * when a login happened, false when the URL carried none. `search` is the query string as the
 * page loaded with it (the app's router may have rewritten the URL since); default: the URL's. */
async function completeLoginIfRedirected(search) {
  const params = new URLSearchParams(search ?? window.location.search);
  const code = params.get("privy_oauth_code");
  const state = params.get("privy_oauth_state");
  if (!code || !state) return false;
  try {
    await client().auth.oauth.loginWithCode(code, state, "google");
    return true;
  } finally {
    const url = new URL(window.location.href);
    OAUTH_PARAMS.forEach((p) => url.searchParams.delete(p));
    window.history.replaceState(window.history.state, "", url.pathname + url.search + url.hash);
  }
}

async function currentUser() {
  try {
    const { user } = await client().user.get();
    return user ?? null;
  } catch {
    return null; // no session
  }
}

/** The user's embedded Solana address (created if they have none yet), or null when logged out. */
async function solanaAddress() {
  let user = await currentUser();
  if (!user) return null;
  if (!getUserEmbeddedSolanaWallet(user)) {
    const eth = getUserEmbeddedEthereumWallet(user);
    ({ user } = await client().embeddedWallet.createSolana(eth ? { ethereumAccount: eth } : undefined));
  }
  const account = getUserEmbeddedSolanaWallet(user);
  if (!account) throw new Error("Privy did not create a Solana wallet");
  return account.address;
}

async function solanaProvider() {
  const user = await currentUser();
  if (!user) throw new Error("Signed out of Google: continue with Google again");
  const account = getUserEmbeddedSolanaWallet(user);
  if (!account) throw new Error("No Privy Solana wallet");
  if (provider && providerAddress === account.address) return provider;
  const entropy = getEntropyDetailsFromUser(user, account);
  if (!entropy) throw new Error("No entropy details for the Privy wallet");
  provider = await client().embeddedWallet.getSolanaProvider(account, entropy.entropyId, entropy.entropyIdVerifier);
  providerAddress = account.address;
  return provider;
}

/** Sign a serialized transaction (wire format). Privy signs the message bytes into this
 * wallet's signature slot; the app keeps only that slot (app/src/privy/wire.rs). */
async function signTransaction(bytes) {
  const p = await solanaProvider();
  const { signedTransaction } = await p.request({
    method: "signTransaction",
    params: { transaction: asBytes(bytes) },
  });
  return asBytes(signedTransaction);
}

/** Sign raw message bytes: the 64-byte ed25519 signature. */
async function signMessage(bytes) {
  const p = await solanaProvider();
  const { signature } = await p.request({
    method: "signMessage",
    params: { message: toB64(asBytes(bytes)) },
  });
  const sig = fromB64(signature);
  if (sig.length !== 64) throw new Error(`unexpected signature length ${sig.length}`);
  return sig;
}

/** End the Privy session (tokens removed from this device). */
async function logout() {
  provider = providerAddress = null;
  if (!privy) return;
  const user = await currentUser();
  await privy.auth.logout(user ? { userId: user.id } : undefined);
}

window.SchedinaPrivy = Object.freeze({
  init,
  startGoogleLogin,
  completeLoginIfRedirected,
  solanaAddress,
  signTransaction,
  signMessage,
  logout,
});
