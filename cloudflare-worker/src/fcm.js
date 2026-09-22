// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-OSGJSX6GIIUF
/**
 * fcm.js — Firebase Cloud Messaging HTTP v1 wake-push sender for the relay.
 *
 * Consumed by ptt-relay.js (DO) → sendWakePush(env, token, roomId).
 *
 * Sends a DATA-ONLY, high-priority message so the client wakes and reconnects
 * to pull the (end-to-end encrypted) audio itself. The push payload carries
 * only a room identifier — never audio or message content.
 *
 * Auth: a Google OAuth2 access token is minted from the service-account JSON in
 * env.FCM_SERVICE_ACCOUNT_JSON via a signed JWT (RS256, WebCrypto) and cached
 * for its lifetime so a busy room doesn't re-mint on every push.
 *
 * Returns { ok, status, error, stale }:
 *   ok=true                      → delivered
 *   ok=false, stale=true         → token is dead (UNREGISTERED / 404); caller
 *                                  should drop the presence row
 *   ok=false, stale=false        → transient/other failure; keep the row
 */

const FCM_SCOPE = "https://www.googleapis.com/auth/firebase.messaging";
const TOKEN_SKEW_MS = 60_000; // refresh a minute early to avoid edge expiry

// Module-scoped cache. A Durable Object is a single isolate, so this persists
// across pushes within the DO's lifetime. Keyed by client_email so a config
// swap doesn't serve a stale token.
let _accessCache = null; // { key, accessToken, expMs }

export async function sendWakePush(env, token, roomId) {
  if (!env || !env.FCM_SERVICE_ACCOUNT_JSON) {
    return { ok: false, status: 0, error: "FCM not configured", stale: false };
  }
  if (!token) {
    return { ok: false, status: 0, error: "Empty token", stale: true };
  }

  let sa;
  try {
    sa = typeof env.FCM_SERVICE_ACCOUNT_JSON === "string"
      ? JSON.parse(env.FCM_SERVICE_ACCOUNT_JSON)
      : env.FCM_SERVICE_ACCOUNT_JSON;
  } catch {
    return { ok: false, status: 0, error: "Bad FCM_SERVICE_ACCOUNT_JSON", stale: false };
  }
  if (!sa.project_id || !sa.client_email || !sa.private_key) {
    return { ok: false, status: 0, error: "Incomplete service account", stale: false };
  }

  let accessToken;
  try {
    accessToken = await getAccessToken(sa);
  } catch (e) {
    return { ok: false, status: 0, error: `OAuth mint failed: ${e.message}`, stale: false };
  }

  const url = `https://fcm.googleapis.com/v1/projects/${sa.project_id}/messages:send`;
  const wakeTs = String(Date.now());
  const message = {
    message: {
      token,
      data: {
        // Dual keys: Android historically required kind/ts; older servers sent
        // type/sentAt. Send both so any current client matches any current server.
        kind: "wake",
        type: "wake",
        room: String(roomId),
        ts: wakeTs,
        sentAt: wakeTs,
      },
      android: { priority: "high", ttl: "60s" },
      apns: {
        headers: { "apns-priority": "10", "apns-push-type": "background" },
        payload: { aps: { "content-available": 1 } },
      },
    },
  };

  let res;
  try {
    res = await fetch(url, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${accessToken}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(message),
    });
  } catch (e) {
    return { ok: false, status: 0, error: `fetch failed: ${e.message}`, stale: false };
  }

  if (res.ok) return { ok: true, status: res.status, error: null, stale: false };

  let errCode = "";
  let errMsg = "";
  try {
    const body = await res.json();
    errMsg = body?.error?.message || "";
    const details = body?.error?.details || [];
    for (const d of details) {
      if (d.errorCode) errCode = d.errorCode;
    }
    if (!errCode) errCode = body?.error?.status || "";
  } catch { /* non-JSON error body */ }

  const stale = res.status === 404 || errCode === "UNREGISTERED" || errCode === "INVALID_ARGUMENT";
  return { ok: false, status: res.status, error: errMsg || errCode || `HTTP ${res.status}`, stale };
}

async function getAccessToken(sa) {
  const now = Date.now();
  if (_accessCache && _accessCache.key === sa.client_email && _accessCache.expMs - TOKEN_SKEW_MS > now) {
    return _accessCache.accessToken;
  }

  const tokenUri = sa.token_uri || "https://oauth2.googleapis.com/token";
  const iat = Math.floor(now / 1000);
  const exp = iat + 3600;
  const header = { alg: "RS256", typ: "JWT" };
  const claims = { iss: sa.client_email, scope: FCM_SCOPE, aud: tokenUri, iat, exp };

  const signingInput = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(claims))}`;
  const key = await importPkcs8(sa.private_key);
  const sig = await crypto.subtle.sign(
    { name: "RSASSA-PKCS1-v1_5" },
    key,
    new TextEncoder().encode(signingInput),
  );
  const jwt = `${signingInput}.${b64urlBytes(new Uint8Array(sig))}`;

  const res = await fetch(tokenUri, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: `grant_type=${encodeURIComponent("urn:ietf:params:oauth:grant-type:jwt-bearer")}&assertion=${encodeURIComponent(jwt)}`,
  });
  if (!res.ok) {
    const t = await res.text().catch(() => "");
    throw new Error(`token endpoint ${res.status}: ${t.slice(0, 200)}`);
  }
  const body = await res.json();
  if (!body.access_token) throw new Error("no access_token in response");

  _accessCache = {
    key: sa.client_email,
    accessToken: body.access_token,
    expMs: now + (Number(body.expires_in) || 3600) * 1000,
  };
  return _accessCache.accessToken;
}

async function importPkcs8(pem) {
  const der = pemToDer(pem);
  return crypto.subtle.importKey(
    "pkcs8",
    der,
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["sign"],
  );
}

function pemToDer(pem) {
  // Service-account private_key arrives with literal "\n" already expanded by
  // JSON.parse; strip the PEM armor and any whitespace, then base64-decode.
  const b64 = pem
    .replace(/-----BEGIN [^-]+-----/, "")
    .replace(/-----END [^-]+-----/, "")
    .replace(/\s+/g, "");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

function b64url(str) {
  return b64urlBytes(new TextEncoder().encode(str));
}

function b64urlBytes(bytes) {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
