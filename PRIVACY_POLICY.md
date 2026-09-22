<!--
   Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
   Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
   CodeMark: SCLLC1-sassytalkie-MROFUXGIE3VP
-->
# Privacy Policy for Sassy-Talk (SassyTalkie)

**Last Updated:** September 22, 2026

**Effective Date:** January 14, 2025

---

## Overview

Sassy-Talk ("the App") is a secure Push-to-Talk (PTT) walkie-talkie application developed by Sassy Consulting LLC. Audio is encrypted end-to-end on your device. This Privacy Policy explains what infrastructure the App may contact and what it does not collect.

---

## How communication works

Local Bluetooth / Wi‑Fi multicast paths stay on your LAN and do not contact Sassy Consulting servers. When you use the **cellular relay**, encrypted audio ciphertext is forwarded through a Cloudflare Worker at `relay.sassyconsultingllc.com`. The relay cannot decrypt your audio.

Optional **wake notifications** (Android) may register a Firebase Cloud Messaging (FCM) token with the relay so an offline peer can be woken; the push carries a room id only — never audio. Delivery uses Google FCM.

Paid unlocks use **Google Play Billing** (Play builds) or a **license API** on the relay (direct APK). We do not create user accounts or store conversation recordings.

We do **not**: sell your data, serve ads, access your contacts, or claim there are “no central servers / no third parties” — Cloudflare (relay) and, when enabled, Google (FCM / Play) are involved as described above.

---

## Information We DO NOT Collect

- Names, email addresses, or phone numbers as account PII (license checkout email, if any, is handled by the payment provider / license flow — not stored as chat identity)
- Conversation history or voice recordings on our servers
- Location tracking beyond what Cloudflare connection routing may see as an IP
- Analytics SDKs or advertising identifiers for marketing
- Cookies in the native apps

---

## Permissions Required

The App requires the following Android permissions to function:

### 🎤 RECORD_AUDIO
- **Purpose:** Capture your voice when you press the Push-to-Talk button
- **Usage:** Audio is transmitted directly to your connected peer device via Bluetooth
- **Storage:** Audio is NOT saved to your device or transmitted to any server

### 📡 BLUETOOTH / BLUETOOTH_ADMIN / BLUETOOTH_CONNECT / BLUETOOTH_SCAN
- **Purpose:** Establish peer-to-peer connections with other devices running Sassy-Talk
- **Usage:** Connect to paired Bluetooth devices for direct communication
- **Data:** Only paired device names and MAC addresses are accessed (standard Bluetooth pairing)

### 🌐 INTERNET (Optional)
- **Purpose:** Reserved for future features (currently unused)
- **Usage:** No internet communication occurs in the current version
- **Note:** The app functions fully without an internet connection

### 📶 ACCESS_WIFI_STATE / CHANGE_WIFI_MULTICAST_STATE
- **Purpose:** Reserved for future WiFi Direct features (currently unused)
- **Usage:** No WiFi data is currently transmitted

---

## How Your Data is Handled

### Peer-to-Peer Architecture
All voice communication occurs **directly between your device and your peer's device** using Bluetooth RFCOMM protocol. There are no intermediary servers.

### Local Processing Only
- Voice data is captured from your microphone
- Encoded on your device
- Transmitted directly via Bluetooth to the paired device
- Received and decoded on the peer's device
- Played through the peer's speaker

