//! `fixtures/desk/*.json` round-trip through the desk's own C4 types: each file deserialises, and
//! serialising the value back gives the same JSON (so a fixture can neither carry a field the
//! type lacks nor miss one the type has). Every file in the directory must be listed here.

mod common;

use std::collections::BTreeSet;

use base64::Engine;
use common::repo_root;
use desk::api::types::{
    CardResponse, CashoutQuoteResponse, ClientMessage, ErrorBody, FxResponse, HouseResponse,
    ProfileResponse, QuoteRequest, QuoteResponse, StreamFrame, Ticket, TicketStatus,
    TicketsResponse, TipsterResponse, TipstersResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = repo_root().join("fixtures/desk").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str) -> T {
    let v = load(name);
    let typed: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        v,
        "{name} does not round-trip"
    );
    typed
}

const FILES: [&str; 25] = [
    "card.json",
    "card_lines.json",
    "card_soccer.json",
    "cashout_quote.json",
    "error_rejected.json",
    "error_unavailable.json",
    "fx.json",
    "house.json",
    "profile.json",
    "quote.json",
    "quote_request.json",
    "sentiment.json",
    "stream_feed.json",
    "stream_feed_copy.json",
    "stream_heartbeat.json",
    "stream_progress_escrowed.json",
    "stream_progress_hedging.json",
    "stream_progress_minted.json",
    "stream_progress_refunded.json",
    "stream_subscribe.json",
    "stream_ticket.json",
    "ticket.json",
    "tickets.json",
    "tipster.json",
    "tipsters.json",
];

