import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.github.freecamphone"
    compileSdk = 36
    defaultConfig {
        applicationId = "io.github.freecamphone"
        minSdk = 26
        targetSdk = 36
        versionCode = (project.findProperty("versionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("versionName") as String?) ?: "0.1.0"
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // ponytail: debug-key signed so CI releases install as-is; add a real keystore via signingConfigs for Play Store.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { jvmTarget.set(JvmTarget.JVM_17) } }

// Builds the Rust core (crates/phone-core) into jniLibs for every ABI.
val cargoNdk by tasks.registering(Exec::class) {
    workingDir = rootProject.file("..")
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "armeabi-v7a", "-t", "x86_64", "-P", "26",
        "-o", "android/app/src/main/jniLibs", "build", "--release", "-p", "phone-core",
    )
}
tasks.named("preBuild") { dependsOn(cargoNdk) }
