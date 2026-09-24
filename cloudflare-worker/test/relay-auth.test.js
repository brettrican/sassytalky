// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
import { describe, it, expect } from "vitest";
import {
  hmacSha256Hex,
  signCapabilityTokenV2,
  verifyCapabilityIdentity,
  verifyCapabilityToken,
  verifyIssuanceProof,
} from "../src/relay-auth.js";

const SECRET = "test-only-auth-secret-with-enough-entropy";
const ROOM = "room-test-0001";
const PEER = "peer.install-01";

describe("relay capability auth", () => {
  it("accepts legacy room tokens for compatibility", async () => {
    const exp = Math.floor(Date.now() / 1000) + 300;
    const sig = await hmacSha256Hex(`${ROOM}.${exp}`, SECRET);
    expect(await verifyCapabilityToken(`${exp}.${sig}`, ROOM, SECRET)).toBeNull();
  });

  it("binds v2 tokens to room, peer, and class", async () => {
    const exp = Math.floor(Date.now() / 1000) + 300;
    const token = await signCapabilityTokenV2(ROOM, exp, PEER, "member", SECRET);
    const identity = await verifyCapabilityIdentity(token, ROOM, SECRET);
    expect(identity).toMatchObject({
      error: null,
      version: 2,
      peer: PEER,
      authClass: "member",
    });
    expect((await verifyCapabilityIdentity(token, "other-room-0001", SECRET)).error)
      .toBe("Invalid token signature");
  });

  it("rejects expired, malformed, and unconfigured verification", async () => {
    const expired = await signCapabilityTokenV2(
      ROOM, Math.floor(Date.now() / 1000) - 120, PEER, "member", SECRET,
    );
    expect((await verifyCapabilityIdentity(expired, ROOM, SECRET)).error).toBe("Token expired");
    expect((await verifyCapabilityIdentity("v2.bad", ROOM, SECRET)).error).toBe("Malformed token");
    expect((await verifyCapabilityIdentity("anything", ROOM, [])).error)
      .toBe("AUTH_SECRET not configured");
  });
});

describe("versioned issuance proof", () => {
  it("verifies a fresh room/peer/class-bound proof", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const nonce = "nonce-for-test-0001";
    const proof = await hmacSha256Hex(
      `v1\nterminal\n${ROOM}\n${PEER}\n${ts}\n${nonce}`,
      SECRET,
    );
    const request = new Request("https://relay.example/auth", {
      headers: {
        "X-Sassy-Auth-Version": "1",
        "X-Sassy-Auth-Timestamp": String(ts),
        "X-Sassy-Auth-Nonce": nonce,
        "X-Sassy-Auth-Proof": proof,
      },
    });
    expect(await verifyIssuanceProof(request, ROOM, PEER, "terminal", SECRET)).toBeNull();
    expect(await verifyIssuanceProof(request, ROOM, "different-peer", "terminal", SECRET))
      .toBe("Invalid proof");
  });
});

describe("extractToken precedence", () => {
  it("prefers Authorization, then protocol, then X-Sassy-Token, then query", async () => {
    const { extractToken } = await import("../src/relay-auth.js");
    const url = new URL("https://relay.example/ws?room=r&token=from-query");
    const bearer = new Request(url, { headers: { Authorization: "Bearer from-bearer" } });
    expect(extractToken(bearer, url)).toBe("from-bearer");
    const proto = new Request(url, {
      headers: { "Sec-WebSocket-Protocol": "sassytalk.from-proto" },
    });
    expect(extractToken(proto, url)).toBe("from-proto");
    const hdr = new Request(url, { headers: { "X-Sassy-Token": "from-header" } });
    expect(extractToken(hdr, url)).toBe("from-header");
    const q = new Request(url);
    expect(extractToken(q, url)).toBe("from-query");
  });
});
