//! Test support (6.0b): a replay tape with spread and total markets.
//!
//! The 28 Oct tape records moneylines only (l6-contracts L6-3), so spreads and totals reach the
//! desk in live mode or through a tape built here: a copy of a tape directory whose
//! `markets_selected.json` also lists the extra markets (real 28 Oct listings, e.g.
//! `fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json`) and whose every `/books`
//! snapshot also holds a book for each of them, two levels a cent apart around its listed best
//! bid and ask.
//! Used by `e2e-user synth-tape` (the lifecycle's ticket C) and the in-process quoter tests.
//!
//! 7.0b: [`write_tape_with`] can also add one soccer match (its three 1-X-2 outcome markets) from
//! a `/markets` body such as `fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json`. The real
//! matches are days after the tape's night, so the match is moved onto it ([`soccer_on_night`]).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

/// Contracts at each synthetic book level.
pub const LEVEL_SIZE: u64 = 20_000;

/// Writes into `out` (created) a copy of the tape at `from` (a night directory, or its `raw/` or
/// `sample/`) whose catalogue and books also carry the spread and total markets of `lines` (a
/// `/markets` body). Moneylines in `lines` are left out, so the tape's own stay as recorded.
/// Returns the symbols added.
pub fn write_tape(from: &Path, lines: &Path, out: &Path) -> Result<Vec<String>, String> {
    write_tape_with(from, lines, None, out)
}

