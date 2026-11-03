//! The Privy signer's transaction rules on wire bytes (8.D, L8-3), dependency-free so `just
//! app-check` tests it on the host (`rustc --test src/privy/wire.rs`).
//!
//! A transaction is `shortvec(n) ‖ n × 64-byte signatures ‖ message`. The message starts with
//! the header (`num_required_signatures`, …; a v0 message is prefixed by `0x80`), then
//! `shortvec(k) ‖ k × 32-byte account keys`; the first `num_required_signatures` keys sign, in
//! slot order. In L8 the desk's sponsor is often the fee payer (slot 0, left zeroed for the desk
//! to fill) and the user signs slot 1, so the user's wallet must fill its own slot and nothing
//! else: [`merge_signer_signature`] takes from what Privy returned only that slot, and only if
//! the message is byte-for-byte the one sent.

/// Where a serialized transaction's parts sit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Offset of the first signature.
    pub sigs_start: usize,
    /// Number of signature slots.
    pub num_sigs: usize,
    /// Offset of the message (to the end of the bytes).
    pub message_start: usize,
    /// The header's `num_required_signatures`.
    pub required: usize,
    /// The message's static account keys.
    pub keys: Vec<[u8; 32]>,
}

/// Decode a compact-u16 ("shortvec") length at `pos`: (value, bytes used).
fn shortvec(bytes: &[u8], pos: usize) -> Result<(usize, usize), String> {
    let mut value = 0usize;
    for i in 0..3 {
        let b = *bytes.get(pos + i).ok_or("truncated length")?;
        value |= usize::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err("length too long".into())
}

/// Parse a serialized legacy or v0 transaction far enough to find its signers.
pub fn parse(tx: &[u8]) -> Result<Layout, String> {
    let (num_sigs, used) = shortvec(tx, 0)?;
    let sigs_start = used;
    let message_start = sigs_start + num_sigs * 64;
    let mut pos = message_start;
    let first = *tx.get(pos).ok_or("truncated transaction")?;
    if first & 0x80 != 0 {
        if first & 0x7f != 0 {
            return Err(format!("unsupported message version {}", first & 0x7f));
        }
        pos += 1;
    }
    let header = tx.get(pos..pos + 3).ok_or("truncated message header")?;
    let required = usize::from(header[0]);
    pos += 3;
    let (num_keys, used) = shortvec(tx, pos)?;
    pos += used;
    let keys_bytes = tx
        .get(pos..pos + num_keys * 32)
        .ok_or("truncated account keys")?;
    let keys = keys_bytes.as_chunks::<32>().0.to_vec();
    if num_sigs != required {
        return Err(format!("{num_sigs} signature slots for {required} signers"));
    }
    if num_keys < required {
        return Err("fewer account keys than signers".into());
    }
    Ok(Layout {
        sigs_start,
        num_sigs,
        message_start,
        required,
        keys,
    })
}

/// The slot `signer` signs in: its position among the required signers.
pub fn signer_slot(layout: &Layout, signer: &[u8; 32]) -> Result<usize, String> {
    layout
        .keys
        .iter()
        .take(layout.required)
        .position(|k| k == signer)
        .ok_or_else(|| "this wallet is not a signer of the transaction".to_string())
}

/// The message bytes (what an ed25519 signature covers).
pub fn message<'a>(tx: &'a [u8], layout: &Layout) -> &'a [u8] {
    &tx[layout.message_start..]
}

