// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-A6CM6WSA3BMG
/**
 * relay-auth.js — Shared capability-token helpers for the SassyTalk relay.
 *
 * Tokens minted by GET /auth?room=... are accepted by every endpoint:
 *
 *   legacy: "<expSec>.<hexSig>"
 *   v2:     "v2.<expSec>.<peerB64>.<authClass>.<hexSig>"
 *
 * v2 binds the capability to both the room and peer identity. Legacy tokens
 * remain verifiable while unmodified Android/iOS/desktop clients migrate.
 *
 * presence.js and share.js import the verifiers from here so that
 * registering an FCM token / minting a share blob requires the same room
 * capability as opening the WebSocket. This module is the single source of
 * truth for every route, including the WebSocket front door.
 */

// Allowed clock skew between worker and client when verifying token.exp.
const CLOCK_SKEW_SEC = 30;
const PROOF_CLOCK_SKEW_SEC = 60;
const ROOM_RE = /^[A-Za-z0-9._:@-]{8,64}$/;
const PEER_RE = /^[A-Za-z0-9._:@-]{1,64}$/;
const NONCE_RE = /^[A-Za-z0-9_-]{16,128}$/;
const AUTH_CLASSES = new Set(["member", "terminal"]);

/**
 * Verify a capability token against a room id.
 * Returns null on success, or a human-readable error string on failure.
 * Fails CLOSED: with no active key configured this rejects every token.
 *
 * `secret` accepts a single key or a list of active keys. During an AUTH_SECRET
 * rotation, pass secretsFor(env) = [AUTH_SECRET, AUTH_SECRET_PREV] so tokens
 * minted under the outgoing key keep verifying until they expire. Tokens are
 * always *signed* with the current AUTH_SECRET only.
 */
export async function verifyCapabilityToken(token, roomId, secret) {
  const result = await verifyCapabilityIdentity(token, roomId, secret);
  return result.error;
}

/**
 * Verify a token and return its bound identity/classification.
 *
 * Legacy success: { error: null, version: 1, peer: null, authClass: "legacy" }
 * v2 success:     { error: null, version: 2, peer, authClass }
 */
export async function verifyCapabilityIdentity(token, roomId, secret) {
  const secrets = (Array.isArray(secret) ? secret : [secret]).filter(Boolean);
  if (secrets.length === 0) return identityError("AUTH_SECRET not configured");
  if (!token || typeof token !== "string") return identityError("Missing token");

  if (token.startsWith("v2.")) {
    return verifyV2Token(token, roomId, secrets);
  }

  const dot = token.indexOf(".");
  if (dot <= 0) return identityError("Malformed token");
  const expRaw = token.slice(0, dot);
  if (!/^\d+$/.test(expRaw)) return identityError("Malformed token");
  const expSec = Number.parseInt(expRaw, 10);
  const sig = token.slice(dot + 1);
  if (!Number.isSafeInteger(expSec) || !/^[0-9a-f]{64}$/.test(sig)) {
    return identityError("Malformed token");
  }

  const nowSec = Math.floor(Date.now() / 1000);
  if (expSec + CLOCK_SKEW_SEC < nowSec) return identityError("Token expired");

  // Accept a signature from any active key. Loop over all candidates without
  // short-circuiting so verification time doesn't reveal which key matched.
  let ok = false;
  for (const s of secrets) {
    const expected = await hmacSha256Hex(`${roomId}.${expSec}`, s);
    if (timingSafeEqualHex(sig, expected)) ok = true;
  }
  return ok
    ? { error: null, version: 1, peer: null, authClass: "legacy", expSec }
    : identityError("Invalid token signature");
}

async function verifyV2Token(token, roomId, secrets) {
  const parts = token.split(".");
  if (parts.length !== 5) return identityError("Malformed token");
  const [, expRaw, peerB64, authClass, sig] = parts;
  if (!/^\d+$/.test(expRaw)) return identityError("Malformed token");
  const expSec = Number.parseInt(expRaw, 10);
  const peer = decodeBase64Url(peerB64);
  if (!Number.isSafeInteger(expSec) || !peer || !PEER_RE.test(peer) ||
      !AUTH_CLASSES.has(authClass) || !/^[0-9a-f]{64}$/.test(sig)) {
    return identityError("Malformed token");
  }
  if (expSec + CLOCK_SKEW_SEC < Math.floor(Date.now() / 1000)) {
    return identityError("Token expired");
  }

  const canonical = tokenV2Canonical(roomId, expSec, peer, authClass);
  let ok = false;
  for (const secret of secrets) {
    const expected = await hmacSha256Hex(canonical, secret);
    if (timingSafeEqualHex(sig, expected)) ok = true;
  }
  return ok
    ? { error: null, version: 2, peer, authClass, expSec }
    : identityError("Invalid token signature");
}

/** Mint an identity-bound v2 capability token. */
export async function signCapabilityTokenV2(roomId, expSec, peer, authClass, secret) {
  if (!PEER_RE.test(peer || "") || !AUTH_CLASSES.has(authClass)) {
    throw new TypeError("Invalid capability identity");
  }
  const sig = await hmacSha256Hex(
    tokenV2Canonical(roomId, expSec, peer, authClass),
    secret,
  );
  return `v2.${expSec}.${encodeBase64Url(peer)}.${authClass}.${sig}`;
}

