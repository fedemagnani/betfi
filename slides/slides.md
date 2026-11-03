---
theme: default
colorSchema: dark
title: BetFi
info: Decentralized betting for everyone, powered by Pascal.
fonts:
  sans: Inter
  mono: JetBrains Mono
transition: fade
drawings:
  enabled: false
layout: center
class: text-center
---

<div class="text-5xl font-semibold tracking-tight">Bet<span class="green">Fi</span></div>

<div class="muted mt-1">decentralized betting for everyone</div>

<div class="flex items-center justify-center gap-2 mt-2 muted text-xs">
  powered by <img src="/logos/pascal.svg" class="h-4" alt="Pascal">
</div>

<!--
A sportsbook for traditional bettors. Prices and hedges from Pascal, settlement on Solana.
-->

---
layout: center
---

# Value proposition

<div class="points mt-8">
  <div class="item"><b>Simple, intuitive, permissionless</b> <span>betting for traditional bettors</span></div>
  <div class="side"></div>
  <div class="item"><b>Odds made by BetFi</b> <span>from the hedging strategies available on Pascal's order books</span></div>
  <div class="side"><img src="/logos/pascal.svg" class="h-12" alt="Pascal"></div>
  <div class="item"><b>For traditional bettors</b> <span>a smooth experience, copy trading, mid-game parlay cash-out. No trading or crypto jargon</span></div>
  <div class="side"></div>
</div>

<!--
- Permissionless: anyone can bet, no account approval, settlement on Solana.
- Every odd we quote comes from Pascal's order books: we price the cost of hedging the parlay
  there, and every parlay is hedged leg by leg on Pascal with our builder code.
- Volume for Pascal: each leg's entry hedge is close to the stake, so an n-leg parlay trades
  about n times it; legs are re-traded in play; and the flow is uninformed for Pascal's makers.
- The screens show euros, odds and payouts, never tokens or positions.
-->

---
layout: center
---

# Value proposition

<div class="split mt-8">
  <div class="points one">
    <div class="item"><b>Account abstraction</b> <span>Google sign-in through Privy's embedded wallet. No wallet app needed</span></div>
    <div class="item"><b>Bet with any token</b> <span>any crypto asset, swapped into the stake through DEX aggregators</span></div>
    <div class="item"><b>Verified tipsters, shared revenue</b> <span>tipsters earn a share of the copy fee on their parlays; anyone can verify the stakes of the tipster and every copier</span></div>
    <div class="item"><b>Mid-game cash-out</b> <span>take your money early</span></div>
  </div>
  <svg class="flow" width="312" height="246" viewBox="0 0 312 246" role="img" aria-label="Wallets (Privy, Phantom, MetaMask) authenticate the user to BetFi; BetFi swaps through aggregators (0x, Jupiter, DFlow); BetFi hedges on the prediction market Pascal">
    <defs><marker id="bf-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 z" class="head"/></marker></defs>
    <rect class="box" x="23" y="1" width="144" height="72" rx="10"/>
    <text class="name" x="95" y="15">wallets</text>
    <image href="/logos/privy.png" x="37" y="26" width="46" height="12"/>
    <image href="/logos/phantom.png" x="97" y="26.5" width="56" height="11"/>
    <image href="/logos/metamask.png" x="74.5" y="45" width="36" height="18"/>
    <line x1="95" y1="79" x2="95" y2="113" marker-end="url(#bf-arrow)"/>
    <text class="label" x="85" y="99" text-anchor="end">auth</text>
    <text class="word" x="95" y="136"><tspan class="bet">Bet</tspan><tspan class="fi">Fi</tspan></text>
    <line x1="134" y1="123" x2="168" y2="123" marker-end="url(#bf-arrow)"/>
    <line x1="168" y1="133" x2="134" y2="133" marker-end="url(#bf-arrow)"/>
    <rect class="box" x="172" y="91" width="138" height="74" rx="10"/>
    <text class="name" x="241" y="107">aggregators</text>
    <image href="/logos/jupiter.png?v=2" x="179.5" y="114.5" width="60.4" height="25"/>
    <image href="/logos/dflow.png" x="251.9" y="120" width="14" height="14"/>
    <text class="mark" x="269.9" y="131">DFlow</text>
    <image href="/logos/0x.svg" x="232.9" y="143.5" width="21" height="13"/>
    <line x1="95" y1="148" x2="95" y2="182" marker-end="url(#bf-arrow)"/>
    <text class="label" x="85" y="168" text-anchor="end">hedging</text>
    <rect class="box" x="23" y="188" width="144" height="56" rx="10"/>
    <text class="name" x="95" y="202">prediction market</text>
    <image href="/logos/pascal.svg" x="58.25" y="212" width="73.5" height="20"/>
  </svg>
</div>

<!--
- Privy turns a Google login into an embedded Solana wallet. Our sponsor key co-signs every
  transaction as fee payer, so the user never holds SOL either.
- Any token: the aggregator's swap and the stake land in one transaction, or not at all.
  On devnet the swap is our mock AMM, answering in Jupiter's shape.
- Copy fee: 2% of the copied stake, split by the program at mint, 80% to the tipster.
  A tipster's record is built only from settled bets they staked themselves.
- Cash out at a desk-signed price from Pascal's live books, settled on chain.
-->

---
layout: center
---

# Stack · Rust native <img src="/logos/rust.svg" class="title-mark" alt="">

