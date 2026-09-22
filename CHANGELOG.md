<!--
   Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
   Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
   CodeMark: SCLLC1-sassytalkie-TRE5664GFP4D
-->
# Changelog

All notable changes to SassyTalkie. Format loosely follows
[Keep a Changelog](https://keepachangelog.com); versions map to Android
`versionName` (versionCode in parentheses).

## [3.2.5] (81) - 2026-08-17

### Changed
- **Rebuilt both arm64-v8a and x86_64 native libs** from current Rust source
  (`cargo ndk`) so the shipping `.so` files match the floor-arbitration /
  `ptt_frames` consolidation already in tree. VersionCode 81 / versionName
  3.2.5 — no app Kotlin feature delta beyond the native rebuild (verified from
  the release commit).

## [3.2.4] (80) - 2026-08-17

### Changed
- **Target / compile SDK 35 → 36 (Android 16).** Play stops accepting updates
  that still target API 35 after 31 August 2026. Edge-to-edge, predictive back
  (`enableOnBackInvokedCallback` + Compose `BackHandler`), and unrestricted
  large-screen orientation were already in place from the Android 15 work —
  this release is the version bump Play's policy requires so production
  uploads can continue.

## [3.2.3] (79) - 2026-08-16

### Changed
- **Google Play Billing Library 7.1.1 → 9.1.0.** Play stops accepting updates
  that still ship Billing Library 7 after 31 August 2026. The paywall already
  used the ProductDetails / QueryPurchasesParams APIs, so the live change is
  the 8.x listener (`QueryProductDetailsResult.productDetailsList`) plus
  `enableAutoServiceReconnection()` so a dropped Play connection retries
  instead of hanging Restore. Next publishing deadline moves to 31 August 2028.

## [3.2.2] (78) - 2026-08-16

### Changed
- **Custom deep-link scheme is now `sassy-talks://`.** Registered in the
  manifest alongside the previous `sassytalk://`, which is still accepted on
  the way in — invites get pasted into chats and left there, and a link shared
  last week is live until it expires, so dropping the old scheme would turn
  every one of them into a dead tap. New links are minted with the new scheme
  only. The relay's `/v/` viewer page (the fallback for a click that does not
  resolve via App Links) now fires the new scheme too; that half takes effect
  on the next worker deploy.
  - Verified on-device: `sassy-talks://`, legacy `sassytalk://`, and the
    verified https App Link all open MainActivity rather than a browser.

### Added
- **Trial countdown before the paywall.** The trial worked but arrived
  unannounced — five sessions of a working radio, then a wall with no lead-up.
  The radio screen now shows a muted "2 free sessions left" / "Last free
  session" card with an Upgrade shortcut, for the last two sessions only:
  counting down from five nags someone who has barely started, and saying
  nothing is how the wall becomes a surprise.
- **The gate explains itself.** Both flavors distinguish "you used your 5 free
  sessions" from "this build arrived locked". A bare paywall after five working
  sessions reads as the app breaking rather than as a transaction.

### Fixed
- The gate is a dead end by design, but the new Upgrade shortcut made that a
  trap: a user who tapped it with sessions still left could not get back to the
  radio. Back now works from the gate while the trial is unspent, and only
  then — once it is gone the gate is final again.

## [3.2.1] (77) - 2026-08-16

### Fixed
- **Phones crashed on start after the 3.2 update.** `3.2` added JNI methods
  (`nativeSetSpeakerPreference`, `nativeReassertRxRoute`,
  `nativeHasLiveAudioPath`) and rebuilt only the x86_64 native library.
  Physical devices load arm64, which was still the 3.1.20 binary, so
  `setSpeakerphonePreference()` threw `UnsatisfiedLinkError` on the first
  launch after permissions. Both ABIs are now rebuilt together, and the
  JNI wrappers catch `Throwable` so a missing symbol logs instead of
  taking the process down. CI now also checks those 3.2 symbols exist in
  **both** `.so` files.