### No Storage
- Audio is processed in real-time and discarded immediately
- No recordings are saved
- No conversation logs are kept
- No connection history is stored (beyond Android's standard Bluetooth pairing cache)

### Encryption
- All voice data transmitted between devices is **encrypted** using industry-standard AES-256-GCM encryption
- Encryption keys are generated locally and never leave your device
- End-to-end encryption ensures only your paired device can decrypt your voice

---

## Third-Party Services

**We do not use any third-party services, SDKs, or analytics platforms.**

The App is built entirely with:
- Rust programming language
- Android native APIs
- No external libraries that collect data

---

## Children's Privacy

Sassy-Talk does not knowingly collect any personal information from children under 13 years of age. Since the App collects no personal data whatsoever, it is inherently compliant with COPPA (Children's Online Privacy Protection Act).

---

## Data Security

While we do not collect or store your data, we take security seriously:

- ✅ **End-to-end encryption** for all voice transmissions
- ✅ **Local-only processing** - no cloud storage
- ✅ **Open-source architecture** - code can be audited
- ✅ **Minimal permissions** - only essential Android permissions requested
- ✅ **No user accounts** - no authentication data to compromise

---

## Your Rights

Since we do not collect or store any personal data:
- There is no data to access, correct, or delete
- There are no profiles to manage
- No opt-out procedures are necessary

You can uninstall the App at any time to completely remove it from your device.

---

## Changes to This Privacy Policy

We may update this Privacy Policy from time to time. We will notify you of any changes by:
- Posting the new Privacy Policy on our website
- Updating the "Last Updated" date at the top of this document
- Providing in-app notification (if applicable)

Your continued use of the App after any changes indicates your acceptance of the updated Privacy Policy.

---

## Compliance

This Privacy Policy is designed to comply with:
- 🇪🇺 **GDPR** (General Data Protection Regulation)
- 🇺🇸 **CCPA** (California Consumer Privacy Act)
- 🇺🇸 **COPPA** (Children's Online Privacy Protection Act)
- 📱 **Google Play Store** data safety requirements
- 🍎 **Apple App Store** privacy requirements

---

## Technical Details (For Transparency)

### Data Flow
```
[Your Microphone] 
    ↓ (Local capture)
[Audio Encoding]
    ↓ (AES-256 encryption)
[Bluetooth RFCOMM]
    ↓ (Direct transmission)
[Peer Device]
    ↓ (Decryption)
[Peer Speaker]
```

### What Happens to Your Voice
1. Captured by microphone when PTT pressed
2. Encoded into digital format
3. Encrypted with AES-256-GCM
4. Transmitted via Bluetooth
5. Decrypted on peer device
6. Played through speaker
7. **Discarded** (no storage)

---

## Contact Us

If you have questions about this Privacy Policy or the App's data practices:

**Sassy Consulting LLC**
- Website: https://saukprairieriverview.com
- Email: contact@sassyconsulting.com (replace with your actual email)
- Address: [Your Business Address]

---

## Consent

By downloading, installing, or using Sassy-Talk, you acknowledge that:
1. You have read and understood this Privacy Policy
2. You agree to the collection and use of information as described
3. You understand the App operates peer-to-peer with no central servers
4. You grant the necessary Android permissions for the App to function

---

## Summary for Google Play Store

**Data Safety Section Summary:**

- ✅ **Data Collected:** None
- ✅ **Data Shared:** None
- ✅ **Data Transmitted:** Encrypted voice (peer-to-peer only, no servers)
- ✅ **Data Stored:** None
- ✅ **Encryption:** Yes (AES-256-GCM end-to-end)
- ✅ **Can Request Data Deletion:** N/A (no data collected)
- ✅ **Third-Party Data Sharing:** None

**Permissions Justification:**
- `RECORD_AUDIO`: Core functionality (voice communication)
- `BLUETOOTH_*`: Core functionality (peer connection)
- `INTERNET`: Declared but unused (reserved for future features)

---

*This privacy policy is effective as of January 14, 2025, and applies to all versions of Sassy-Talk distributed through official channels.*

---

## Legal Disclaimer

Sassy-Talk is provided "as is" without warranties of any kind. While we implement strong encryption and security practices, users should be aware that:
- Bluetooth connections have a limited range (~10-100 meters)
- Physical proximity to your communication partner is required
- Standard Bluetooth security limitations apply
- Users are responsible for complying with local laws regarding radio communications

---

**Version:** 1.0.0  
**Policy Version:** 1.0  
**Last Reviewed:** January 14, 2025
