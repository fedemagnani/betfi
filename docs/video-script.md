# Demo video script (≤ 3 min)

The script for the BetFi v2 screens (L7, the BetFi C2 mockup) on branch `l7`. The app is shown
as **BetFi**, in English, dark only. Record on **today's live slate** (`just demo`), never on the
design build: the design build shows the mockup's fictional content and is for the visual review
only. Times are the cut's, not the wall clock's: the crank's devnet transactions take a few
seconds each, so record the beats as separate takes and cut the waits.

The order of the screens is fixed: Sportsbook → bet drawer → placement celebration → event
detail → tipster profile → copiers drawer → copy confirm → cash-out celebration.

## The slate and the four tickets

Fill this table from the banner of the `just demo` you record with. The rehearsal's slate is
shown as an example.

The 30 Oct rehearsal's slate, for reference: 10 NBA games from 23:00 UTC (for example Hornets @ Wizards `NBA_26OCT30_CHAWAS_ML.CHA` 23:00, Pistons @ Nets `…_DETBKN_ML.DET`, Knicks @ 76ers `…_NYKPHI_ML.NYK`, Bulls @ Celtics `…_CHIBOS_ML.CHI`), moneylines only at 09:00 UTC; the Soccer tab showed Bundesliga (Elversberg – Mainz, 19:30, `BUND_26OCT30_ELVM05_ML.{ELV,DRAW,M05}`) and LaLiga (Villarreal – Espanyol, 20:00, `LALIGA_26OCT30_VILESP_ML.{VIL,DRAW,ESP}`), no EPL. With a soccer leg on the slip the whole take must finish before that game's start minus 60 s (19:29 UTC that day).

| Game | Sport | Start (UTC) | Symbol the banner prints (or the card lists) |
| --- | --- | --- | --- |
| N1: first NBA game | Basketball | _hh:mm_ | `NBA_<date>_<AWAYHOME>_ML.<AWAY>` |
| N2: second NBA game | Basketball | _hh:mm_ | `NBA_<date>_<AWAYHOME>_ML.<AWAY>` |
| N3: third NBA game | Basketball | _hh:mm_ | `NBA_<date>_<AWAYHOME>_ML.<AWAY>` |
| S1: a soccer match, if the card lists one | Soccer | _hh:mm_ | `EPL_<date>_<HOMEAWAY>_ML.{HOME,DRAW,AWAY}` |

Four tickets, so that resolving a game for the cash-out never touches the tip being copied
(D-431):

- **W, the tip** (tab 1, mock USDC, "Share as a tip" on, made in the pre-flight): N1 and N3
  winners, plus S1's draw when there is a soccer game. Named, for example "Friday treble".
- **W′, its first copy** (tab 3, made in the pre-flight), so the copiers drawer has a row.
- **B, the bet on camera** (tab 1, paid in SOL): N2 and N3 winners. Resolving N2 wins B's first
  leg and leaves W untouched.
- **W″, the copy on camera** (tab 2, mock USDC): a copy of W.

## Pre-flight checklist

- [ ] **One desk only.** Stop every other `just demo` and `just demo-devnet`, in every checkout
      (`pgrep -fl target/debug/desk` prints nothing). Two desks share the crank key and race on
      `settle`; the program pays once and refuses the other with `TicketNotOpen`, which shows
      as an error line on camera (D-231).
- [ ] **Cut times.** Record well before the first start on the slate. Tips and copies stay open
      while a game is live (D-660); spread and total legs leave the card at tip-off (D-301), so a
      copy of W holding them needs those games unstarted until the copy beat is done.
- [ ] **The slate.** `cargo test -p pascal-mock --test live live_smoke_real_upstream -- --ignored --nocapture`
      prints the NBA moneylines starting within 24 h; you need at least 3, or `just demo` falls
      back to the replay (where tips and copies do not work). The Soccer tab shows only when the
      day's card has a soccer game (D-480); without one, skip S1 and the soccer lines below.