## [3.2] (76) - 2026-08-15

### Fixed
- **Jitter between two PTT radios.** The 400 ms “peer speaking” LED was being
  used as the transmit lock, so a normal cellular gap looked like a free
  channel, the second radio keyed up, and the live jitter buffer dumped both
  streams. Floor occupancy is now a separate hold (1.5 s of audio silence,
  300 ms after PTT_STOP). Incoming audio re-asserts the floor even if the LED
  already blinked off. The cache no longer treats a gap between two peers as
  network jitter, no longer drops the first talker's prebuffer on overlap, and
  no longer plays a second peer's beacon into the live talker's speaker.

### Added
- Authenticated control plane, deterministic floor/emergency preemption, TLS
  pin helpers, relay connection state, FCM wake policy, and technical-audit
  export — the uncommitted audit remediations that this Play build ships.

## [3.1.23] (75) - 2026-08-10

### Changed
- **Paywall now waits for 5 sessions that actually had a peer.** Previously the
  gate could greet a user before they had ever heard the radio work. "Value" is
  deliberately defined as a session with at least one peer present — not an app
  launch, and not a session created, because a walkie-talkie with nobody on the
  other end has demonstrated nothing, and hosting a QR nobody scans must not
  cost a free session. Counting is by distinct session id, so rejoining the same
  channel all day is one session, not a drained trial.
  - Every gate routes through one predicate (`TrialGate.mayUseRadio`) — the UI
    gate, `AutoConnectManager`, and all three `WalkieService` transport gates.
    Changing only the UI would have produced the worst version of this: a trial
    user on the radio screen whose transports silently refuse to connect.
  - The silent post-launch entitlement refresh also had to be taught about the
    trial; taking the server answer at face value would have dropped trial users
    onto the paywall a beat after launch, from a background coroutine.
  - Soft gate by design: it lives in SharedPreferences and clearing app data
    resets it. Hardening would need a server round-trip per join, which breaks
    the offline/mesh case the radio exists for.

### Fixed
- **Invite links now open straight into the session.** A tapped invite imported
  the session and then parked on the Auth screen waiting for a Continue press —
  a second confirmation of the decision the user just made by tapping the link,
  which read as "the link didn't work". It now goes straight to the radio, with
  the same relay teardown the Continue button did so the joiner lands in the
  imported room rather than the previous one. Verified that a URL click opens
  the app rather than the browser (App Links domain verification: verified).

## [3.1.22] (74) - 2026-08-10

### Added
- **Post-update state reset.** An in-place update keeps every byte of prior
  state while the binary underneath it changes: native caches and counters
  describe a process that no longer exists, and the relay socket is bound to
  the old one. That is why "just reinstall it" kept working as a fix — the
  update path never did the cleanup. On a version change the app now clears
  the derived state (audio cache, timeline, diagnostic counters).
  - Triggered by a startup version check, not only by
    `ACTION_MY_PACKAGE_REPLACED`: a force-stopped app never receives that
    broadcast and OEM battery managers force-stop aggressively, so the devices
    most likely to need the reset are the least likely to get the broadcast.
    The receiver is an accelerator that does the work before the app is opened.
  - Session keys, profile and entitlement are deliberately PRESERVED. Wiping
    them on every Play update would log every user out of their channel and
    force a QR re-pair with their host, which is worse than the staleness it
    fixes and would defeat the stored-credential rejoin path. Set
    `update_full_reset` for literal fresh-install semantics; off by default
    because the cost lands on the user.
  - Verified on real in-place updates: reset fires on 73->74 before first
    launch, does not fire on first install, does not repeat on relaunch, and a
    session established before a 74->75 update is restored afterwards.

## [3.1.21] (73) - 2026-08-09

