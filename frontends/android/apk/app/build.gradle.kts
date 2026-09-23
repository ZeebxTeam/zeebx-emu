plugins {
    id("com.android.application")
}

// Não há fonte Java nem Kotlin neste módulo: o aplicativo inteiro é o `.so` que o
// `compilar.sh` deixa em `src/main/jniLibs`. O Gradle aqui só empacota, assina e alinha.
android {
    namespace = "io.github.zeebxteam.zeebx"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.zeebxteam.zeebx"
        // O AAudio, que é a saída de som do `cpal` no Android, chega no 26.
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "0.3.0"
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
            // O `.so` fica descompactado e alinhado à página, que é o que o Android 15 pede e
            // o que deixa o sistema mapeá-lo direto do APK, sem cópia.
            useLegacyPackaging = false
        }
    }
}
