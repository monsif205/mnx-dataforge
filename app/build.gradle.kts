import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.monex.dataforge"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.monex.dataforge"
        minSdk = 35
        targetSdk = 36
        // CI passes the run number, so every build has a higher versionCode and can update the previous one.
        val code = (System.getenv("VERSION_CODE") ?: "1").toInt()
        versionCode = code
        versionName = "0.2.$code"
        // Samsung A06 and nearly all current phones are arm64. One ABI keeps the APK small.
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    signingConfigs {
        // Your own release key (from GitHub secrets). Same key on every build = updates install over the old app.
        val ks = System.getenv("KEYSTORE_FILE")
        if (ks != null && File(ks).exists()) {
            create("release") {
                storeFile = File(ks)
                storePassword = System.getenv("KEYSTORE_PASSWORD")
                keyAlias = System.getenv("KEY_ALIAS") ?: "mnx"
                keyPassword = System.getenv("KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Own key when the secrets exist, otherwise the debug key (installs, but cannot update a release-signed app).
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures { compose = true }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    // Compose is pinned explicitly (no BOM) because the liquid-glass library needs Compose 1.10.x.
    val compose = "1.10.3"
    implementation("androidx.compose.ui:ui:$compose")
    implementation("androidx.compose.ui:ui-graphics:$compose")
    implementation("androidx.compose.foundation:foundation:$compose")
    implementation("androidx.compose.animation:animation:$compose")
    // Real liquid glass (blur + refraction lens). All usage is isolated in ui/Glass.kt.
    implementation("io.github.kyant0:backdrop:1.0.6")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.0")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
}