/**
 * Verify a versioned operator proof for token issuance.
 *
 * Proof v1:
 * HMAC-SHA256("v1\n<class>\n<room>\n<peer>\n<unixSec>\n<nonce>", AUTH_SECRET)
 *
 * This deliberately reuses AUTH_SECRET: deployments need no new secret or
 * service. It is suitable for trusted terminal/operator tooling that already
 * has the relay secret. It is not an E2E room-key proof; the relay never has
 * the clients' encryption key and therefore cannot validate one.
 */
export async function verifyIssuanceProof(request, roomId, peer, authClass, secret) {
  if (!secret) return "AUTH_SECRET not configured";
  if (request.headers.get("X-Sassy-Auth-Version") !== "1") {
    return "Missing or unsupported proof version";
  }
  const tsRaw = request.headers.get("X-Sassy-Auth-Timestamp") || "";
  const nonce = request.headers.get("X-Sassy-Auth-Nonce") || "";
  const proof = (request.headers.get("X-Sassy-Auth-Proof") || "").toLowerCase();
  if (!/^\d+$/.test(tsRaw)) return "Malformed proof";
  const ts = Number.parseInt(tsRaw, 10);
  if (!Number.isSafeInteger(ts) || Math.abs(Math.floor(Date.now() / 1000) - ts) > PROOF_CLOCK_SKEW_SEC) {
    return "Proof timestamp outside allowed window";
  }
  if (!NONCE_RE.test(nonce) || !/^[0-9a-f]{64}$/.test(proof)) {
    return "Malformed proof";
  }
  const expected = await hmacSha256Hex(
    `v1\n${authClass}\n${roomId}\n${peer}\n${ts}\n${nonce}`,
    secret,
  );
  return timingSafeEqualHex(proof, expected) ? null : "Invalid proof";
}

/**
 * Active verification keys, current first: [AUTH_SECRET] normally, or
 * [AUTH_SECRET, AUTH_SECRET_PREV] during a *time-bounded* rotation grace window.
 *
 * The previous key is honored ONLY while AUTH_SECRET_PREV_UNTIL (unix seconds)
 * is set and in the future. This makes the grace window self-closing: if the
 * operator forgets the final `secret delete`, a leaked old key still stops being
 * accepted at AUTH_SECRET_PREV_UNTIL instead of forging tokens forever. A
 * PREV with no/expired UNTIL is ignored (fail toward the current key only).
 *
 * Zero-downtime rotation:
 *   1. wrangler secret put AUTH_SECRET_PREV        (= the current AUTH_SECRET value)
 *   2. wrangler secret put AUTH_SECRET_PREV_UNTIL  (= now + a few token lifetimes, e.g. `date -d '+15 min' +%s`)
 *   3. wrangler secret put AUTH_SECRET             (= a fresh `openssl rand -hex 32`)
 *   4. after AUTH_SECRET_PREV_UNTIL passes, optionally `wrangler secret delete AUTH_SECRET_PREV AUTH_SECRET_PREV_UNTIL`
 *      (acceptance already stopped at the deadline — the delete is just cleanup)
 */
export function secretsFor(env) {
  const active = [env.AUTH_SECRET];
  if (env.AUTH_SECRET_PREV && prevWindowOpen(env)) {
    active.push(env.AUTH_SECRET_PREV);
  }
  return active.filter(Boolean);
}

/** True while a configured AUTH_SECRET_PREV is still inside its grace window. */
function prevWindowOpen(env) {
  const until = Number.parseInt(env.AUTH_SECRET_PREV_UNTIL || "", 10);
  if (!Number.isFinite(until)) return false; // no deadline set → don't honor PREV
  return Math.floor(Date.now() / 1000) <= until;
}

/**
 * Pull the capability token from an incoming request. Precedence:
 *   1. Authorization: Bearer <token>
 *   2. Sec-WebSocket-Protocol (comma list; first non-empty token-like value,
 *      or a `sassytalk.<token>` entry)
 *   3. X-Sassy-Token header
 *   4. ?token= query param (Android/iOS and older clients)
 */
export function extractToken(request, url) {
  const auth = request.headers.get("Authorization") || "";
  if (auth.startsWith("Bearer ")) return auth.slice(7).trim();

  const proto = request.headers.get("Sec-WebSocket-Protocol") || "";
  if (proto) {
    for (const part of proto.split(",")) {
      const p = part.trim();
      if (p.startsWith("sassytalk.")) return p.slice("sassytalk.".length);
    }
  }

  const hdr = request.headers.get("X-Sassy-Token");
  if (hdr && hdr.trim()) return hdr.trim();

  return url.searchParams.get("token");
}

export async function hmacSha256Hex(data, secret) {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(data));
  const bytes = new Uint8Array(sig);
  let hex = "";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return hex;
}

export function timingSafeEqualHex(a, b) {
  if (typeof a !== "string" || typeof b !== "string") return false;
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

export function isValidRoomId(id) {
  return typeof id === "string" && ROOM_RE.test(id);
}

export function isValidPeerId(id) {
  return typeof id === "string" && PEER_RE.test(id);
}

function tokenV2Canonical(roomId, expSec, peer, authClass) {
  return `v2\n${roomId}\n${expSec}\n${peer}\n${authClass}`;
}

function identityError(error) {
  return { error, version: null, peer: null, authClass: null, expSec: null };
}

function encodeBase64Url(value) {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function decodeBase64Url(value) {
  try {
    if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
    const padded = value.replace(/-/g, "+").replace(/_/g, "/")
      + "=".repeat((4 - (value.length % 4)) % 4);
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}
