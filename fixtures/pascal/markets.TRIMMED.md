# rest_markets.json is trimmed

`rest_markets.json` is the verbatim `GET https://data.pascal.trade/api/v1/markets` envelope
captured at 2026-10-27 23:54:10 UTC (`seq` 12110239487, `state_time_ms` 1793145250512) with
**`data` cut from 3,097 markets to 50**. Nothing else was changed: the envelope fields, the
order of the kept markets (Pascal's own order, alphabetical by symbol) and every field of
every kept market object are exactly as served. The full 5.5 MB body is not committed; the
live endpoint is rate-limited (a second call within seconds returns 429).

The 50 markets were chosen by `scripts/record_fixtures.py` from the list in its config to
cover the shapes the client and the topic mapper must handle:

| Shape | Examples in the file |
| --- | --- |
| Politics and geopolitics, mirroring Polymarket (`reference.kind = "polymarket"`) | `TX_GOV_2026.REP`, `TX_SEN_2026.DEM`, `MID_2026_BAL_PWR.DS_DH`, `US_IRAN_CFR_THRU.26OCT31`, `FRANCE_PRES_2027.GLUCKSMANN`, `NATO_RUS_CLASH_BY.26DEC31` |
| Fed rates (the largest MAC topic by 30-day notional) | `FED_DECISION_26OCT.NO_CHANGE` |
| Pascal-native (no `reference`) | `REP_HOUSE_SEATS_2026_RNG.180_230`, `EPL_2027_PTS_DED.MCI`, `ANTH_IPO_MC_27DEC31_RNG.1_1P5T`, `KALSHI_IPO_BY.26DEC31`, `POLYMARKET_TOKEN_BY.27DEC31`, `POLY_NEW_CLOB_BY.26DEC31`, `US_JOBS_26OCT_SCL.M25_100K` |
| Sports with `display_attributes.market_type` (moneyline, spread, team total, map winner) | `NBA_26OCT27_BKNBOS_TT.219`, `NHL_26OCT27_NJDDAL_ML.NJD`, `NHL_26OCT27_MINUTA_SP.UTA_1`, `NFL_26NOV01_BALBUF_ML.BAL`, `MLB_26OCT27_MILTB_ML.MIL`, `CFB_26OCT30_BAYUCF_ML.BAY`, `LALIGA_26OCT30_VILESP_ML.DRAW`, `BUND_26OCT30_ELVM05_ML.DRAW`, `CS2_26OCT28_MGLZNEM_M1ML.MGLZ`, `LOL_26OCT28_IGLYON_M1ML.IG`, `DOTA_26OCT28_GLLEG_M1ML.GL` |
| Sports futures (no `market_type`) | `EPL_2027_CHAMP.ARS`, `NBA_2027_CHAMPION.MIA`, `F1_2026_CHAMP.HAM`, `MLB_2026_WS.TB`, `UCL_2027_CHAMP.PSG`, `NFL_2027_CHAMP.CHI`, `INFANTINO_OUT_BY.26DEC31` |
| Macro / finance / AI / crypto | `ANTHROPIC_IPO.26DEC31`, `WTI_PRICE_26OCT.REACH_110`, `LARGESTCO_26DEC31.NVDA`, `CLARITY_ACT_SIGNED_BY.26DEC31`, `HYPERLIQUID_AIRDROP_BY.26DEC31`, `AI_MILLENNIUM_COUNT_2026.0` |
| No `topic` at all (tag fallback) | `AI_BEST_MODEL_26OCT31.ANTHROPIC`, `AI_BEST_MODEL_26DEC31.GOOGLE` |
| Both China-Taiwan topic spellings (en dash and hyphen) | `CHN_INVADES_TWN_BY.26DEC31`, `CHN_TWN_CLASH_BY.26DEC31` |
| `scalar` + `range` | `REP_HOUSE_SEATS_2026_RNG.180_230`, `ANTH_IPO_MC_27DEC31_RNG.1_1P5T` (`US_JOBS_26OCT_SCL.M25_100K` has `scalar` only) |
| `outcome_labels`, `reverse_description`, `game_page`, `colors`, `expected_event_start_time_ms` | `CFB_26OCT30_BAYUCF_ML.BAY`, `DOTA_26OCT28_GLLEG_M1ML.GL`, `CS2_26OCT28_MGLZNEM_M1ML.MGLZ`, `BUND_26OCT30_ELVM05_ML.DRAW` |
| Missing optional top-level fields | `DOTA_26OCT28_TYAI_M2ML.TY` (no `mark_price`), `F1_2026_CHAMP.HAM` and `KALSHI_IPO_BY.26DEC31` (no `best_bid`), `TX_SEN_2026.DEM`, `MID_2026_BAL_PWR.DS_DH`, `REP_HOUSE_SEATS_2026_RNG.180_230`, `ANTH_IPO_MC_27DEC31_RNG.1_1P5T` (no `listing_ts_ms`) |
| Topic bucketed OTH | `HOTTEST_YEAR_2026.RANK1` |