/// `original` with only `signer`'s slot replaced by the signature found in that slot of
/// `signed` (what the wallet returned). Refused when the wallet changed the message, the slot
/// count, or left the slot empty. Every other slot of `original` is kept as it was.
pub fn merge_signer_signature(
    original: &[u8],
    signed: &[u8],
    signer: &[u8; 32],
) -> Result<(Vec<u8>, usize), String> {
    let a = parse(original)?;
    let b = parse(signed)?;
    if a.num_sigs != b.num_sigs || message(original, &a) != message(signed, &b) {
        return Err("the wallet changed the transaction".into());
    }
    let slot = signer_slot(&a, signer)?;
    let from = b.sigs_start + slot * 64;
    let sig = &signed[from..from + 64];
    if sig.iter().all(|&x| x == 0) {
        return Err("the wallet did not sign".into());
    }
    let mut out = original.to_vec();
    let to = a.sigs_start + slot * 64;
    out[to..to + 64].copy_from_slice(sig);
    Ok((out, slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPONSOR: [u8; 32] = [7; 32];
    const USER: [u8; 32] = [9; 32];
    const PROGRAM: [u8; 32] = [3; 32];

    /// A transaction with `signers` as the required signers (slots zeroed), one read-only
    /// program key, and a tiny body; `v0` adds the version prefix and an empty lookup list.
    fn tx(signers: &[[u8; 32]], v0: bool) -> Vec<u8> {
        let mut t = vec![signers.len() as u8];
        t.extend(std::iter::repeat_n(0u8, 64 * signers.len()));
        if v0 {
            t.push(0x80);
        }
        t.extend([signers.len() as u8, 0, 1]);
        t.push((signers.len() + 1) as u8);
        for s in signers {
            t.extend(s);
        }
        t.extend(PROGRAM);
        t.extend([0xAB; 32]); // blockhash
        t.extend([1, signers.len() as u8, 0, 2, 0xde, 0xad]); // one instruction
        if v0 {
            t.push(0);
        }
        t
    }

    fn slot_bytes(t: &[u8], slot: usize) -> &[u8] {
        &t[1 + slot * 64..1 + (slot + 1) * 64]
    }

    #[test]
    fn parses_legacy_and_v0() {
        for v0 in [false, true] {
            let t = tx(&[SPONSOR, USER], v0);
            let l = parse(&t).unwrap();
            assert_eq!((l.sigs_start, l.num_sigs, l.required), (1, 2, 2));
            assert_eq!(l.keys, vec![SPONSOR, USER, PROGRAM]);
            assert_eq!(signer_slot(&l, &USER), Ok(1));
            assert_eq!(signer_slot(&l, &SPONSOR), Ok(0));
            assert!(
                signer_slot(&l, &PROGRAM).is_err(),
                "a read-only key is not a signer"
            );
        }
    }

    #[test]
    fn sponsored_user_fills_only_slot_one() {
        let original = tx(&[SPONSOR, USER], true);
        // The wallet signs slot 1 and (misbehaving) writes junk into the sponsor's slot 0.
        let mut signed = original.clone();
        signed[1..65].fill(0xEE);
        signed[65..129].fill(0x55);
        let (merged, slot) = merge_signer_signature(&original, &signed, &USER).unwrap();
        assert_eq!(slot, 1);
        assert!(
            slot_bytes(&merged, 0).iter().all(|&b| b == 0),
            "sponsor slot untouched"
        );
        assert!(slot_bytes(&merged, 1).iter().all(|&b| b == 0x55));
        assert_eq!(merged[129..], original[129..], "message unchanged");
    }

    #[test]
    fn keeps_a_signature_already_in_another_slot() {
        let mut original = tx(&[SPONSOR, USER], false);
        original[1..65].fill(0x11); // the sponsor signed first
        let mut signed = original.clone();
        signed[1..65].fill(0);
        signed[65..129].fill(0x22);
        let (merged, _) = merge_signer_signature(&original, &signed, &USER).unwrap();
        assert!(slot_bytes(&merged, 0).iter().all(|&b| b == 0x11));
        assert!(slot_bytes(&merged, 1).iter().all(|&b| b == 0x22));
    }

    #[test]
    fn single_signer_slot_zero() {
        let original = tx(&[USER], false);
        let mut signed = original.clone();
        signed[1..65].fill(0x33);
        let (merged, slot) = merge_signer_signature(&original, &signed, &USER).unwrap();
        assert_eq!(slot, 0);
        assert_eq!(merged, signed);
    }

    #[test]
    fn refuses_a_changed_message() {
        let original = tx(&[SPONSOR, USER], true);
        let mut signed = original.clone();
        signed[65..129].fill(0x55);
        let last = signed.len() - 2;
        signed[last] ^= 1;
        assert!(merge_signer_signature(&original, &signed, &USER).is_err());
    }

    #[test]
    fn refuses_an_unsigned_slot_and_a_stranger() {
        let original = tx(&[SPONSOR, USER], true);
        assert_eq!(
            merge_signer_signature(&original, &original, &USER),
            Err("the wallet did not sign".into())
        );
        let mut signed = original.clone();
        signed[65..129].fill(0x55);
        assert!(merge_signer_signature(&original, &signed, &[1; 32]).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[2, 0, 0]).is_err());
        let mut t = tx(&[USER], false);
        t.truncate(70);
        assert!(parse(&t).is_err());
        // Slot count disagreeing with the header.
        let mut t = tx(&[USER], false);
        t[0] = 2;
        t.splice(1..1, [0u8; 64]);
        assert!(parse(&t).is_err());
    }

    #[test]
    fn multi_byte_shortvec() {
        assert_eq!(shortvec(&[0x80, 0x01], 0), Ok((128, 2)));
        assert_eq!(shortvec(&[0x7f], 0), Ok((127, 1)));
        assert!(shortvec(&[0x80], 0).is_err());
    }
}