#[test]
fn every_fixture_round_trips() {
    let listed: BTreeSet<&str> = FILES.into_iter().collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(repo_root().join("fixtures/desk"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        on_disk,
        listed.iter().map(|s| (*s).to_owned()).collect(),
        "fixtures/desk and this test disagree"
    );

    round_trip::<CardResponse>("card.json");
    // l6-contracts L6-2 (6.0b); tests/catalogue_lines.rs checks its content.
    round_trip::<CardResponse>("card_lines.json");
    // l7-contracts L7-1 (7.0b); tests/catalogue_soccer.rs checks its content.
    round_trip::<CardResponse>("card_soccer.json");
    round_trip::<QuoteRequest>("quote_request.json");
    let quote: QuoteResponse = round_trip("quote.json");
    let ticket: Ticket = round_trip("ticket.json");
    let tickets: TicketsResponse = round_trip("tickets.json");
    let cashout: CashoutQuoteResponse = round_trip("cashout_quote.json");
    round_trip::<ErrorBody>("error_rejected.json");
    round_trip::<ErrorBody>("error_unavailable.json");
    round_trip::<ClientMessage>("stream_subscribe.json");
    // l4-contracts L4-2 (4.A); tests/tipsters.rs checks their numbers.
    round_trip::<TipstersResponse>("tipsters.json");
    round_trip::<TipsterResponse>("tipster.json");
    round_trip::<HouseResponse>("house.json");
    // L5-4 (5.0c); tests/sentiment.rs checks its numbers.
    round_trip::<desk::sentiment::SentimentResponse>("sentiment.json");
    for f in [
        "stream_heartbeat.json",
        "stream_progress_escrowed.json",
        "stream_progress_hedging.json",
        "stream_progress_minted.json",
        "stream_progress_refunded.json",
        "stream_ticket.json",
        "stream_feed.json",
        "stream_feed_copy.json",
    ] {
        round_trip::<StreamFrame>(f);
    }

    // The numbers are C2's arithmetic.
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(b64.decode(&quote.quote_sig).unwrap().len(), 64);
    assert_eq!(
        bs58::decode(&quote.quote_signer).into_vec().unwrap().len(),
        32
    );
    assert_eq!(
        quote.odds_offered_u,
        quote.payout_u * 1_000_000 / quote.stake_u
    );
    assert_eq!(quote.n as usize, quote.legs.len());
    let mut acc = quote.stake_u;
    for l in &quote.legs {
        acc = acc * (1_000_000 - u64::from(l.m1_ppm)) / l.price_u;
    }
    assert_eq!(acc, quote.payout_u);
    assert!(quote.min_payout_per_unit_u <= quote.odds_offered_u);

    let mut acc = ticket.stake_u;
    for l in &ticket.legs {
        acc = acc * (1_000_000 - 50_000) / l.p_fill_u.unwrap();
    }
    assert_eq!(acc, ticket.payout_u);
    assert_eq!(tickets.tickets[0], ticket);
    let settled = &tickets.tickets[1];
    assert_eq!(settled.state, TicketStatus::Settled);
    assert_eq!(settled.paid_u, Some(settled.payout_u));
    assert_eq!(cashout.ticket, ticket.ticket);
    assert_eq!(Some(cashout.c_u), ticket.cash_out.c_u);
}

/// l6-contracts L6-2 (6.0c): the new files and the additive fields hold together.
#[test]
fn six_c_fields_are_consistent() {
    let profile: ProfileResponse = round_trip("profile.json");
    desk::profiles::check_handle(profile.handle.as_deref().unwrap()).unwrap();
    assert!(profile.since > 0);
    let fx: FxResponse = round_trip("fx.json");
    assert_eq!(fx.fallback, fx.source == "fallback");
    assert!(desk::fx::days_from_date(&fx.as_of).is_some());
    assert!((desk::fx::MIN_PPM..=desk::fx::MAX_PPM).contains(&fx.eur_per_usd_ppm));

    // posted_odds_u is the minted odds (0 before mint); titles only on own tips, and clean.
    let tipster: TipsterResponse = round_trip("tipster.json");
    let tickets: TicketsResponse = round_trip("tickets.json");
    let all: Vec<&Ticket> = tipster
        .settled
        .iter()
        .chain(&tipster.open)
        .chain(&tickets.tickets)
        .collect();
    for t in all {
        let minted = matches!(
            t.state,
            TicketStatus::Open | TicketStatus::Settled | TicketStatus::CashedOut
        );
        assert_eq!(t.posted_odds_u, if minted { t.odds_offered_u } else { 0 });
        if let Some(title) = &t.title {
            assert_eq!(&desk::profiles::clean_title(title).unwrap(), title);
            assert_eq!(t.tipster.as_deref(), Some(t.user.as_str()));
            assert!(t.copied_from.is_none());
        }
    }
    assert!(tipster.open.iter().any(|t| t.title.is_some()));

    // verified = on the board; copiers ≤ copies.
    let board: TipstersResponse = round_trip("tipsters.json");
    for r in board.tipsters.iter().chain([&tipster.tipster]) {
        assert_eq!(
            r.verified,
            desk::tipsters::metrics::on_board(r),
            "{}",
            r.wallet
        );
        assert!(r.copiers <= r.copies);
        if let Some(h) = &r.handle {
            desk::profiles::check_handle(h).unwrap();
        }
    }
}

/// l7-contracts L7-1 (7.0c): the additive fields hold together.
#[test]
fn seven_c_fields_are_consistent() {
    use desk::api::types::FeedKind;
    use desk::profiles::{Network, check_link};

    // SOL: EUR per SOL = USD per SOL × EUR per USD, a sane price; a Pyth price has a time.
    let fx: FxResponse = round_trip("fx.json");
    assert_eq!(fx.sol_fallback, fx.sol_source == "fallback");
    assert_eq!(fx.sol_fallback, fx.sol_as_of_ms == 0);
    let usd_u = u128::from(fx.eur_per_sol_ppm) * 1_000_000 / u128::from(fx.eur_per_usd_ppm);
    assert!((1_000_000..100_000_000_000).contains(&usd_u), "{usd_u}");

    // Profile links are bare handles in their stored form; verified = on the board.
    let profile: ProfileResponse = round_trip("profile.json");
    for (net, v) in [
        (Network::X, &profile.links.x),
        (Network::Youtube, &profile.links.youtube),
        (Network::Telegram, &profile.links.telegram),
        (Network::Instagram, &profile.links.instagram),
    ] {
        if let Some(v) = v {
            assert_eq!(&check_link(net, v).unwrap(), v);
        }
    }
    let board: TipstersResponse = round_trip("tipsters.json");
    let row = board
        .tipsters
        .iter()
        .find(|r| r.wallet == profile.wallet)
        .unwrap();
    assert_eq!(profile.verified, row.verified);

    // Tips: one team code per leg; the pools add up to the tipster's copies.
    let tipster: TipsterResponse = round_trip("tipster.json");
    let tips: Vec<&Ticket> = tipster.settled.iter().chain(&tipster.open).collect();
    let mut copies = 0;
    for t in &tips {
        assert_eq!(t.teams.as_ref().unwrap().len(), t.legs.len());
        assert_eq!(t.copies == Some(0), t.pool_u == Some(0));
        copies += t.copies.unwrap();
    }
    assert_eq!(copies, tipster.tipster.copies);

    // A slot only where the transaction's signature is; confirm times only with a slot.
    let tickets: TicketsResponse = round_trip("tickets.json");
    for t in tips.iter().copied().chain(&tickets.tickets) {
        let x = &t.tx;
        assert_eq!(x.slot.open_pending.is_some(), x.open_pending.is_some());
        assert_eq!(x.slot.cash_out.is_some(), x.cash_out.is_some());
        assert_eq!(x.slot.settle.is_some(), x.settle.is_some());
        assert!(x.confirm_ms.open_pending.is_none() || x.slot.open_pending.is_some());
        assert!(x.confirm_ms.cash_out.is_none() || x.slot.cash_out.is_some());
    }

    // The copy ticker line carries who, amount, team and odds.
    let f: StreamFrame = round_trip("stream_feed_copy.json");
    match f {
        StreamFrame::Feed {
            kind,
            who,
            amount_u,
            team,
            odds_u,
            ..
        } => {
            assert_eq!(kind, FeedKind::Copy);
            assert!(who.is_some() && team.is_some());
            assert!(amount_u.unwrap() > 0 && odds_u.unwrap() > 1_000_000);
        }
        other => panic!("{other:?}"),
    }

    // Sentiment: at most 10 recent stakes per game, newest first, from both sources.
    let s: desk::sentiment::SentimentResponse = round_trip("sentiment.json");
    for g in &s.games {
        assert!(g.recent.len() <= desk::sentiment::MAX_RECENT);
        assert!(g.recent.windows(2).all(|w| w[0].ts_ms >= w[1].ts_ms));
        for r in &g.recent {
            assert!(r.source == "pascal" || r.source == "schedina");
            assert!(r.team == "away" || r.team == "home");
        }
        assert!(g.traders >= g.pascal_holders.away.accounts + g.pascal_holders.home.accounts);
    }
}