### Fixed
- **PTT position could push the button off-screen and over the hint text.**
  Found by driving the new sliders to their limits rather than trusting mid-range
  values. Two distinct causes, both from `offset` not participating in layout:
  X had no bound at all, so a 2x-wide button at full left offset ran past the
  screen edge; and Y shifted only the drawing, sliding the button on top of the
  hint row while its neighbours stayed put. X is now clamped against the real
  measured width via `BoxWithConstraints`, and Y is applied as padding so the
  Column reflows and nothing can overlap at any slider combination.

## [3.1.20] (72) - 2026-08-09

### Changed
- **PTT button is fully adjustable.** A fixed 240dp centred circle assumed one
  grip; the radio actually gets held left-handed, right-handed, thumbed from the
  bottom edge, or palmed in a mount, and a circle has the least horizontal reach
  per unit of area. Settings now expose size, width scale, roundness, and X/Y
  position, with a reset. Roundness is a spectrum rather than a preset list:
  100% = circle (stadium when wide), 0% = rectangle, anything between = rounded
  rect. Default is now 1.25x wider than tall, so the resting shape is a stadium
  with materially more horizontal contact area.
- **Emergency SOS moved to the overflow menu.** On the radio screen it competed
  for space with the enlarged PTT target and sat where a sliding thumb lands.
  Menu -> item -> confirm is deliberate enough that the long-press gesture is no
  longer needed, and it frees the area under the PTT entirely.

### Fixed
- **Cohort re-entry claimed a QR scan was required when it was not.** Joined
  cohorts were labelled "Rejoin - Scan" unconditionally, even though the full
  session JSON (including the room key) is kept in EncryptedSharedPreferences on
  the original import and re-importing it restores the session outright. Users
  were being sent back to a host for a fresh QR to recover a session they still
  held. The row now checks for stored credentials and only says "Scan" when one
  is genuinely needed.

### Added
- **Pre/post-encryption tracing.** Bounded, frame-budgeted logging of each audio
  frame before and after AEAD, so encryption can be verified from outside the
  process instead of taken on faith. Debug builds arm it automatically for the
  first 3 frames of a transmission; it logs fragments of live mic audio, so it
  is budgeted rather than a persistent switch.

## [3.1.19] (71) - 2026-08-09

### Added
- **Relay transfer diagnostics.** New native counter module (`diag.rs`) plus
  `nativeDiagSnapshot()` and `scripts/relay-diag.sh`, which reads every
  attached adb device at once and prints a verdict. It separates the three
  causes of "audio isn't getting through" that look identical from one device:
  nothing arriving (wrong room / WS down / peer silent), arriving but not
  decrypting (peers on different session keys), and arriving fine (problem is
  downstream). The middle case was previously invisible - `packets_received`
  counts wire arrivals, not frames that survive AEAD, so two peers on
  mismatched keys showed healthy counters and silent audio.
- Snapshot is emitted to logcat under tag `SassyDiag` while diagnostics are
  enabled, deliberately instead of an exported broadcast receiver - that would
  be a standing surface leaking the session room id to any app on the device
  in exchange for a debugging convenience.

### Fixed
- **Diagnostics panel CAPTURE / CODEC sections could never update.** Their only
  producer was the Kotlin `PttAudioPipeline`, which is never constructed
  anywhere in the app - capture, codec and crypto all run in Rust, so Kotlin
  never saw a frame and the panel showed boot defaults forever. Now fed from
  the native snapshot (verified live: 149 frames in 3s = 50fps, Opus 29B/frame).
  GATE remains unfed - its producer is the same dead class - and needs the gate
  decision surfaced from Rust.
- **Flaky test masking a real failure.** `disabled_is_exact_noop` failed ~1 run
  in 3 because the NS enable/atten flags are process-global and parallel tests
  raced them. Serialising them made the suite deterministic and revealed that
  `denoise_improves_snr_on_noisy_tone` fails consistently: the 1kHz fundamental
  comes out ~41dB down against the 12dB the assertion allows. That test is now
  `#[ignore]`d with the finding recorded rather than loosened - noise
  suppression is user-facing and this needs a speech-like stimulus to judge.

