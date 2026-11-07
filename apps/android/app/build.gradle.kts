plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The version follows the workspace (Cargo.toml [workspace.package]).
val workspaceVersion: String = rootDir.resolve("../../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }
    .substringAfter('"').substringBefore('"')
val versionParts = workspaceVersion.split('.').map { it.toInt() }
// Shown in the interface ("0.3.3 (abc1234)") so testers can tell builds apart.
val buildCommit: String = runCatching {
    val process = ProcessBuilder("git", "rev-parse", "--short", "HEAD").directory(rootDir).start()
    process.inputStream.bufferedReader().readText().trim().also { process.waitFor() }
}.getOrDefault("").ifEmpty { "dev" }

android {
    namespace = "com.hachiwii.oink.android"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "com.hachiwii.oink.android"
        minSdk = 26
        targetSdk = 35
        versionCode = versionParts[0] * 10000 + versionParts[1] * 100 + versionParts[2]
        versionName = "$workspaceVersion ($buildCommit)"
    }

    buildTypes {
        release {
            // JNI looks up callback methods by name; keep the app unobfuscated.
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        buildConfig = true
    }
    packaging {
        jniLibs.useLegacyPackaging = false
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.webkit:webkit:1.13.0")
}
