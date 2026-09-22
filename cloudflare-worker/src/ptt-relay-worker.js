// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-35TFPXR3KTKZ
/**
 * SassyTalk PTT Relay — Dedicated Worker
 *
 * Pure WebSocket relay for encrypted audio. No website, no APIs, no assets.
 * Lives at relay.sassyconsultingllc.com
 *
 * cloudflare-worker/public/ is marketing/checkout HTML for local reference only —
 * it is NOT served by this worker (no [assets] binding; do not add one).
 */

export { PttRoom } from "./ptt-relay.js";
import { handleShareRoute } from "./share.js";
import { handlePresenceRoute } from "./presence.js";
import { handleViewerRoute } from "./viewer.js";
import { handleWellKnownRoute } from "./wellknown.js";
import { handleLicenseRoute } from "./license.js";
// Single source of truth for token verification + key rotation. The /ws path
// uses the exact same verifier as /presence and /share so they can never drift
// apart on what tokens they accept (the inline copy this replaced had already
// diverged — it was missing a type guard in timingSafeEqualHex).
import {
  verifyCapabilityIdentity,
  secretsFor,
  hmacSha256Hex,
  isValidRoomId,
  isValidPeerId,
  signCapabilityTokenV2,
  verifyIssuanceProof,
  extractToken,
} from "./relay-auth.js";

// Token lifetime in seconds. Short enough to limit replay risk, long enough
// that flaky cellular reconnects within the same session don't need a refresh.
const TOKEN_TTL_SEC = 300;

// Short-link targets. Keep the right-hand-side updated whenever the underlying
// R2 path or marketing-site download route changes — the public short URL
// stays stable (it is what gets baked into QR codes, posters, etc.).
const SHORT_LINKS = {
  "/dl/apk": "https://sassyconsultingllc.com/download/sassy-talk/android/sassytalkie.apk",
};