## [3.1.18] (70) — 2026-08-08

### Changed
- **CI builds the native libs from source.** The release workflow never
  compiled Rust — it packaged whatever `libsassytalkie.so` happened to be
  committed, which made the checked-in binary the real release artifact and
  let x86_64 drift three releases behind (see 3.1.16). It also only triggered
  on `android-app/**`, so a change to the audio engine, transport or protocol
  produced no build at all. Now: triggers on `android-native/**` and `core/**`
  too, installs NDK 27.1.12297006 (pinned to Gradle's `ndkVersion`), runs the
  core unit tests, builds BOTH ABIs in one `cargo ndk` invocation, and fails
  the build if either ABI is missing or has lost an exported JNI symbol.
- **`ship.sh` refuses to ship a stale native lib.** New preflight: fails if
  any Rust source is newer than either committed `.so`, or if the APK/AAB
  predates the libs they are meant to contain — with the exact rebuild command
  in the error. Caught a real staleness case on its first run.

### Fixed
- **Idle battery drain.** Three loops woke far more often than they did work:
  the peer stale-check ticked every 1s even with zero peers tracked (3,600
  wakeups/hour to evaluate an empty set — now 5s while idle, snapping back to
  1s the moment a peer appears, well inside the 8s stale threshold); the
  transport-advisory loop woke every 2s but only acted on every 5th tick (now
  a plain 10s tick — same refresh rate, 1/5 the wakeups); and the cohort
  snapshotter crossed JNI for the full user list every 30s even with no
  session established (now early-outs on `isAuthenticated`).
- **JNI calls in the composition body.** `isEncrypted()` and
  `getTransportName()` were read directly during composition, so every
  recompose of the radio screen — pulse animation, TX/RX state, roster
  changes, emergency banners — crossed the JNI boundary. Hoisted into state
  refreshed on the advisory tick and on connection-state changes. The
  press-time `isEncrypted()` gate is deliberately left live: it authorises
  transmission and must not read a cached value.

## [3.1.17] (69) — 2026-08-08

### Fixed
- **CRITICAL: radio screen crashed on entry.** `LiveTranslationOverlay` was
  given `Modifier.padding(horizontal = (-16).dp)` to bleed past the parent
  Column's inset, but Compose's padding modifier rejects negative values and
  throws `IllegalArgumentException: Padding must be non-negative` when the
  modifier is *constructed* — so every composition of MainScreen died. Present
  since 6b8d717, shipped in **3.1.15 (67)** and **3.1.16 (68)**. Replaced with
  the supported idiom: a custom `layout` that measures wider and places itself
  back by the inset, reporting the original width so siblings are unaffected.
  Anyone on 3.1.15/3.1.16 should update.

### Added
- **Emergency / SOS beacons — the life-safety path is live.** `emergency.rs`
  had shipped as dead code since it was written: full codec, beacon cadence
  and man-down state machine with 22 passing tests, but no JNI binding and no
  UI, so pressing anything produced no wire frame. Now wired end to end.
  - Long-press "HOLD FOR SOS" on the radio screen raises a beacon (long-press
    only — a tap-to-fire distress control beside a 240dp PTT target would
    misfire constantly). Re-broadcasts on the core-owned 5s cadence until
    stood down; "I'M OK" sends the one-shot clear.
  - Inbound beacons render as a full-width red card above everything else,
    with sender, kind, age, note and coordinates when present.
  - **Payloads are AEAD-sealed with the session key.** Control frames
    otherwise cross the relay in cleartext, so an attached GPS fix would have
    handed the relay operator a live location feed for whoever is in distress.
    With no session key the beacon still goes out — a distress call is not
    gated on key state — but without coordinates, and the receiver labels it
    "unverified sender".
  - 12 instrumented tests exercise the real JNI + sealing on-device; 5 unit
    tests guard the opcode space.

### Changed
- **Opcode registry unified in `core/src/protocol.rs`.** `emergency.rs` had
  self-allocated OP_MANDOWN=0x1B / OP_EMERGENCY_CLEAR=0x1C after checking only
  the Rust constants — but Kotlin's `ControlFrame` held a second, larger
  registry that already used 0x1B/0x1C for the hybrid PQC handshake. Wiring
  emergency as written would have routed a man-down beacon into
  `handleHybridInit`, parsing a life-safety frame as a key exchange. Man-down
  moved to 0x1D, clear to 0x1E, every opcode now lives in one file, and
  `all_opcodes_are_unique` (Rust) + `EmergencyOpcodeTest` (Kotlin) guard both
  halves. Nothing shipped at the colliding values, so there is no interop
  break.

## [3.1.16] (68) — 2026-08-08

### Fixed
- **x86_64 devices shipped three releases of stale native code.** The committed
  `jniLibs/x86_64/libsassytalkie.so` was last built 2026-07-26, but the Rust
  sources changed in 3.1.13, 3.1.14, and 3.1.15 (BT RFCOMM gate, RX priority,
  adaptive jitter, comm-mode stay-hot release). Only the arm64-v8a lib was
  rebuilt for those releases, so Chromebooks, x86 tablets, and every emulator
  ran 3.1.12-era audio/transport code while reporting 3.1.15. Both ABIs are
  now rebuilt from the same tree in one `cargo ndk` invocation.

### Changed
- Fresh native build of `libsassytalkie.so` for arm64-v8a + x86_64. No Kotlin
  or Rust source changes — this release exists to correct the shipped binary.

## [3.1.15] (67) — 2026-08-01

### Fixed
- **"Takes up a phone line" while service runs.** WalkieService no longer holds
  `AudioFocusRequest(USAGE_VOICE_COMMUNICATION)` for its whole lifetime; that
  told Android a voice call was in progress, snapped the volume rocker to
  `STREAM_VOICE_CALL`, and blocked conference-call add-line + normal dialer
  routing. Focus is now requested on the first inbound audio frame and
  abandoned after 5 s of silence (stay-hot window keeps back-to-back "copy?"
  bursts snappy).
- **Same for `MODE_IN_COMMUNICATION` on Moto/Xiaomi.** The Rust RX thread
  released `AudioTrack` + comm-mode only when the whole session ended;
  now releases after 5 s of silence and re-engages on the next burst,
  matching the Kotlin stay-hot window so the phone stays "free" between
  transmissions on quirked devices.
- **VPN/backpressure stall latency.** Cellular WS retry ceiling cut 64 ms →
  15 ms (`SEND_RETRY_MAX` 8→3, `SEND_RETRY_SLEEP_MS` 8→5). Outbound queue's
  drop-oldest policy still handles loss shaping; pump now moves on 4× faster
  when the socket buffer is chronically full.

### Changed
- **Adaptive jitter visible.** `AudioCache.status_json` now exposes
  `jitter_base_frames`, `jitter_adaptive_extra`, `jitter_effective_frames`,
  and `jitter_ewma_ms`. Diagnostics dump + debug overlay show
  `base+extra=effective, ewma=Xms` so you can see the controller reacting.
- **Adaptive controller test.** Unit test proves
  `note_live_arrival_jitter` climbs under sustained 60 ms gaps and decays
  back to 0 under steady 20 ms arrivals.

### Deferred
- **Real UDP transport.** Cloudflare Workers still has no WebTransport
  server termination ([workerd#6454](https://github.com/cloudflare/workerd/discussions/6454)),
  so #1 from the "remove jitter" plan is unshippable here. Real UDP requires
  either WebRTC data channels (Android + iOS + Tauri) or moving the relay
  off Workers to a host that speaks HTTP/3 — both need explicit sign-off.

## [3.1.14] — 2026-07-29

### Fixed
- **Relay soft backpressure.** Durable Object no longer hard-closes sockets on
  the first rate overshoot or a single `peer.send` failure — soft-drops excess
  frames and only closes after sustained abuse / consecutive send failures.
  Text JSON pings now refresh DO liveness; alarm arming left the audio hot path.
- **Silent OkHttp TX drops.** Cellular outbound pump retries when
  `WebSocket.send` returns false (buffer full); previously ignored → looked
  like ~30% relay packet loss under cellular backpressure.
- **Diagnostics panel.** RX pkt/s wired; RTT/HB age from liveness (incl. relay
  peers); queue/ws drop counters + local drop %; jitter prebuffer shown.

### Changed
- Live jitter default **5 frames (100 ms)** + adaptive boost from inter-arrival
  EWMA; longer reorder wait (60 ms) and drain age (35 ms); cellular queues
  256 frames; dual-path congestion skip at ~75% full.
- **Encryption:** no weakening — dual-path already encrypts once; AES-GCM kept.

## [3.1.13] (65) — 2026-07-27

### Fixed
- **Bluetooth silent TX.** Failover/connected/PTT now require an RFCOMM data
  link (BLE alone is control-plane). RFCOMM retry + “Linking Bluetooth…” UI;
  PTT refuses with a clear reason when no audio path is ready.
- **Minimized loudspeaker jitter.** FGS holds AudioFocus, renews WakeLock on
  inbound RX, and bumps jitter prebuffer while backgrounded.
- **Share “Open in SassyTalk”.** Viewer uses App Link → custom scheme →
  intent:// with paste fallback (worker deploy required for live page).

### Changed
- iOS `MARKETING_VERSION` aligned to **3.1.13** / build **65**.
- GitHub Actions **Build iOS** now runs on published releases so macOS
  runners can produce the matching iOS build automatically.

## [3.1.6] (58) — 2026-07-15

### Fixed
- **Permission-screen crash on notification deny.** POST_NOTIFICATIONS (and
  Bluetooth) are now optional: startup gates only on mic + camera, the results
  callback no longer auto-re-requests (a permanently-denied permission answered
  instantly, looping the request until ANR), and a permanently-denied core
  permission routes to the app's Settings page instead of a dead button.
- **Dead PTT button while roster shows peers.** The on-screen PTT now falls
  back to the native IP pipeline when `PttCoordinator` is absent (Bluetooth
  off/unpermitted or entitlement not yet cached at init) — the same fallback
  the notification-shade toggle and hardware PTT already had. BT transport
  init also retries on entitlement unlock and Main-screen mount instead of
  being skipped once per session, and a rejected press now explains itself on
  the hint line instead of eating the touch.
- **Keyboard resize.** IME insets were applied twice (root `safeDrawing` +
  per-screen `imePadding`), shifting content by 2x the keyboard height; the
  root now excludes IME and `windowSoftInputMode="adjustResize"` pins the
  behavior across OEM defaults.
- **QR session screen.** Continue renders directly above the generated QR
  (was: below the share buttons inside a scrolling column — off-screen on
  phones), the inner scroll is gone, and the duplicate Active Session card is
  suppressed while the generated QR is on screen.
- **Paywall promo/license entry hidden by keyboard.** Both gate screens now
  scroll and imePadding so the code field rides above the IME (typed text was
  invisible under the keyboard); typed text is white on a filled container
  with a teal cursor for contrast.
- **Session crypto wiped by back-navigation.** The radio screen's back arrow
  called native `disconnect()`, which nulls the transport's AEAD session —
  re-entering via Continue then rejected every PTT press ("Authenticate via
  QR first") while presence still showed peers. Back is now pure navigation
  (End Session remains the hard teardown), and Continue re-arms crypto from
  the persisted channel session if a wipe already happened.
- **Debug builds re-locked onto the paywall.** `refresh()` lacked the debug
  entitlement bypass `isUnlockedCached` has, so the silent post-startup
  reconciliation flipped dev installs onto the gate a beat after launch.

### Added
- **Invite links inside the session.** The radio screen's Session QR dialog
  now has the same Copy Link / Share Link actions as the Auth screen — no
  more backing out of the session to mint an invite.

## [3.1.5] (57) — 2026-07-14

### Added
- **In-app diagnostics panel.** Settings toggle shows live relay room, cellular
  stats, peer counts, and copyable debug dump for remote join troubleshooting.

### Fixed
- **Remote invite links.** `sassytalk://` deep links, https invite URL import,
  and Enter Code tab paste fallback; screenshots allowed on auth/QR screens.
- **Relay join stability.** Disabled auto PQC on relay peers; zombie socket and
  transport-steal fixes; same-name self-echo no longer drops peer audio.

### Changed
- Fresh native rebuild (`libsassytalkie.so`) from `main` merge.

## [3.1.1] (52) — 2026-07-07

### Added
- **Play paywall promo redemption.** Friends & family can enter a promo code
  on the Play build paywall (relay `/license/promo`) without a Google Play
  purchase. Receipt refresh mirrors the direct-license offline model.

## [3.1.0] (51) — 2026-07-07

### Added
- **Picture-in-picture.** Leaving the main radio screen while connected
  auto-enters PiP with channel, transport, and TX/RX status.
- **Efficient QR rendering.** Shared `QrBitmap` utility generates at display
  size with bulk pixel writes instead of oversized `setPixel` loops.

### Changed
- **Edge-to-edge (Android 15).** API 35 theme disables contrast scrims,
  cutout mode is `always`, navigation bar uses dark icons, and insets are
  applied once at the activity root (no double-padding on child screens).
- **Predictive back** enabled for Android 13+.
- Removed dead legacy XML layout, unused drawables, and WalkieService
  network-type polling (badge was removed earlier).
- Profile and auth screens use `imePadding()` for keyboard overlap.
- PiP overlay live-updates channel/transport; auto-enter enabled on Android 12+.
- End-session and entitlement refresh use coordinator-safe threading.
- **Android 12+ splash screen** with branded slate background on cold start.
- Removed unused AppCompat/Material/ConstraintLayout dependencies (~legacy XML).

## [3.0.2] (50) — 2026-07-07

### Fixed
- **Paywall infinite loading.** Play billing catalog load now times out after
  15s, surfaces errors when the unlock product is missing or Play is
  unavailable, and offers Retry / Restore instead of a stuck "Loading…" button.

## [3.0.1] (49) — 2026-07-07

### Fixed
- **PTT orchestration.** All transmit paths (on-screen, notification shade,
  hardware key, Bluetooth media button) now route through `PttCoordinator` so
  BLE wake, RFCOMM pump, and reach watchdog run consistently.
- **"Not reaching peer" false alarm.** Main screen now uses the watchdog's
  `peerReachFailed` signal instead of an inverted reach indicator.
- **Users tab offline labels.** Registry peers without heartbeats yet show
  "On channel"; "Out of contact" only when liveness tracking confirms STALE.
- **Relay WebSocket leak.** Reconnecting cellular relay tears down the
  previous client before opening a new one.
- **Peer registry eviction.** Peers are removed from the user registry when
  they leave the channel.
- **UI polish.** Centralized cache-status polling, QR generation off the main
  thread, fixed snackbar overlay layout, hold-vs-tap PTT hint text, and
  deduplicated incoming-audio vs cache strip display.

## [3.0.0] (48) — 2026-07-07

### Added
- **Transport Advisor.** The app now scores available encrypted audio planes
  (WiFi multicast, Cloudflare relay, Bluetooth) and tells you which path is
  active and whether a better one is reachable. When cellular is down, it
  confirms Bluetooth fallback; when WiFi returns, it advises switching for
  lower latency. Advisories appear inline on the main screen and as toasts on
  meaningful path changes.
- **Active audio-plane badge.** Connection status now shows the encrypted PTT
  transport plane (not just the OS network type), color-coded: green = WiFi,
  orange = relay, cyan = Bluetooth.

### Changed
- Consolidates the v2.9.0 paywall (Play IAP + direct license key) with the
  full multi-transport encrypted PTT stack into the shipping 3.0 release.

## [2.9.0] (47) — 2026-07-03

### Added
- **Distribution flavors.** The app now builds as two variants with identical
  features and the same `applicationId`:
  - **Play** — free install with a one-time in-app unlock
    (`sassytalkie_unlock`) through Google Play Billing. Purchases restore
    automatically on reinstall or a second device; refunds revoke on the next
    online launch.
  - **Direct** (website APK) — unlocks with a license key or promo code.
    One license covers up to 3 devices; activation issues a 30-day offline
    receipt that silently renews whenever the app is online, so off-grid use
    keeps working.
- **License service** on the relay worker (`/license/*`): activation,
  revalidation, device deactivation, plus operator endpoints for issuing and
  revoking keys. Keys and device ids are stored only as salted HMACs — the
  server never persists a raw key. Keys carry 100 bits of CSPRNG entropy.
- **Promo codes**: shareable codes with redemption caps and optional expiry,
  entered in the same activation field. Device-bound and idempotent — a
  re-entry refreshes the unlock instead of consuming another redemption.
- **Verified App Links**: `assetlinks.json` now serves the release signing
  cert, so `https://relay.sassyconsultingllc.com/v/…` invite links open the
  sideloaded app directly instead of the browser landing page.

### Changed
- Invite deep-links imported while the app is locked now hold at the unlock
  screen (the session still imports and is live immediately after unlock).
- **Edge-to-edge display** using the modern API: content draws full-bleed
  behind transparent system bars with proper inset handling (fixes the Play
  Console "deprecated edge-to-edge APIs" and "may not display for all users"
  advisories on Android 15+).
- **Rotation and large screens**: the portrait lock is gone — the app now
  rotates and resizes on tablets, foldables, and split-screen. A live session
  (audio engine, transport, PTT) survives rotation without interruption.
- Release artifacts are flavor-qualified: website APK =
  `app-direct-release.apk`, Play AAB = `app-play-release.aab`; CI and the
  ship pipeline updated to match.

### Security
- Entitlement state lives in Android-Keystore-encrypted storage, same as
  session keys.
- License endpoints fail closed when server secrets are unconfigured.

## [2.8.1] (46) — 2026-06-25

### Added
- **Cross-platform invite links.** `https://relay…/v/<id>#<key>` now imports
  on Android, iOS, and desktop. The decryption key rides in the URL fragment
  and never reaches the server; the shared Rust core performs one audited
  AES-256-GCM open on every platform (byte-parity proven against Android's
  Java encrypter with an independent known-answer vector).
- Browser landing page for recipients without the app (`/v/<id>`): open-app /
  install chooser that resolves the key entirely client-side.
- `/.well-known/assetlinks.json` and `apple-app-site-association` served from
  the relay so tapped invites can open the app directly (Universal/App Links).
- Desktop: paste an invite link straight into the join field — auto-detected
  vs raw QR JSON. iOS: `onOpenURL` handler + associated-domains entitlement.

### Fixed
- Android share-link generation aligned to the worker `/share` contract —
  fixes the "no token" copy-link failure and dead invite links.

## [2.7.6] (45) — 2026-06-23

Baseline for this changelog: 16 KB page-size support, hybrid post-quantum
key exchange (X25519 + ML-KEM) with auto-upgrade on 2-party sessions,
WiFi → relay → Bluetooth transport fallback with auto-recovery, sealed-sender
blinding, group mix-mode, live transcription feed. Earlier history lives in
the git log.
