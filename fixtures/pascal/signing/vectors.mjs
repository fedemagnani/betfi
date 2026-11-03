// Signing test vectors, copied verbatim from Pascal's published sources. Only the public
// test-vector seeds ([7; 32] owner, [8; 32] trading key) appear here; no real key material.
//
// DOCS = https://docs.pascal.trade, section "Request Signing" (fetched 2026-10-28):
//   key table under "All test vectors in this section use the following keys", the
//   "Place Order" test vector + its request example, and the "Cancel Order" / "Mass Cancel"
//   test vectors (used here only for their 80-byte headers).
// QUICKSTART = github.com/pascal-research-inc/pascal-python-quickstart @ dd9d4824f0318c9fe26882876e5ed39165170485,
//   tests/test_signing_vectors.py: PLACE_ORDER_MESSAGE_HEX / PLACE_ORDER_SIGNATURE with the inputs of
//   test_place_order_signing_vector(), CANCEL_ORDER_MESSAGE_HEX.

export const KEYS = {
  source: "DOCS Request Signing key table; QUICKSTART OWNER_*/TRADING_* constants",
  ownerSeedHex: "07".repeat(32),
  ownerPublicKey: "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB",
  tradingSeedHex: "08".repeat(32),
  tradingPublicKey: "2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1",
  deploymentId: 3,
};

// Shared auth inputs of every trading-key vector (both sources): client_ts_ms 1731536000000, recv_window_ms 5000.
export const AUTH = {
  owner: KEYS.ownerPublicKey,
  signer: KEYS.tradingPublicKey,
  deploymentId: 3,
  clientTsMs: 1731536000000n,
  recvWindowMs: 5000,
};

export const DOCS_PLACE_ORDER = {
  source: "DOCS Request Signing > Place Order > Test vector (message_hex, signature) and Examples > Request",
  // Inputs as given by the docs request example. metadata byte 220 is 0 ("reserved: Zero byte").
  order: {
    clientOrderId: 42n,
    replaceClientOrderId: 43n,
    symbol: "SIM_EVENT_1.MARKET_1",
    side: "BID",
    price: "0.550000",
    size: 10,
    tif: "GTT",
    postOnly: true,
    reduceOnly: true,
    allowMissingReplace: true,
    expiresTsMs: 1731539600000n,
  },
  messageHex:
    "01010300881300000094962793010000ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca0a0000000000000070640800000000002a000000000000002b000000000000008082cd2793010000000000000000000000000000000000000000000000000000000000000000000053494d5f4556454e545f312e4d41524b45545f3100000000000000000000000000000000dcaf53738f1d5f06164660498a75888e7f0c78ed5f1bc17be1dd1cb3ccf53ce3000000010700000000000000",
  signature: "46gXGMwTxEckctHD3tjVyedhGG95VK2F97F3idkSYFR3WNEX4rG9g6g8KDTQPoxzLieudk8WHDXw7xNV3R2WwKCd",
  request: {
    client_order_id: "42",
    replace_client_order_id: "43",
    symbol: "SIM_EVENT_1.MARKET_1",
    side: "BID",
    price: "0.550000",
    size: "10",
    tif: "GTT",
    post_only: true,
    reduce_only: true,
    allow_missing_replace: true,
    expires_ts_ms: "1731539600000",
    auth: {
      client_ts_ms: "1731536000000",
      recv_window_ms: "5000",
      owner: "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB",
      signer: "2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1",
      signature: "46gXGMwTxEckctHD3tjVyedhGG95VK2F97F3idkSYFR3WNEX4rG9g6g8KDTQPoxzLieudk8WHDXw7xNV3R2WwKCd",
    },
  },
};

export const QUICKSTART_PLACE_ORDER = {
  source: "QUICKSTART tests/test_signing_vectors.py::test_place_order_signing_vector",
  // Same order without allow_missing_replace; the quickstart writes its client metadata byte
  // PYTHON_CLIENT_METADATA = 2 at offset 220 (pascal_quickstart/signing.py).
  order: {
    clientOrderId: 42n,
    replaceClientOrderId: 43n,
    symbol: "SIM_EVENT_1.MARKET_1",
    side: "BID",
    price: "0.550000",
    size: 10,
    tif: "GTT",
    postOnly: true,
    reduceOnly: true,
    expiresTsMs: 1731539600000n,
    metadata: 2,
  },
  messageHex:
    "01010300881300000094962793010000" +
    "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c" +
    "1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca" +
    "0a0000000000000070640800000000002a000000000000002b00000000000000" +
    "8082cd279301000000000000000000000000000000000000000000000000000000" +
    "0000000000000053494d5f4556454e545f312e4d41524b45545f310000000000" +
    "0000000000000000000000dcaf53738f1d5f06164660498a75888e7f0c78ed" +
    "5f1bc17be1dd1cb3ccf53ce3020000010300000000000000",
  signature: "3uG7HnnR2j3R26LoW3VNSJzKUfs16M9kXc58JbhkACwm2KmBXQuozLCQvREuAoxuHQjzADdBTV7NkgNrqRANgaZN",
};

// First 80 bytes (the permit header) of other trading-key vectors, to check cmd_type handling.
export const OTHER_HEADERS = [
  {
    source: "DOCS Request Signing > Cancel Order > Test vector message_hex (bytes 0..80)",
    cmdType: 2,
    messageHex:
      "01020300881300000094962793010000ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca00000000000000002a00000000000000",
  },
  {
    source: "QUICKSTART tests/test_signing_vectors.py CANCEL_ORDER_MESSAGE_HEX (bytes 0..80)",
    cmdType: 2,
    messageHex:
      "01020300881300000094962793010000" +
      "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c" +
      "1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca" +
      "00020000000000002a00000000000000",
  },
  {
    source: "DOCS Request Signing > Mass Cancel > Test vector message_hex (bytes 0..80)",
    cmdType: 10,
    messageHex:
      "010a0300881300000094962793010000ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca000000000000000053494d5f4556454e545f312e4d41524b45545f31000000000000000000000000",
  },
];
