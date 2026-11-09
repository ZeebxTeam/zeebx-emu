plugins {
    id("com.android.application")
}

// There is no Java or Kotlin source in this module: the whole app is the `.so` that
// `build.sh` leaves in `src/main/jniLibs`. Gradle here only packages, signs and aligns.
android {
    namespace = "io.github.zeebxteam.zeebx"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.zeebxteam.zeebx"
        // AAudio, which is `cpal`'s sound output on Android, arrives in 26.
        minSdk = 26
        targetSdk = 35
        versionCode = 8
        versionName = "0.5.1"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    packaging {
        jniLibs {
            // The `.so` stays uncompressed and page-aligned, which is what Android 15 asks for
            // and what lets the system map it straight from the APK, without a copy.
            useLegacyPackaging = false
        }
    }
}
