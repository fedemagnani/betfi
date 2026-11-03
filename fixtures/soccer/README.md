# Soccer 1-X-2 listings (7.0b)

`markets_2026-10-28_soccer.TRIMMED.json` is the real `GET /markets` body of `data.pascal.trade`
at 28 Oct 2026 18:43 UTC (`tapes/nba-2026-10-28/raw/markets_full.json.gz`, not committed), cut
to 18 markets of five matches on 31 Oct:

- the three moneyline outcomes (`_ML.<HOME>`, `_ML.DRAW`, `_ML.<AWAY>`) of Aston Villa – Fulham,
  Man City – Brighton and Hull – Ipswich (EPL), Bayern – Dortmund (Bundesliga) and
  Barcelona – Alavés (LaLiga);
- Aston Villa – Fulham's both-teams-to-score, one spread and one total goals market, which the
  desk leaves out (D-435).

The only edits: the other markets removed, and each market's `stats` and
`display_attributes.colors` dropped; the envelope (`seq`, `state_time_ms`, `server_time_ms`) and
every other key are as served. The symbols list the home team first (`AVLFUL`: Aston Villa at
home), unlike the NBA's away-first symbols.

Used by `crates/desk/tests/catalogue_soccer.rs`, `crates/desk/tests/quoter_soccer.rs`,
`crates/pricing/tests/soccer.rs` and `e2e-user synth-tape --soccer` (`just e2e`, ticket D), which
moves Aston Villa – Fulham onto the 28 Oct tape's night (symbols `EPL_26OCT28_…`, game id
`epl-ast-ful-2026-10-28`, kick-off at the night's latest start) with books.

`markets_2026-11-03_ucl.TRIMMED.json` (3 Nov): the real `GET /markets` body at 3 Nov 2026 ~10:40
UTC, cut to the 24 markets of two Champions League matches: Atlético Madrid – Bayern Munich
(3 Nov) and AEK Athens – Real Madrid (4 Nov; its symbol's home code `AEK1` is four letters, so the
draw's pair `AEK1RMA` has an odd length). Same edits as above. Used by
`crates/desk/tests/catalogue_soccer.rs` (the Champions League, `UCL_`).