const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
  "Access-Control-Allow-Headers": "Content-Type",
};

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const path = url.pathname;

    if (request.method === "OPTIONS") {
      return new Response(null, { headers: CORS_HEADERS });
    }

    // Encrypted session-share endpoints. End-to-end: the worker never sees
    // plaintext or the key (key lives in the recipient URL's #fragment).
    const shareResp = await handleShareRoute(request, env, url);
    if (shareResp) return shareResp;

    // Presence: map (room, peer) → FCM token so audio for an offline peer
    // can trigger a wake push.
    const presenceResp = await handlePresenceRoute(request, env, url);
    if (presenceResp) return presenceResp;

    // License activation/validation for the direct-distribution APK build.
    // Play builds never call these (they use Play Billing client-side).
    const licenseResp = await handleLicenseRoute(request, env, url);
    if (licenseResp) return licenseResp;

    // Browser landing page for /v/<id> invite links. Recipients with the app
    // are bounced straight into it by the App Link; everyone else gets a chooser
    // (open / install) here instead of a bare 404. Never reads the share blob —
    // the #fragment key is resolved entirely client-side.
    const viewerResp = handleViewerRoute(request, url);
    if (viewerResp) return viewerResp;

    // App-ownership association files so /v/ links open the app directly:
    //   /.well-known/assetlinks.json               (Android App Links)
    //   /.well-known/apple-app-site-association     (iOS/macOS Universal Links)
    const wellKnownResp = handleWellKnownRoute(request, env, url);
    if (wellKnownResp) return wellKnownResp;

    // Health check — 503 when required bindings/secrets are missing so probes
    // and operators see a misconfigure before clients hit opaque /auth or /ws failures.
    if (path === "/" || path === "/health") {
      const missing = [];
      if (!env.AUTH_SECRET) missing.push("AUTH_SECRET");
      if (!env.SHARES) missing.push("SHARES");
      if (!env.LICENSES) missing.push("LICENSES");
      if (missing.length) {
        return jsonResponse({
          service: "sassytalk-relay",
          status: "misconfigured",
          missing,
        }, 503);
      }
      return jsonResponse({
        service: "sassytalk-relay",
        status: "ok",
        max_peers_per_room: 16,
      });
    }

    // Issue an HMAC-signed token bound to (roomId, expiry). Client hits this
    // before opening the WebSocket. No PII required — the token is purely a
    // capability grant, and the audio payload itself is end-to-end encrypted.
    if (path === "/auth") {
      const roomId = url.searchParams.get("room");
      if (!isValidRoomId(roomId)) {
        return jsonResponse({ error: "Missing or invalid room ID" }, 400);
      }
      if (!env.AUTH_SECRET) {
        // Surface a clear error rather than silently issuing tokens with a
        // weak/empty key. Operator must `wrangler secret put AUTH_SECRET`.
        return jsonResponse({ error: "AUTH_SECRET not configured" }, 500);
      }
      const peer = url.searchParams.get("peer");
      const requestedClass = url.searchParams.get("auth_class") || "member";
      if (peer !== null && !isValidPeerId(peer)) {
        return jsonResponse({ error: "Invalid peer ID" }, 400, false);
      }
      if (requestedClass !== "member" && requestedClass !== "terminal") {
        return jsonResponse({ error: "Unsupported auth class" }, 400, false);
      }

      // Existing clients omit peer and receive the legacy room-bound token.
      // Identity-bound v2 issuance requires peer. Terminal classification
      // always requires a fresh cryptographic proof; operators may also make
      // proof mandatory for every issuance with REQUIRE_AUTH_PROOF=true.
      const proofRequired = requestedClass === "terminal" || env.REQUIRE_AUTH_PROOF === "true";
      if (proofRequired) {
        if (!peer) return jsonResponse({ error: "Proof-based auth requires peer" }, 400, false);
        const proofErr = await verifyIssuanceProof(
          request, roomId, peer, requestedClass, env.AUTH_SECRET,
        );
        if (proofErr) return jsonResponse({ error: proofErr }, 401, false);
        const nonceErr = await consumeProofNonce(request, env);
        if (nonceErr) return jsonResponse({ error: nonceErr }, 401, false);
      }

      const expSec = Math.floor(Date.now() / 1000) + TOKEN_TTL_SEC;
      const token = peer
        ? await signCapabilityTokenV2(roomId, expSec, peer, requestedClass, env.AUTH_SECRET)
        : await signLegacyToken(roomId, expSec, env.AUTH_SECRET);
      // No CORS on the token response: native clients (OkHttp/reqwest) don't
      // enforce CORS, and withholding it stops a malicious web origin from
      // reading a freshly-minted room capability token.
      return jsonResponse({
        token,
        token_version: peer ? 2 : 1,
        auth_class: peer ? requestedClass : "legacy",
        expires_at: expSec,
        ttl: TOKEN_TTL_SEC,
      }, 200, false);
    }

    // Short-link redirects. 302 (not 301) so we can re-point later without
    // browsers and link unfurlers permanently caching the old destination.
    if (SHORT_LINKS[path]) {
      return new Response(null, {
        status: 302,
        headers: {
          Location: SHORT_LINKS[path],
          // 5-minute redirect cache — small enough to roll a fix quickly,
          // large enough that QR-scan stampedes don't hammer the worker.
          "Cache-Control": "public, max-age=300",
          ...CORS_HEADERS,
        },
      });
    }

    // WebSocket relay
    if (path === "/ws" || path === "/api/ptt/ws") {
      const roomId = url.searchParams.get("room");
      if (!isValidRoomId(roomId)) {
        return new Response("Missing or invalid room ID", { status: 400 });
      }

      // Token verification. We do this in the worker so an attacker can't
      // even cause a DO to be instantiated (which costs $) without a valid
      // capability for that specific room. Fail CLOSED: a missing AUTH_SECRET
      // is a server misconfiguration, not a reason to drop room-scoping and
      // let anyone open any room.
      if (!env.AUTH_SECRET) {
        return new Response("Server misconfigured: AUTH_SECRET unset", { status: 503 });
      }
      // Prefer Authorization / Sec-WebSocket-Protocol (desktop); keep ?token= for
      // Android/iOS and older clients. extractToken owns the precedence.
      const token = extractToken(request, url);
      const identity = await verifyCapabilityIdentity(token, roomId, secretsFor(env));
      if (identity.error) {
        return new Response(identity.error, { status: 401 });
      }
      const claimedPeer = url.searchParams.get("peer") || "";
      if (identity.version === 2 && claimedPeer !== identity.peer) {
        return new Response("Token peer identity mismatch", { status: 403 });
      }

      const doId = env.PTT_RELAY.idFromName(roomId);
      const room = env.PTT_RELAY.get(doId);
      // Durable Objects are not publicly addressable. Overwrite (never trust)
      // these internal headers so the DO can classify the authenticated socket.
      const headers = new Headers(request.headers);
      headers.set("X-Sassy-Verified-Auth-Class", identity.authClass);
      headers.set("X-Sassy-Verified-Peer", identity.peer || claimedPeer);
      headers.set("X-Sassy-Token-Version", String(identity.version));
      return room.fetch(new Request(request, { headers }));
    }

    return new Response("Not found", { status: 404 });
  },
};

function jsonResponse(body, status = 200, cors = true) {
  const headers = { "Content-Type": "application/json" };
  if (cors) Object.assign(headers, CORS_HEADERS);
  return new Response(JSON.stringify(body), { status, headers });
}

/**
 * Token format: "<expSec>.<hexSig>" where hexSig = HMAC-SHA256(roomId + "." + expSec, secret).
 * Compact, query-param-safe, no JSON parsing needed on either end.
 */
async function signLegacyToken(roomId, expSec, secret) {
  const sig = await hmacSha256Hex(`${roomId}.${expSec}`, secret);
  return `${expSec}.${sig}`;
}

/**
 * Best-effort replay rejection for proof nonces using the already-required
 * SHARES KV binding. KV is eventually consistent, so the timestamp window and
 * short-lived capability remain the primary bounds; this blocks ordinary
 * accidental/captured replay without inventing another service.
 */
async function consumeProofNonce(request, env) {
  if (!env.SHARES) return "Proof replay store unavailable";
  const nonce = request.headers.get("X-Sassy-Auth-Nonce");
  const key = `auth-proof-nonce:${nonce}`;
  if (await env.SHARES.get(key)) return "Proof nonce already used";
  await env.SHARES.put(key, "1", { expirationTtl: 120 });
  return null;
}
