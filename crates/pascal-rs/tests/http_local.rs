//! REST behaviour against a local HTTP server: pagination resumes after a failure with the
//! items already collected, and a base URL path prefix is kept.

use std::sync::{Arc, Mutex};

use pascal_rs::ratelimit::{Backoff, RetryPolicy};
use pascal_rs::{Client, PartialError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Requests = Arc<Mutex<Vec<String>>>;

/// Serves one request per connection; `handler(target)` returns (status, body).
async fn serve(
    handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> (String, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests: Requests = Arc::default();
    let log = requests.clone();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (log, handler) = (log.clone(), handler.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf);
                let target = head.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock().unwrap().push(target.clone());
                let (status, body) = handler(&target);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.shutdown().await.ok();
            });
        }
    });
    (format!("http://{addr}"), requests)
}

fn trade(id: u64) -> String {
    format!(
        r#"{{"trade_id":"{id}","symbol":"A.B","taker_side":"BID","price":"0.500000","size":"1","maker":"M","taker":"T","seq":"{id}","trade_ts_ms":"{id}"}}"#
    )
}

fn page(ids: &[u64], cursor: Option<&str>) -> String {
    let items: Vec<String> = ids.iter().map(|&i| trade(i)).collect();
    let cursor = cursor.map_or(String::new(), |c| format!(r#","next_cursor":"{c}""#));
    format!(
        r#"{{"status":"success","data":{{"items":[{}]{cursor}}},"seq":"100","state_time_ms":"1","server_time_ms":"2"}}"#,
        items.join(",")
    )
}

#[tokio::test]
async fn collect_while_returns_partial_items_and_resumes() {
    let page2_calls = Arc::new(Mutex::new(0));
    let calls = page2_calls.clone();
    let (base, requests) = serve(move |target| {
        if !target.contains("before_cursor") {
            return (200, page(&[30, 29], Some("c1")));
        }
        let mut n = calls.lock().unwrap();
        *n += 1;
        if *n == 1 {
            // Non-retryable, so the client returns it immediately.
            (
                422,
                r#"{"status":"error","data":{"code":"INVALID_REQUEST","message":"boom"}}"#.into(),
            )
        } else {
            (200, page(&[28], None))
        }
    })
    .await;
    let client = Client::builder()
        .base_url(format!("{base}/pfx/"))
        .retry(RetryPolicy {
            max_attempts: 1,
            backoff: Backoff {
                base: std::time::Duration::from_millis(1),
                max: std::time::Duration::from_millis(1),
            },
        })
        .build()
        .unwrap();
    let mut pager = client.trades_pager(&["A.B"], Some(2)).unwrap();

    let err: PartialError<_> = pager.collect_while(|_| true, 10).await.unwrap_err();
    assert_eq!(
        err.items.iter().map(|t| t.trade_id).collect::<Vec<_>>(),
        [30, 29]
    );
    assert_eq!(err.error.api_code(), Some("INVALID_REQUEST"));
    assert_eq!(pager.cursor(), Some("c1"), "pager stays on the failed page");
    assert!(!pager.is_done());

    let rest = pager.collect_while(|_| true, 10).await.unwrap();
    assert_eq!(rest.iter().map(|t| t.trade_id).collect::<Vec<_>>(), [28]);
    assert!(pager.is_done());
    assert_eq!(pager.first_seq(), Some(100));

    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|r| r.starts_with("/pfx/api/v1/trades?")),
        "{requests:?}"
    );
    assert!(requests[1].contains("before_cursor=c1") && requests[2].contains("before_cursor=c1"));

    // `?` still works: a PartialError converts into Error.
    fn uses_question_mark(r: Result<Vec<u8>, PartialError<u8>>) -> pascal_rs::Result<Vec<u8>> {
        Ok(r?)
    }
    assert!(uses_question_mark(Ok(vec![1])).is_ok());
}

#[tokio::test]
async fn search_lenient_keeps_good_markets() {
    // The recorded search response, with one market given a sport this crate does not know.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pascal/rest_markets_search.json");
    let mut raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    raw["data"][1]["display_attributes"]["market_type"] =
        serde_json::json!({"kind": "sports", "sport": "curling"});
    let body = raw.to_string();
    let (base, requests) = serve(move |_| (200, body.clone())).await;
    let client = Client::builder().base_url(base).build().unwrap();
    let q = pascal_rs::MarketSearch {
        symbols: vec!["TX_GOV_2026.REP".into()],
        ..Default::default()
    };
    assert!(matches!(
        client.search_markets(&q).await,
        Err(pascal_rs::Error::Decode { .. })
    ));
    let r = client.search_markets_lenient(&q).await.unwrap();
    assert_eq!(r.data.items.len(), 2);
    assert_eq!(r.data.failures.len(), 1);
    assert_eq!(r.data.failures[0].index, 1);
    assert_eq!(r.data.items[1].0, 2);
    assert!(
        client
            .search_markets_lenient(&pascal_rs::MarketSearch::default())
            .await
            .is_err()
    );
    let requests = requests.lock().unwrap().clone();
    assert_eq!(
        requests,
        ["/api/v1/markets/search", "/api/v1/markets/search"]
    );
}