- [ ] **SOL on `demo-3`.** The three burners need about 0.045 SOL in all:
      `solana balance 6UDpy1nCGH5XkFPzXxLDivzAV4NCZR5e3GsoQP6ttFuz --url https://api.devnet.solana.com`.
      Measured on 30 Oct: a tipster with a mockBONK and a mockSOL ticket used 0.025 SOL, a copier 0.008 SOL (wallet 2's transfer takes about 12 s to confirm on public devnet).
- [ ] **The vault.** `just crank-status-devnet`: `paused false`, and `free` well above the
      payouts you will lock (a few hundred USDC at most).
- [ ] **Start.** `just demo` (from a worktree: `SCHEDINA_ENV_FILE=<main checkout>/.env just demo`). Copy
      the banner's `desk` URL and its `curl … /mock/resolve` lines into a scratch file; both
      ports change on every start.
- [ ] **The SOL price, then the mockSOL peg.** Paying in SOL swaps mockSOL through the devnet
      pool, which must sit at the real SOL price for the drawer's ≈ SOL figure to hold (V-3).
      First wait until the desk serves a market price, not its fallback:
      `curl -s http://127.0.0.1:<desk port>/fx` shows `"sol_fallback":false` (the desk reads
      the price every 60 s). Then, signed by the upgrade authority as the AMM's admin (never the
      desk's key):

      ```sh
      cargo run -q --locked -p desk --bin crank -- --url devnet \
        --authority "$AUTHORITY_KEYPAIR_PATH" \
        amm-peg --from-fx --desk http://127.0.0.1:<desk port> --yes
      ```

      Run it again until it prints `amm-peg: within 1% of the target; nothing sent`. Each run
      moves the pool by at most 10% of a reserve, so a far price takes several runs (D-479).
      Leave out `--yes` for a dry run that prints the plan. `just demo --e2e` with `E2E_SOL=1`
      does the same loop by itself (D-547).
      On 30 Oct the pool started at 200 USDC per mockSOL (+106%) and took four bounded swaps to reach 96.945 USDC per SOL (€87.10 ÷ 0.8985 EUR/USD); each later run found it 1.4–1.6% low after the previous mockSOL ticket and one swap re-pegged it (signatures in `git show cb85bed:docs/demo-devnet.md`).
- [ ] **Pascal's books, and the price on the slip pill.** On 29 Oct Pascal's NBA books were
      empty for about 25 minutes: the Sportsbook still showed values, but the drawer never
      priced. Before recording, put one pick on the slip and check that the purple slip pill
      shows "1 Pick • Total x.xx" and the drawer shows Total and Payout; or run
      `curl -s 'https://data.pascal.trade/api/v1/books?symbols=<symbol>'` on one game.
- [ ] **RPC.** For a local recording only, `DEMO_APP_HOSTED_RPC=1 just demo` puts the hosted RPC
      in the app bundle, so the browser avoids the public endpoint's 429 answers. That bundle
      embeds an API key: serve it on localhost only, never publish or deploy it, and keep the
      browser's network tab off camera. Without the variable the app uses public devnet RPC.
- [ ] **Screen.** Chrome at 390 × 844 (device toolbar), the browser's time zone set to US
      Eastern (or say that times show in yours: a CET browser shows the US night as the next
      day). Next to it, the terminal running `just demo`, with a font large enough for the
      `[desk]` and `[mock]` lines.
- [ ] **Burner tabs.** Tabs 1 (the tipster), 2 (the copier on camera) and 3 (the first copier):
      open the banner's `dev wallet` URL in each and tap **Create a fresh test wallet (in this
      tab only)**. Fund each from `demo-3`, tab 1 first (it pays for W, B and the cash-out):

      ```sh
      solana transfer <burner address> 0.025 --url https://api.devnet.solana.com \
        -k <demo-3 keypair> --allow-unfunded-recipient
      ```

      (0.025 for tab 1, 0.01 each for tabs 2 and 3; the second transfer can take about 12 s to
      confirm). A burner lives only in its tab's memory: from now on **never reload a tab, type
      a URL or use a bookmark** in tabs 1 to 3. Move with the bottom nav and in-app links only.
