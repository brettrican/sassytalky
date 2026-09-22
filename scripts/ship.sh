#!/usr/bin/env bash
# Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
# Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
# CodeMark: SCLLC1-sassytalkie-56XZVRHZUV2W
# scripts/ship.sh — one-button release pipeline.
#
# Usage:
#   scripts/ship.sh <version>
#
# Example:
#   scripts/ship.sh 2.7.6
#
# What it does, in order, with early-exit on failure:
#   1. Verify the AAB + universal APK exist on disk at the expected paths
#      (caller is responsible for having already run bundleRelease + the
#      bundletool universal-APK extract).
#   2. Push both to R2 using VERSIONED keys (sassytalkie-vX.Y.Z.{apk,aab}).
#      Wrangler PUTs to versioned keys work reliably; canonical key PUTs
#      silently truncate on Windows wrangler (≤4.98). We never push to the
#      canonical key directly.
#   3. Update the sassyconsultingllc worker's `LATEST_ANDROID_APK` and
#      `LATEST_ANDROID_AAB` env vars in wrangler.jsonc to point at the new
#      versioned filenames, then `wrangler deploy` so the canonical
#      `sassytalkie.apk` / `sassytalkie.aab` URLs serve the new version.
#   4. Sanity HEAD on the canonical URL and verify the served Content-Length
#      matches the local APK byte count.
#
# Required env:
#   CLOUDFLARE_ACCOUNT_ID    — Sassy Consulting LLC account ID
# Required tools on PATH:
#   wrangler (or npx wrangler@latest), curl, sha256sum, jq, sed
set -euo pipefail

VERSION="${1:?usage: ship.sh <version>  (e.g. 2.7.6)}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# v2.9.0 flavor split: the website serves the DIRECT flavor APK (license-key
# gate, no Google Billing); the AAB pushed to Play is the PLAY flavor
# (Billing paywall). Build with:
#   ./gradlew assembleDirectRelease bundlePlayRelease
APK="$ROOT/android-app/app/build/outputs/apk/direct/release/app-direct-release.apk"
AAB="$ROOT/android-app/app/build/outputs/bundle/playRelease/app-play-release.aab"
WORKER_CFG="${SASSYCONSULTINGLLC_WORKER_DIR:-/v/Projects/sassyconsultingllc-cloudflare}/wrangler.jsonc"

: "${CLOUDFLARE_ACCOUNT_ID:?CLOUDFLARE_ACCOUNT_ID must be set (Sassy Consulting LLC account)}"

echo "── ship.sh v${VERSION} ─────────────────────────────────────────────"

# 0) native-lib staleness guard
#
# This script uploads whatever is already built, and Gradle packages whatever
# .so is committed — neither compiles Rust. That combination shipped x86_64
# three releases stale (3.1.13/14/15 all carried a 3.1.12-era lib) because
# only arm64 got rebuilt by hand and nothing anywhere complained. CI now
# builds both ABIs from source; this is the same guard for the local path.
#
# Rule: no Rust source may be newer than either committed .so, and no .so may
# be newer than the artifacts about to ship.
JNILIBS="$ROOT/android-app/app/src/main/jniLibs"
RUST_SRC_DIRS=("$ROOT/android-native/src" "$ROOT/core/src")
stale=0
for abi in arm64-v8a x86_64; do
  so="$JNILIBS/$abi/libsassytalkie.so"
  if [ ! -s "$so" ]; then
    echo "FAIL: $abi/libsassytalkie.so missing or empty" >&2
    stale=1
    continue
  fi
  newer=$(find "${RUST_SRC_DIRS[@]}" -type f \( -name '*.rs' -o -name '*.toml' \) \
            -newer "$so" -print -quit 2>/dev/null || true)
  if [ -n "$newer" ]; then
    echo "FAIL: $abi native lib is OLDER than Rust source ($newer)" >&2
    stale=1
  fi
done
if [ "$stale" -ne 0 ]; then
  cat >&2 <<'HINT'

Rebuild BOTH ABIs in one invocation, then re-package, then re-run ship.sh:

  (cd android-native && cargo ndk -t arm64-v8a -t x86_64 \
      -o ../android-app/app/src/main/jniLibs build --release)
  (cd android-app && ./gradlew assembleDirectRelease bundlePlayRelease)

Build the two ABIs together — building them separately is what let one go
stale unnoticed.
HINT
  exit 1
fi
echo "native libs: both ABIs present and newer than Rust sources"

# 1) preflight
for f in "$APK" "$AAB"; do
  [ -f "$f" ] || { echo "missing: $f" >&2; exit 1; }
done

# Artifacts must be newer than the libs they are supposed to contain.
for abi in arm64-v8a x86_64; do
  so="$JNILIBS/$abi/libsassytalkie.so"
  for art in "$APK" "$AAB"; do
    if [ "$so" -nt "$art" ]; then
      echo "FAIL: $(basename "$art") predates $abi/libsassytalkie.so — re-package before shipping" >&2
      exit 1
    fi
  done