/// [`write_tape`], plus, with `soccer` (a `/markets` body), the first EPL match listed there
/// with all three outcome markets, moved onto the tape's night by [`soccer_on_night`]. Returns
/// the symbols added.
pub fn write_tape_with(
    from: &Path,
    lines: &Path,
    soccer: Option<&Path>,
    out: &Path,
) -> Result<Vec<String>, String> {
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let parse = |p: &Path, text: &str| {
        serde_json::from_str::<Value>(text).map_err(|e| format!("{}: {e}", p.display()))
    };
    let files = [from.to_path_buf(), from.join("raw"), from.join("sample")]
        .into_iter()
        .find(|d| d.join("books.jsonl").is_file())
        .ok_or_else(|| format!("no books.jsonl under {}", from.display()))?;
    let selected = [
        from.join("markets_selected.json"),
        files.join("markets_selected.json"),
    ]
    .into_iter()
    .chain(files.parent().map(|p| p.join("markets_selected.json")))
    .find(|p| p.is_file())
    .ok_or_else(|| format!("no markets_selected.json for {}", from.display()))?;

    let mut extra: Vec<Value> = parse(lines, &read(lines)?)?["data"]
        .as_array()
        .ok_or_else(|| format!("{}: no data array", lines.display()))?
        .iter()
        .filter(|m| {
            matches!(
                m["display_attributes"]["market_type"]["market"]["type"].as_str(),
                Some("spread" | "total_points")
            )
        })
        .cloned()
        .collect();
    let mut sel = parse(&selected, &read(&selected)?)?;
    if let Some(p) = soccer {
        let body = parse(p, &read(p)?)?;
        let night = sel["data"]
            .as_array()
            .ok_or("markets_selected.json: no data array")?;
        extra.extend(soccer_on_night(&body, night).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    let mut books = serde_json::Map::new();
    for m in &extra {
        let symbol = m["symbol"].as_str().unwrap_or_default().to_owned();
        books.insert(symbol, book_of(m)?);
    }

    let out_files = out.join("sample");
    fs::create_dir_all(&out_files).map_err(|e| format!("{}: {e}", out_files.display()))?;
    sel["data"]
        .as_array_mut()
        .ok_or("markets_selected.json: no data array")?
        .extend(extra.iter().cloned());
    write(&out.join("markets_selected.json"), &format!("{sel}\n"))?;

    let mut text = String::new();
    for line in read(&files.join("books.jsonl"))?.lines() {
        let mut v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // A truncated line stays as it was (the loader skips it).
            Err(_) => {
                text.push_str(line);
                text.push('\n');
                continue;
            }
        };
        if let Some(b) = v["body"]["data"]["books"].as_object_mut() {
            b.extend(books.clone());
        }
        text.push_str(&v.to_string());
        text.push('\n');
    }
    write(&out_files.join("books.jsonl"), &text)?;
    for name in ["trades.jsonl", "markets.jsonl", "search.jsonl"] {
        let p = files.join(name);
        if p.is_file() {
            write(&out_files.join(name), &read(&p)?)?;
        }
    }
    let script = [from.join("script.toml"), files.join("../script.toml")]
        .into_iter()
        .find(|p| p.is_file());
    if let Some(p) = script {
        write(&out.join("script.toml"), &read(&p)?)?;
    }
    Ok(books.keys().cloned().collect())
}

/// The first EPL match of `body` (a `/markets` body) listing all three moneyline outcomes,
/// moved onto the night of `night` (a tape's selected markets), for a soccer leg on a replay
/// tape (7.0b). Every date token of the match (`26OCT31` in symbols and rule URLs, `2026-10-31`
/// in game ids and slugs) becomes the night's (from its first moneyline), its start becomes the
/// night's latest start and its expected resolution [`pricing::SOCCER_GAME_LENGTH_MS`] later.
/// Only the three outcome markets are returned; everything else in them is as listed.
pub fn soccer_on_night(body: &Value, night: &[Value]) -> Result<Vec<Value>, String> {
    let data = body["data"].as_array().ok_or("no data array")?;
    let mut by_event: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for m in data {
        let symbol = m["symbol"].as_str().unwrap_or_default();
        if symbol.starts_with("EPL_")
            && m["display_attributes"]["market_type"]["market"]["type"] == "moneyline"
            && let Some((event, _)) = symbol.split_once('.')
        {
            by_event.entry(event).or_default().push(m);
        }
    }
    let (event, outcomes) = by_event
        .into_iter()
        .find(|(_, v)| v.len() == 3)
        .ok_or("no EPL match with all three outcomes")?;
    let tokens = |m: &Value| -> Option<(String, String)> {
        let symbol = m["symbol"].as_str()?;
        let game_id = m["display_attributes"]["game_page"]["game_id"].as_str()?;
        Some((
            symbol.split('_').nth(1)?.to_owned(),
            super::parse::game_date(game_id)?.to_owned(),
        ))
    };
    let (from_tok, from_date) =
        tokens(outcomes[0]).ok_or_else(|| format!("{event}: no date in the symbol or game id"))?;
    let (to_tok, to_date) = night
        .iter()
        .filter(|m| m["symbol"].as_str().is_some_and(|s| s.contains("_ML.")))
        .find_map(tokens)
        .ok_or("the tape lists no moneyline with a dated symbol and game id")?;
    let start = night
        .iter()
        .filter_map(|m| {
            m["display_attributes"]["expected_event_start_time_ms"]
                .as_str()?
                .parse::<u64>()
                .ok()
        })
        .max()
        .ok_or("the tape lists no start time")?;
    outcomes
        .into_iter()
        .map(|m| {
            let text = m
                .to_string()
                .replace(&from_tok, &to_tok)
                .replace(&from_date, &to_date);
            let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let da = &mut v["display_attributes"];
            da["expected_event_start_time_ms"] = json!(start.to_string());
            da["expected_resolution_time_ms"] =
                json!((start + pricing::SOCCER_GAME_LENGTH_MS).to_string());
            Ok(v)
        })
        .collect()
}

fn write(p: &Path, text: &str) -> Result<(), String> {
    fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))
}

/// A two-level book around the market's listed best bid and ask.
fn book_of(m: &Value) -> Result<Value, String> {
    let price = |k: &str| {
        m[k].as_str()
            .and_then(|s| pascal_rs::parse_micros(s).ok())
            .filter(|p| *p > 10_000 && *p < 990_000)
            .ok_or_else(|| format!("{}: no usable {k}", m["symbol"]))
    };
    let (bid, ask) = (price("best_bid")?, price("best_ask")?);
    let lv = |p: i64| json!([pascal_rs::format_micros(p), LEVEL_SIZE.to_string()]);
    Ok(json!({
        "asks": [lv(ask), lv(ask + 10_000)],
        "bids": [lv(bid), lv(bid - 10_000)],
    }))
}