- [ ] **Test money.** In each tab, **Account → Deposit → Get 1,000 test USDC**. In tab 1 also
      **Get 1 mockSOL** (the pill's SOL pays in mockSOL).
- [ ] **Handle.** In tab 1, **Account → Profile**, type a handle under **Handle** (3 to 20 of
      `a-z 0-9 _`), **Save profile**. The profile, the copiers drawer and the copy page then
      show `@handle` instead of an address.
- [ ] **W, the tip.** In tab 1 on the Sportsbook, tap N1's and N3's winners (and on the
      **Soccer (1–X–2)** tab, S1's **X**). Open the slip pill, choose **€10**, turn on **Share
      as a tip**, tap **Bet • €10.00**, wait for "Schedina Locked In!". Then **Account → Your
      tipster page →** the tip **→ Name this tip**, type a title, **Save**.
- [ ] **W′, the first copy.** In tab 3: **Tipsters → Show everyone →** the handle → tap W under
      **ACTIVE PARLAYS → Copy Schedina • Total x →**, choose **€10**, **Copy Schedina • €10.00**.
      Back in tab 1, nothing more to do: the copiers drawer will list tab 3's copy.
- [ ] **Terminal.** Have the `verify-order` command from [`docs/demo.md`](demo.md#signed-not-sent) typed in a second terminal
      (it reads [`docs/signed-batch.json`](signed-batch.json)), so the honesty beat is one Enter.
- [ ] **Tabs 4 and 5.** Tab 4: [`docs/demo.md`](demo.md#signed-not-sent) on the repository. Tab 5: Solscan on devnet, for
      View Transaction.

Lessons from the L5 and L6 rehearsals (`git show cb85bed:docs/rehearsal.md`) that still hold:

- **The resolve lines.** The banner prints one line per NBA game, every one with
  `"price": "1"`, and `"1"` there means **the away team wins** (the first team in the banner's
  "Away @ Home"). That is the mock's price, not the app's outcome label: on the app's buttons
  `1` is the home team (D-482). For a home win send `"0"`. A total resolves on its own
  `…_TT.<line>` symbol and a soccer leg on its outcome's own symbol (`…_ML.DRAW` and so on),
  which the banner does not print: take them from the desk's card ([`docs/demo.md`](demo.md), "Resolving
  a game"). A market resolves once per mock session.
- **The cash-out offer lasts 30 s.** Tap **Confirm cash out €…** promptly.
- **Paying in SOL or mockBONK turns tips off** ("Tips are off: …", D-221): tip in euros, pay a
  separate bet in SOL. That is why W is in euros and B in SOL.
- **The fee line is the desk's real margin**, about 9% to 17% on a multi-leg parlay on a live
  slate (D-363), not the mockup's 2%. Say "our margin, shown in euros" rather than reading the
  number. Each overlapping leg pays the 5% tier; that is the overlap margin, not a bug.
- **The price-moved banner** ("One price moved" with **Accept**) shows only when a price moves
  against you, which may not happen during the take. To show it, use the design build's seeded
  example (`just phone-gate`, the design pass) and say that it is seeded.
- **My Bets offers Cash out on the tip too.** Do not tap W's button by mistake: cash out B.

## The script

Measured on 30 Oct (run 7): Bet → minted about 3–5 s; a leg resolved → Won and Cash out offered about 14 s; offer → "Cash Out Secured!" about 1.4 s; a tip's games resolved → B and its copy settled on screen about 22 s; the mockBONK faucet and swap about 17 s.

Cut these waits: Bet → "Schedina Locked In!" (escrowed, hedged, minted: a few seconds), a
resolution → "Cash out €…" offered (about 11 s in L6), and Confirm cash out → "Cash Out
Secured!".

| Time | On screen (exact clicks) | Say |
| --- | --- | --- |
| **0:00** | Tab 1, **Sports**. The app bar: BETFI and the balance pill, one euro figure for everything in the wallet. The live ticker line under it. Tabs **Basketball (1–2)** and **Soccer (1–X–2)**: tap Soccer to show S1's three buttons `1`, `X` and `2`, then back to Basketball. "UPCOMING MATCHES · Today": each card's values, the lowest green and the highest purple. | "This is BetFi, a parlay app on Pascal's markets: the NBA, and soccer's 1-X-2. Every number is priced off Pascal's live order books, shown in euros and in SOL at the real SOL price. This chip says TEST: devnet test money, and the hedges are mock-filled. I'll show you exactly what that means." |
| **0:15** | Tap N2's away team and N3's pick (the same team as in W). The purple slip pill: "2 Picks • Total x.xx SCHEDINA ›". Tap it: the bet drawer rises over the blurred Sportsbook. The picks with their league; **Stake Amount**: tap the **€25** preset (or drag the green slider), then the **€ EUR ▾** pill and choose **SOL**: under €25.00 it now reads "Pay 0.xx SOL (1 SOL = €…)", and the swap's fee line appears. **Total** and **Payout**. Point at **Share as a tip**: "Tips are off" while paying in SOL. Tap **Bet • … SOL**. The button walks through "Approve in your wallet…", "Escrowed…", "Hedging…", "Minted ✓". | "Two games, one slip. I pay in SOL: it is swapped into the stake in the same transaction, at today's SOL price. Tap Bet." |
| **0:35** | Terminal: `[desk] … schedina::signed_order` lines, one per leg, `tif=Ioc`; `[mock] … filled`; `[desk] feed: … minted`. Back to tab 1: the placement celebration: confetti, **Schedina Locked In!**, **Maximum Potential Payout** counting up in € with ≈ SOL, Stake Placed, Total, the legs sliding in. Tap **View Transaction ↗**: Solscan, the `open_pending` with the swap in it. | "The desk hedges every leg with an IOC order on Pascal, carrying our builder code. Honest part: no Pascal account is funded yet, so these orders are signed for Pascal but filled by our mock against Pascal's live book. The payout is locked in a vault on Solana before the ticket exists." |
| **0:55** | Second terminal: Enter on `cargo run -q -p pascal-rs --bin pascal-cli -- verify-order docs/signed-batch.json --signer 5ggBcWJNqYzf4jPNQu2tsEQWNDak2v3b3Vtgmsb4dQ5S --builder 7FJuR9h5skiCySUZTXvfqgUoZcmpu6wQy9oSzYJ24gC5 --builder-fee-rate 0.001` → `3 request(s), 3 verified, 0 failed`. Tab 4 for a second: [`docs/demo.md`](demo.md#signed-not-sent). | "Here is one signed batch, never sent. Anyone can check the signatures and the builder bytes, byte by byte." |
| **1:05** | Tab 1, close the celebration, back on **Sports**: tap N1's card header (league, time, teams). Event detail: the start time in the score slot, **Match Winner (1-2)** with the two outcome cards, **Staked Sentiment** with each side's € and ≈ SOL, **Pool Participation** with the traders, the recent stakes, and **More markets** (tap it: the spread and total lines). The purple **Select to bet** at the bottom. | "Each game has its own page. Pascal's open interest is the same on both sides, so it can't say who people back; we rebuild it from Pascal's public trade log, market makers left out. Pascal has no scores, so a live game shows each side's chance from the price instead." |
| **1:25** | Tab 2. **Tipsters**, turn on **Show everyone**, tap tab 1's handle. The profile: avatar, PRO only if verified, **Follow**, **30D ROI**, **Win Rate**, **Copiers**, **ACTIVE PARLAYS**: tap W (title, legs, € staked, € pool, team chips, Total). | "Tips are bets their tipsters made with their own money. The record counts settled tips only, and handles are signed by the wallet." |
| **1:40** | Tap **View Pool (1) →**: the copiers drawer over the lit profile. "Copiers (1)", **Live Pool**, the Pool Volume in € and ≈ SOL, tab 3's row: avatar, name, € staked, ↗. Close it with ✕. | "Every copy is on chain: each row opens its transaction on Solscan." |
| **1:50** | The docked **Copy Schedina • Total x →**. The copy confirm page: the tipster strip, the **Lock** countdown (the quote's real 10 s), **Schedina Selection** with each leg's PREMATCH tag, **Stake Amount** with **€25**, the summary: Total, Stake, **Copy fee (2% · 80% to @handle)**, **Potential Payout** in € and ≈ SOL. Tap **Copy Schedina • €25.00**. Cut to its celebration: **Schedina Locked In!**, "A copy of bet …". | "One tap copies it: same picks, a fresh price, and a 2% fee on top, 80% to the tipster, split on chain by the program." |
| **2:15** | Terminal: paste N2's line from the banner, for example `curl -s -X POST http://127.0.0.1:59111/mock/resolve -H 'content-type: application/json' -d '{"symbol": "NBA_26OCT30_XXXYYY_ML.XXX", "price": "1"}'` (`"0"` if you picked N2's home team). `[desk] feed: … leg 1 … won`. Tab 1, **My Bets**, **Open**: B with its first leg won, "Cash out now" with its value. Tap **Cash out €…**, then **Confirm cash out €…** ("Offer valid for … s"). | "I force this game's result on the mock's clock; the real game is tonight. The crank resolves the leg on chain. Now cash out at the value shown: signed by the desk, checked by the program." |
| **2:30** | The cash-out celebration: green and gold confetti, **Cash Out Secured!**, **Total Payout Received** counting up in € with ≈ SOL, **View Transaction ↗** and **Brag on 𝕏**. | "Paid out on chain, in one transaction." |
| **2:40** | **Account**: the pill **Test money · hedges mock-filled**, then scroll to **The house**: the vault (free and locked), "Hedges · signed for Pascal, mock-filled", the fee totals with **Builder revenue** (0.1% builder code, computed by the mock in mode M). | "Behind every bet: a vault that locks the full payout, and the hedges with our builder code at 0.1%, which the mock computes while no Pascal account is funded." |
| **2:50** | README top: repo URL, team. | "Schedina, shown in the app as BetFi. Team &lt;names&gt;, Vitruvium &lt;city&gt;. The repo is &lt;URL&gt;." |

Lines to keep word for word, because they are the honesty lines: "devnet test money, and the
hedges are mock-filled", "signed for Pascal but filled by our mock", "no Pascal account is funded
yet", "I force this game's result on the mock's clock". The brag posts say "devnet test money"; the full pill "Test money · hedges
mock-filled" shows only on Account (D-456), so the 2:40 beat shows it.

## If something is missing on the night

- **No soccer game on the card.** The Soccer tab is hidden (D-480). At 0:00 say "and soccer's
  1-X-2 when Pascal lists a match that day", and show the design build's soccer shot
  (`docs/screens/`) for a second, saying it is the design build.
- **mockSOL not pegged** (`amm-peg` failed, or `/fx` keeps `"sol_fallback":true`): pay B in
  **EUR** instead and say "it can also pay in SOL"; the ≈ SOL figures still show, at the
  fallback rate. mockBONK works the same way through **BONK** in the coin selector (Account first: **Get
  50,000 mockBONK**).
- **"No copies yet."** in the copiers drawer: W′ was skipped or not minted. Say "nobody yet", and
  after the copy at 1:50 reopen **View Pool** for one second.
- **Thin Staked Sentiment** ("Too few traders yet") on N1: open another NBA game's page. Soccer
  games have no sentiment yet.

## Optional cuts

Use one of these in place of a beat above, or drop them. Each needs its own take.

- **In play.** Once a game has started, the Sportsbook shows a live card: "Live · n′", each
  side's chance in the score slot (no scores: Pascal has none, D-300), and **View Market Volume
  →** to its event page. Only its winner can still be picked, until 2 h after the start. Say:
  "In play, only the winner: priced on the live book, hedged at once."
- **A spread or total.** On the event page, **More markets**, tap **Spread** or **Total** and a
  line: it replaces that game's other pick on the slip (one leg per game). Resolve it on its own
  symbol ([`docs/demo.md`](demo.md)).
- **Follow.** On the profile, **Follow** signs a message with the wallet (no transaction, no fee)
  and the desk stores it (V-5).
- **Brag on 𝕏.** Opens a post that says "devnet test money" (D-495).
- **The design build** (`just visual`): the mockup next to the app, screen by screen. Say that
  its content is the mockup's, fictional.

## Replay fallback

`just demo --replay` (or the automatic fallback, which prints a `NOTICE`) plays the 28 Oct 2026
night at 60x: NBA winners only, no soccer, no spreads or totals. **Tips and copies do not work
in replay**: the tape's games are in the past for the chain clock, so the program refuses them
(`TipNotAllowed`, `CopyTooLate`). For 1:25 to 2:15, cut in a live take recorded earlier, or show
the rehearsal's tip and copy transactions (`git show cb85bed:docs/demo-devnet.md`) on Solscan. Say "a recorded
night of real Pascal books" instead of "Pascal's live order books".

From the 30 Oct L7 rehearsal (run 7, Solscan devnet): the tip's `open_pending` [5xEQQkN8…](https://solscan.io/tx/5xEQQkN8k3yABm7gDa51awWMhBe1kyhuwCBhiXpZaDqCfbaqU5erTwZm8bby1RNJ9ytsaGWjfHd6VKDgGm5daN2?cluster=devnet), the copy's `open_pending` [4ESi9aki…](https://solscan.io/tx/4ESi9akiAHPuW2TdjXzx2BSreNgNgno5zPet9G7j5uJvDxDDTKo1MftV8uRbXBByxNPU8QKx3YPG23wCvnEtMmxp?cluster=devnet), its `finalize` with `TipsterPaid` 0.089120 USDC to the tipster [31PrSPFm…](https://solscan.io/tx/31PrSPFm66psMY8cHWwAPyuygHx3kEEU7mNXi7UYi5MsVw2hdzxuRyN9K1uYTwxALd8qwkNiwCSRgMjC6sG3aajr?cluster=devnet), the mockSOL `swap` + `open_pending` [4v7TrJdA…](https://solscan.io/tx/4v7TrJdAxc1RY7kwyQyiUJP3vcDTFeSvqPgDbaoooUEZJSegSeSr8zQ6vqGPwRjNnVTtmVYsosGZudMs8zXpv4bx?cluster=devnet) and the `cash_out` [4AxsZ3hu…](https://solscan.io/tx/4AxsZ3huvWeCen9y9Ai9sY9WcCuVBquH5h3vhFw1cSoD7QHVv8NPDGF88yEJjbpxSiNhCsSTqFwiZtm4ViZjSJkX?cluster=devnet).
