# Spread and total listings (6.0b)

`markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json` is the real `GET /markets` body of
`data.pascal.trade` at 28 Oct 2026 18:43 UTC (`tapes/nba-2026-10-28/raw/markets_full.json.gz`,
not committed), cut to the 13 markets of PHI @ IND and the 10 of ORL @ TOR (moneyline, spreads,
totals). The only edits: the other 3,602 markets removed, and each market's `stats` and
`display_attributes.colors` dropped; the envelope (`seq`, `state_time_ms`, `server_time_ms`) and
every other key are as served.

Used by `crates/pricing/tests/lines.rs`, `crates/desk/tests/catalogue_lines.rs`,
`crates/desk/tests/quoter_lines.rs` and `e2e-user synth-tape` (`just e2e`, ticket C), which adds
these spreads and totals, with books, to a copy of the 28 Oct tape.
