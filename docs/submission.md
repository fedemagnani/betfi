# Vitruvium submission checklist

From the Vitruvium skill file (https://it.superteam.fun/vitruvium/skill.md, read again on 30 Oct
2026): the form, the requirements and the "Before you submit" list, item by item, for Schedina
as shown in the BetFi v2 app (L7). **Done** means the repository already satisfies the item;
**You** means the user still has to do it. The skill file gives no deadline: check the submission
page.

## What is left for you

1. **Public repo and push.** Create a public repository, push the branch you submit (`l7` once
   the rehearsal 7.2a-r and these docs are merged), run the secret scan below on it first, and
   put the URL in the README (Team) and the form.
2. **The video** (≤ 3 min). Record from [`docs/video-script.md`](video-script.md) after its
   pre-flight (one desk only, the mockSOL peg, three burner tabs), upload it, and replace
   "Video: _link to be added_" in the README.
3. **Team.** Names, roles, the edition attended and a contact in the README's Team section (the
   `<!-- 5.C -->` markers), and in the video's closing line.
4. **The builder-code question to the organisers** (below).
5. **The form**, one person per team.

## Requirements

| Requirement (skill file) | Status | What satisfies it, or what is left |
| --- | --- | --- |
| Built on Pascal's API | Done | Live catalogue (NBA winners, spreads and totals; EPL, Bundesliga and LaLiga 1-X-2), books, trades and results from `data.pascal.trade` (README, "Pascal endpoints and the builder code"); the desk speaks Pascal's schema through `pascal-rs` |
| By a team based in Italy | **You** | State it in the form and in the README's Team section |
| A public repo | **You** | The repository has no remote yet. Create a public repo, push the branch you submit, and put its URL in the README and the form |
| A demo video of three minutes or less | **You** | Record from [`docs/video-script.md`](video-script.md) (≤ 3 min, opens on the Sportsbook with live values), upload it, and replace "Video: _link to be added_" in the README |
| A builder code if it places orders | **You: confirm** | Schedina places **no** order on Pascal (mode M), but every hedge permit it signs carries `builder_info {builder_pubkey, builder_fee_rate: "0.001000"}` ([`docs/demo.md`](demo.md#signed-not-sent)). The builder key is a throwaway per run, because no builder account is funded (the skill file asks for $100 of free collateral in the builder wallet). The skill file also asks to "show the fee in the demo": the video's 2:40 beat shows Account's "Builder revenue", computed by the mock. Ask the organisers whether this satisfies the rule for a signed-not-sent build, or fund the accounts and run mode R (`git show cb85bed:docs/live.md`, "What would change to go live") |

**The question to send the organisers**, ready to paste: "Our project, Schedina (BetFi), hedges
each bet with IOC orders built and signed for Pascal with a builder code
(`builder_fee_rate` 0.001), but we have not sent them: Pascal's Terms list Italy as restricted
and we hold no mainnet USDC to fund the hedger and builder accounts, so a local mock fills the
signed orders against Pascal's live books. The signed permits and a verifier are in
[`docs/demo.md`](demo.md#signed-not-sent). Does that meet the builder-code requirement, or do you need a funded builder
wallet and a real fill?"

## The form (https://it.superteam.fun/vitruvium#submit)

One person per team fills it. Everything below is **You**.

| Field | Suggested answer |
| --- | --- |
| Name and email | the submitter's |
| Project name | **Schedina** (the same as the README's title; the app is shown as BetFi, D-433) |
| Edition attended | Turin, Milan, Rome or Naples: to be filled in (the spec assumed Naples; confirm) |
| Public repo | the URL from above |
| Demo video or live demo | the uploaded video URL |
| One sentence | "Schedina (shown in the app as BetFi) is a mobile parlay bookmaker on Pascal's NBA and soccer markets: prices from Pascal's live books with our margin shown in euros, every ticket fully collateralised on Solana, the house's risk hedged on Pascal with a builder code, and tipsters whose bets others copy in one tap." |
| What it does on Pascal, and why a trader would use it | It reads Pascal's markets, books, trades and results, prices multi-game tickets (NBA winner, spread and total; soccer 1-X-2) from the books, and hedges each ticket with signed IOC orders carrying a builder code (filled by a mock until a Pascal account is funded). A bettor gets parlays, which Pascal does not offer, without an order book or trading words, with the margin visible and the payout locked on chain; the event page shows what Pascal's traders actually hold on each side. Tipsters get a record built only from settled, staked tips and 80% of a 2% copy fee, paid on chain |

## Before you submit (skill file, item by item)

| Item | Status | How to check |
| --- | --- | --- |
| Public repo; README's first paragraph names what it does on Pascal | Done for the README; **You** for the repo | The README's first paragraph names the Pascal markets, what is real and what is mocked |
| No keys in the repo: `git log -p \| grep -i private` is empty; `.env.example` has empty values | **You: run it on the pushed branch** (see below) | `.env.example` has no non-empty value (checked 30 Oct). `.env`, `keys/` and `*-keypair.json` are ignored by version control |
| Every endpoint in the README exists on docs.pascal.trade | Done | Every Pascal endpoint the README lists is in the skill file's API tables (which come from the docs) and is called by the code (`crates/pascal-rs`, `crates/pascal-mock/src/live.rs`, `scripts/record_tape.py`). L7 added no Pascal endpoint: soccer comes from the same `GET /api/v1/markets` |
| Video under three minutes, opens on the product | **You** | [`docs/video-script.md`](video-script.md) opens at 0:00 on the BetFi Sportsbook with live values; the test-money line is on Account and every celebration |
| Builder wallet and fee rate stated if orders are placed | Done, with the caveat above | README "Pascal endpoints and the builder code": fee rate 0.001000; no funded builder wallet (mode M) |
| One person submitted the form, same project name as the README | **You** | "Schedina" |

## The secret scan

Run this on the exact branch you push, from its checkout:

```sh
git log -p | grep -iE 'private|secret|api-key'
```

The output is **not empty**, and that is expected: the words occur in code and docs that handle
keys, not in key material. On 30 Oct at `aad2795` (`l7`) it printed 109 lines, all of these
kinds:

- test values: `?api-key=secret`, `?api-key=abc` and `?api-key=sekrit` in the redaction tests;
- code words: `secret key`, `from_secret_str`, `STREAM_SECRETS`, "a private schedina";
- the redaction regex (`api[-_]?key=`), and docs that say the hosted RPC URL is never printed
  (`?api-key=…`).

Then check what must never appear. Each of these should print nothing:

```sh
git log -p | grep -oiE 'api[-_]?key=[A-Za-z0-9_-]+' | grep -viE '=(secret|abc|sekrit)$'   # an RPC URL's real key
git log -p | grep -E '\[ *[0-9]{1,3}( *, *[0-9]{1,3}){63} *\]'                              # a Solana keypair file
git ls-files | grep -E '(^|/)\.env$|keypair\.json$|^keys/'                                   # a key or .env file tracked
```

On 30 Oct at `aad2795` all three printed nothing. The public test seeds of Pascal's docs (owner
`[7; 32]`, trading key `[8; 32]`) and the fixtures' throwaway keys are test material by design
(D-067). If anything else turns up, do not push: rewrite the history or start a fresh repository
from a clean tree. Never publish a build made with `DEMO_APP_HOSTED_RPC=1`: it embeds the hosted
RPC key.

## Before the final push

- [ ] Merge the L7 rehearsal (7.2a-r) and these docs into the branch you publish.
      Done 30 Oct: the L7 rehearsal passed twice in a row (runs 6 and 7, 116 checks and 31 devnet transactions each, live slate) and every signature is in `docs/demo-devnet.md` as of commit `cb85bed`.
- [ ] `just check` and `just e2e` green on that branch (`just check` on `aad2795`: 82 suites,
      496 passed, 0 failed), and update the README's Tests table if the counts change (D-432).
- [ ] README: video link, team, edition and repo URL filled in (the `<!-- 5.C -->` markers), and
      the 7.2a-r placeholders filled from the rehearsal (done 30 Oct).
- [ ] The secret scan above.
- [ ] Stop any old demo desk before recording (one desk only).
- [ ] Optional: file Pascal's written confirmation about Italy
      (D-007) in `docs/`.
