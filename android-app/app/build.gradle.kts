// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-DYFRFW6D26JA
import java.util.Properties
import java.io.FileInputStream

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Apply Google Services plugin only when google-services.json is checked in,
// so devs without Firebase access can still build. Activates FCM glue.
if (file("google-services.json").exists()) {
    apply(plugin = "com.google.gms.google-services")
}

android {
    namespace = "com.sassyconsulting.sassytalkie"
    compileSdk = 36
    // Pin to an NDK that is actually installed. AGP's default for this version
    // is 27.0.12077973 (not present); 27.1.12297006 is installed.
    ndkVersion = "27.1.12297006"

    defaultConfig {
        applicationId = "com.sassyconsulting.sassytalkie"
        minSdk = 24
        // Play requires targetSdk 36 for app updates from 31 August 2026.
        targetSdk = 36
        versionCode = 83
        versionName = "3.2.7"
        
        // Feature flag: enable or disable cellular (relay) transport at build time
        buildConfigField("boolean", "ENABLE_CELLULAR_RELAY", "true")
        // Debug builds allow screen capture so we can show off the UI / take
        // screenshots; release overrides this to true (FLAG_SECURE engaged)
        // so production builds can't be screen-recorded or screenshotted.
        // Override per-buildType is in the buildTypes blocks below.
        buildConfigField("boolean", "NO_SCREENSHOTS", "false")
        // Relay worker license endpoints (direct activation + Play promo redemption).
        buildConfigField("String", "LICENSE_API_BASE", "\"https://relay.sassyconsultingllc.com\"")

        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    // Distribution flavors. Same applicationId for both so a user can migrate
    // website APK → Play without losing data; the entitlement gate is the only
    // code that differs (see src/play/... and src/direct/... Entitlements.kt).
    flavorDimensions += "dist"
    productFlavors {
        create("play") {
            dimension = "dist"
            isDefault = true
        }
        create("direct") {
            dimension = "dist"
            versionNameSuffix = "-direct"
        }
    }

    // CMake builds libsassytalkie_opus.so for the new audio.OpusEncoder.
    // Falls back to a stub library when libopus prebuilts aren't vendored —
    // see src/main/cpp/CMakeLists.txt for details.
    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    signingConfigs {
        create("release") {
            // Credentials come from one of two places. CI uses env vars (which
            // the GitHub Actions runner injects from secrets); local devs use
            // app/keystore.properties (gitignored). Either path keeps the
            // password out of the committed source tree.
            val ksFile = file("keystore/release.keystore")
            val ksPropsFile = file("keystore.properties")
            val ksProps = Properties()
            if (ksPropsFile.exists()) {
                FileInputStream(ksPropsFile).use { input -> ksProps.load(input) }
            }
            fun cred(envKey: String, propKey: String): String? {
                val fromEnv = System.getenv(envKey)
                if (!fromEnv.isNullOrBlank()) return fromEnv
                val fromFile = ksProps.getProperty(propKey)
                return if (!fromFile.isNullOrBlank()) fromFile else null
            }

            val storePw = cred("RELEASE_STORE_PASSWORD", "storePassword")
            val alias   = cred("RELEASE_KEY_ALIAS",      "keyAlias")
            val keyPw   = cred("RELEASE_KEY_PASSWORD",   "keyPassword")
            if (ksFile.exists() && storePw != null && alias != null && keyPw != null) {
                storeFile = ksFile
                storePassword = storePw
                keyAlias = alias
                keyPassword = keyPw
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            // Production builds enforce FLAG_SECURE → screenshots and screen
            // recording are blocked. The default in defaultConfig is false so
            // debug builds can be captured for marketing / bug reports.
            buildConfigField("boolean", "NO_SCREENSHOTS", "true")
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            // Mirror the same env-OR-keystore.properties logic the signingConfig uses.
            val ksPropsForCheck = Properties()
            val ksPropsForCheckFile = file("keystore.properties")
            if (ksPropsForCheckFile.exists()) {
                FileInputStream(ksPropsForCheckFile).use { input -> ksPropsForCheck.load(input) }
            }
            fun hasCred(envKey: String, propKey: String): Boolean =
                !System.getenv(envKey).isNullOrBlank() ||
                !ksPropsForCheck.getProperty(propKey).isNullOrBlank()
            val hasReleaseCreds = file("keystore/release.keystore").exists() &&
                hasCred("RELEASE_STORE_PASSWORD", "storePassword") &&
                hasCred("RELEASE_KEY_ALIAS",      "keyAlias") &&
                hasCred("RELEASE_KEY_PASSWORD",   "keyPassword")
            // Allow local devs to opt into a debug-signed release build with
            // ALLOW_DEBUG_SIGNED_RELEASE=1, but never silently — and never on CI.
            val allowDebugSigned = System.getenv("ALLOW_DEBUG_SIGNED_RELEASE") == "1"
            val onCi = System.getenv("CI") == "true" || !System.getenv("GITHUB_ACTIONS").isNullOrBlank()
            // Only enforce credential checks if a release-producing task was actually requested.
            // Otherwise `clean`, `assembleDebug`, IDE syncs, etc. would fail configuration.
            val requestedTasks = gradle.startParameter.taskNames.joinToString(" ").lowercase()
            val isReleaseRequested = listOf("assemblerelease", "bundlerelease", "installrelease", "packagerelease")
                .any { requestedTasks.contains(it) }
            signingConfig = when {
                hasReleaseCreds -> signingConfigs.getByName("release")
                allowDebugSigned -> {
                    logger.warn("ALLOW_DEBUG_SIGNED_RELEASE=1 — signing release with the DEBUG keystore. NEVER upload this AAB.")
                    signingConfigs.getByName("debug")
                }
                isReleaseRequested && onCi -> error(
                    "Release build on CI without release signing credentials. " +
                    "Set RELEASE_KEYSTORE_BASE64 / RELEASE_STORE_PASSWORD / " +
                    "RELEASE_KEY_ALIAS / RELEASE_KEY_PASSWORD secrets and re-run."
                )
                isReleaseRequested -> error(
                    "Release build without signing credentials. Set RELEASE_STORE_PASSWORD / " +
                    "RELEASE_KEY_ALIAS / RELEASE_KEY_PASSWORD env vars (and ensure " +
                    "app/keystore/release.keystore exists), or set ALLOW_DEBUG_SIGNED_RELEASE=1 " +
                    "for a throwaway debug-signed local build."
                )
                else -> signingConfigs.getByName("debug")  // safe default for non-release tasks
            }
        }
    }
    
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        // Enable generation of BuildConfig fields used for feature flags
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    // Play flavor only: Google Play Billing for the one-time unlock purchase.
    // The direct flavor ships zero Google billing code.
    "playImplementation"("com.android.billingclient:billing-ktx:9.1.0")

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.core:core-splashscreen:1.0.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.6.2")
    implementation("androidx.lifecycle:lifecycle-process:2.6.2")
    implementation("androidx.activity:activity-compose:1.9.3")
    
    // Compose
    implementation(platform("androidx.compose:compose-bom:2023.10.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    
    // Icons
    implementation("androidx.compose.material:material-icons-extended")

    // QR Code generation
    implementation("com.google.zxing:core:3.5.2")

    // QR Code scanning — UNBUNDLED ML Kit barcode. The bundled
    // com.google.mlkit:barcode-scanning ships libbarhopper_v3.so at 4 KB ELF
    // alignment (frozen at 17.3.0, never fixed), which fails Play's 16 KB
    // page-size requirement. The play-services variant delivers the model via
    // Google Play Services, so no .so lands in our APK. Same BarcodeScanning API.
    implementation("com.google.android.gms:play-services-mlkit-barcode-scanning:18.3.1")

    // On-device translation — ML Kit Translate. Powers translate/TranslationManager
    // for offline, privacy-preserving real-time translation (no cloud). Models are
    // downloaded per-language on first use, then translation runs fully on-device.
    //
    // ⚠️ 16 KB page-size CAVEAT — unlike the barcode scanner above, ML Kit Translate
    // has NO unbundled play-services variant: this dependency BUNDLES native .so
    // files (TFLite / language-id) into our APK. Google Play now requires all .so
    // segments be 16 KB-aligned. Before release, VALIDATE the bundled translate .so
    // alignment (NDK check_elf_alignment.sh, or `objdump -p <lib>.so | grep LOAD`).
    // If any segment is < 16 KB-aligned, bump the translate version until it is.
    implementation("com.google.mlkit:translate:17.0.3")

    // CameraX for QR scanner — 1.4.0+ aligns libimage_processing_util_jni.so to 16 KB.
    // Stay on 1.4.2: it already meets Play's 16 KB page-size rule; 1.6.x is a
    // separate upgrade once compileSdk 36 is no longer the only gate.
    implementation("androidx.camera:camera-camera2:1.4.2")
    implementation("androidx.camera:camera-lifecycle:1.4.2")
    implementation("androidx.camera:camera-view:1.4.2")

    // JSON parsing
    implementation("org.json:json:20231013")

    // OkHttp for WebSocket (cellular relay transport)
    implementation("com.squareup.okhttp3:okhttp:4.12.0")

    // Firebase Cloud Messaging — wake-push fallback when the relay sees a
    // session-room peer with no active WS at PTT-start. Pinned to a BoM so
    // all firebase-* libs share a tested version set.
    implementation(platform("com.google.firebase:firebase-bom:33.5.1"))
    implementation("com.google.firebase:firebase-messaging-ktx")

    // Android Keystore-backed EncryptedSharedPreferences for session/key storage.
    implementation("androidx.security:security-crypto:1.1.0-alpha06")

    debugImplementation("androidx.compose.ui:ui-tooling")

    testImplementation("junit:junit:4.13.2")

    androidTestImplementation("androidx.test.ext:junit:1.1.5")
    androidTestImplementation("androidx.test:runner:1.5.2")
}