done
APK_SIZE=$(stat -c %s "$APK")
APK_HASH=$(sha256sum "$APK" | awk '{print $1}')
AAB_SIZE=$(stat -c %s "$AAB")
echo "  APK ${APK_SIZE} bytes, sha256 ${APK_HASH:0:16}…"
echo "  AAB ${AAB_SIZE} bytes"

# 2) versioned R2 pushes (these work reliably)
WRANGLER="npx --yes wrangler@latest"
APK_KEY="sassy-talk/android/sassytalkie-v${VERSION}.apk"
AAB_KEY="sassy-talk/android/sassytalkie-v${VERSION}.aab"

echo "  → put ${APK_KEY}"
$WRANGLER r2 object put "sassy-downloads/${APK_KEY}" \
  --file="$APK" --content-type=application/vnd.android.package-archive \
  --remote >/dev/null
echo "  → put ${AAB_KEY}"
$WRANGLER r2 object put "sassy-downloads/${AAB_KEY}" \
  --file="$AAB" --content-type=application/octet-stream \
  --remote >/dev/null

# Round-trip verify the APK — catches the silent-truncation bug if it ever
# bites the versioned key too. AAB is best-effort (size check only).
TMP="$(mktemp -t apk-verify-XXXXXX)"
$WRANGLER r2 object get "sassy-downloads/${APK_KEY}" --remote --file="$TMP" >/dev/null
REMOTE_HASH=$(sha256sum "$TMP" | awk '{print $1}')
rm -f "$TMP"
if [ "$REMOTE_HASH" != "$APK_HASH" ]; then
  echo "FAIL: R2 APK ($REMOTE_HASH) != local ($APK_HASH)" >&2
  exit 1
fi
echo "  ✓ versioned APK round-trip"

# 3) update worker env-vars + deploy
[ -f "$WORKER_CFG" ] || {
  echo "missing worker config: $WORKER_CFG" >&2
  echo "Expected sibling marketing-site wrangler.jsonc (default path above)." >&2
  echo "Override with SASSYCONSULTINGLLC_WORKER_DIR=/path/to/sassyconsultingllc-cloudflare" >&2
  exit 1
}
# Use sed in-place to flip the two pinned strings. Pattern matches the
# committed shape "sassy-talk/android/sassytalkie-vX.Y.Z.{apk,aab}".
sed -i -E "s|\"LATEST_ANDROID_APK\": \"sassy-talk/android/sassytalkie-v[0-9.]+\\.apk\"|\"LATEST_ANDROID_APK\": \"${APK_KEY}\"|" "$WORKER_CFG"
sed -i -E "s|\"LATEST_ANDROID_AAB\": \"sassy-talk/android/sassytalkie-v[0-9.]+\\.aab\"|\"LATEST_ANDROID_AAB\": \"${AAB_KEY}\"|" "$WORKER_CFG"

WORKER_DIR=$(dirname "$WORKER_CFG")
echo "  → deploy worker (canonical URLs will switch to v${VERSION})"
(cd "$WORKER_DIR" && $WRANGLER deploy 2>&1 | tail -3)

# 4) confirm canonical serves the right bytes
echo "  → verify canonical URL"
sleep 2  # let propagation settle
SERVED_LEN=$(curl -sI "https://sassyconsultingllc.com/download/sassy-talk/android/sassytalkie.apk" \
  -r 0-0 -H "Range: bytes=0-0" -o /dev/null -w "%{size_download}\n" 2>/dev/null || true)
SERVED_TOTAL=$(curl -sI "https://sassyconsultingllc.com/download/sassy-talk/android/sassytalkie.apk" \
  | grep -i content-length | awk '{print $2}' | tr -d '\r')
echo "    canonical content-length: ${SERVED_TOTAL:-<unknown>}"
if [ "${SERVED_TOTAL:-0}" = "$APK_SIZE" ]; then
  echo "  ✓ canonical serves v${VERSION}"
else
  echo "  WARN: canonical content-length ${SERVED_TOTAL} ≠ local ${APK_SIZE}"
  echo "  (Worker deploy may still be propagating; recheck in 30s.)"
fi

echo ""
echo "── shipped v${VERSION} ──"
echo "  versioned APK: https://sassyconsultingllc.com/download/${APK_KEY}"
echo "  versioned AAB: https://sassyconsultingllc.com/download/${AAB_KEY}"
echo "  canonical APK: https://sassyconsultingllc.com/download/sassy-talk/android/sassytalkie.apk"
echo "  canonical AAB: https://sassyconsultingllc.com/download/sassy-talk/android/sassytalkie.aab"
echo "  upload AAB to Play: $AAB"