<div class="split snug mt-8">
  <div class="points one">
    <div class="item"><b>Backend</b> <span>tokio · axum · sqlx</span></div>
    <div class="item sub"><b>desk</b> <span>quoter, relay, crank</span></div>
    <div class="item"><b>Frontend</b> <span>Dioxus, compiled to wasm</span></div>
    <div class="item"><b>On-chain</b> <span>Anchor</span></div>
    <div class="item sub"><b>schedina</b> <span>the main program: escrow, vault, tickets</span></div>
    <div class="item sub"><b>mock-amm</b> <span>swap pools and faucets on devnet</span></div>
  </div>
  <svg class="flow" width="176" height="245" viewBox="0 0 176 245" role="img" aria-label="Three Rust layers: the Dioxus frontend asks the tokio and axum backend for quotes; the backend sends transactions to the Anchor programs on Solana">
    <defs><marker id="bf-arrow-stack" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 z" class="head"/></marker></defs>
    <rect class="box" x="4" y="1" width="168" height="50" rx="10"/>
    <text class="name" x="88" y="15">frontend</text>
    <image href="/logos/dioxus.png" x="60.4" y="23" width="15.5" height="20"/>
    <text class="mark" x="79.9" y="37">Dioxus</text>
    <line x1="88" y1="57" x2="88" y2="91" marker-end="url(#bf-arrow-stack)"/>
    <text class="label" x="78" y="77" text-anchor="end">quotes</text>
    <rect class="box" x="4" y="97" width="168" height="50" rx="10"/>
    <text class="name" x="88" y="111">backend</text>
    <image href="/logos/tokio.png" x="43.85" y="120" width="20" height="18"/>
    <text class="mark" x="68.85" y="133">tokio · axum</text>
    <line x1="88" y1="153" x2="88" y2="187" marker-end="url(#bf-arrow-stack)"/>
    <text class="label" x="78" y="173" text-anchor="end">transactions</text>
    <rect class="box" x="4" y="193" width="168" height="50" rx="10"/>
    <text class="name" x="88" y="207">on-chain</text>
    <image href="/logos/anchor.png" x="22.5" y="217" width="16" height="16"/>
    <text class="mark" x="42.5" y="229">Anchor</text>
    <image href="/logos/solana.png" x="94.5" y="219.5" width="59" height="11"/>
  </svg>
</div>

<!--
- Desk: one tokio process. The quoter prices the slip on Pascal's books and signs it; the relay
  co-signs users' transactions as the sponsor; the crank finalizes, resolves and settles.
  Also the hedge desk, the indexer and the catalogue.
- Our crates: pricing (the math), pascal-rs (Pascal client and order signing),
  pascal-mock (Pascal's exact API filling against the live books: Pascal has no devnet).
- The app builds and signs every transaction itself; it never trusts the desk with money.
-->

---
layout: full
class: "!p-4 flex items-center justify-center"
---

<img src="/lifecycle.svg" class="max-h-full max-w-full" alt="BetFi bet lifecycle">

<!--
1. Quote: priced on Pascal's books and signed, valid 10 s.
2. Escrow: the user signs open_pending, the relay co-signs and pays the fee.
3. Hedge: the indexer sees the escrow, the hedge desk buys every leg on Pascal.
4. Mint: finalize locks the full payout in the vault; nothing filled, refund_pending.
5. Results: the crank reads Pascal's results and resolves each leg.
6. Payout: settle pays from the vault, or the user cashes out earlier.
-->

---
layout: center
---

# Odds-making

<div class="split mt-8">
  <div class="points one">
    <div class="item"><b>Single-event parlay</b> <span>easy: buy the payout on the book</span></div>
    <div class="item"><b>Multi-event parlay</b> <span>difficult: no market on "every leg wins", several strategies</span></div>
    <div class="item sub"><b>Static superhedge upfront</b> <span>safe for the protocol, unappealing quotes</span></div>
    <div class="item sub"><b>Live rebalancing</b> <span>dynamic programming; expected hedge error ≥ 0, set by the odds‑maker's risk aversion λ</span></div>
  </div>
  <div class="eqs">
  <div class="side eq">
  <div class="lhs">

$o =$

  </div>
  <div>

$\dfrac{1-m}{\bar p(S)}$

  </div>
  </div>
  <div class="side eq">
  <div class="lhs">

$o^{\mathrm{st}} =$

  </div>
  <div>

$\dfrac{1-m}{\min_i p_i}$

  </div>
  </div>
  <div class="side eq">
  <div class="lhs">

$o^\star =$

  </div>
  <div>

$\dfrac{1-m}{g\big(S\, o^\star\big)}$

  </div>
  </div>
  </div>
</div>

<!--
- Single event: p̄ is the average fill price walking the book for the payout, m the haircut.
  The hedge is exact and model-free; bigger stakes walk deeper, so the odd falls.
- Static: the Fréchet bound. It quotes the odd of the least likely leg alone, so a bettor would
  do better with single bets.
- Live: g(N) is the all-in hedge cost per unit of payout, including a λ·Std premium on the hedge
  error. The odd is a fixed point (eq. 11 of the paper), solved by bisection.
- Two simulated football matches, m = 5%: static 2.35, live 3.76 (λ = 1) and 4.16 (λ = 0.25),
  competitive 4.96.
-->

---
layout: center
---

# Revenue

<div class="points one mt-8">
  <div class="item"><b>Odds haircut</b> <span>3% per leg, inside the quoted total</span></div>
  <div class="item"><b>Cash-out</b> <span>2% of the ticket's value</span></div>
  <div class="item"><b>Copy fee</b> <span>2% of the copied stake: 20% to the protocol, 80% to the tipster</span></div>
</div>

<!--
- The haircut is 5% per leg when legs overlap in time.
- Example: 10 USDC on two overlapping legs pays 53.80; expected take 0.975 before about
  0.20 of Pascal fees.
-->
