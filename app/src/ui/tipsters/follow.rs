//! Follow / Following (V-5, D-471, D-472): the connected wallet signs the v2 profile message
//! with `follow: {wallet, on}` and sends `PUT /profile` (no transaction, nothing to pay). The
//! desk keeps the follow; `GET /profile/{wallet}` counts followers but does not say whether the
//! viewer is one, so this browser remembers the wallets it followed (`localStorage`, one list per
//! follower wallet) to show "Following" again on the next visit. A wrong memory is harmless: the
//! desk's follow is idempotent both ways (D-510).

use crate::desk::types::{FollowRequest, ProfileUpdateV2};
use crate::desk::{Desk, ProfileWriteError, is_address};
use crate::format;
use crate::wallet::Wallet;

use super::{profile_msg, profile_words};

fn key(me: &str) -> String {
    format!("betfi.follows.v1.{me}")
}

/// The page's `localStorage`, if it has one and allows it (private modes may throw).
fn storage() -> Option<js_sys::Object> {
    let s = js_sys::Reflect::get(&js_sys::global(), &"localStorage".into()).ok()?;
    s.is_object().then(|| js_sys::Object::from(s))
}

fn call(method: &str, args: &[&str]) -> Option<String> {
    let s = storage()?;
    let f = js_sys::Function::from(js_sys::Reflect::get(&s, &method.into()).ok()?);
    let out = match args {
        [a] => f.call1(&s, &(*a).into()),
        [a, b] => f.call2(&s, &(*a).into(), &(*b).into()),
        _ => return None,
    };
    out.ok()?.as_string()
}

/// Wallets `me` followed from this browser (base58 only: anything else stored is ignored).
fn remembered(me: &str) -> Vec<String> {
    call("getItem", &[&key(me)])
        .unwrap_or_default()
        .split(',')
        .filter(|w| is_address(w))
        .map(str::to_string)
        .collect()
}

/// Whether `me` followed `wallet` from this browser.
pub fn is_following(me: &str, wallet: &str) -> bool {
    remembered(me).iter().any(|w| w == wallet)
}

fn remember(me: &str, wallet: &str, on: bool) {
    let mut list = remembered(me);
    list.retain(|w| w != wallet);
    if on {
        list.push(wallet.to_string());
    }
    let _ = call("setItem", &[&key(me), &list.join(",")]);
}

/// Follow (`on`) or unfollow `tipster` as the connected wallet (connecting it first). Returns
/// the follower's wallet on success; the error is in words.
pub async fn set_follow(
    desk: Desk,
    wallet: Wallet,
    tipster: String,
    on: bool,
) -> Result<String, String> {
    if desk.is_demo() {
        return Err(profile_words(&ProfileWriteError::Demo));
    }
    if !is_address(&tipster) {
        return Err("This is not a wallet address.".into());
    }
    let pk = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| format!("{e}."))?,
    };
    let me = pk.to_string();
    if me == tipster {
        return Err("You cannot follow yourself.".into());
    }
    let ts = (js_sys::Date::now() / 1000.0) as i64;
    // Follow-only: no handle needed (D-472), so none is sent and none is changed.
    let msg = profile_msg::message_v2(&me, "", None, None, None, Some((&tipster, on)), ts);
    let sig = wallet
        .sign_message(msg.as_bytes())
        .await
        .map_err(|e| format!("Not saved: {e}."))?;
    let body = ProfileUpdateV2 {
        wallet: me.clone(),
        handle: String::new(),
        title: None,
        ticket: None,
        ts,
        sig: profile_msg::base64(&sig),
        links: None,
        follow: Some(FollowRequest {
            wallet: tipster.clone(),
            on,
        }),
    };
    desk.put_profile_v2(&body)
        .await
        .map_err(|e| profile_words(&e))?;
    remember(&me, &tipster, on);
    Ok(me)
}

/// "12 followers", "1 follower".
pub fn followers_words(n: u64) -> String {
    match n {
        1 => "1 follower".into(),
        n => format!(
            "{} followers",
            format::eur(n.saturating_mul(format::MICRO))
                .trim_start_matches('€')
                .trim_end_matches(".00")
        ),
    }
}
