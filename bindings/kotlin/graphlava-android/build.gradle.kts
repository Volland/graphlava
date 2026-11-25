// org.graphlava:graphlava-android: the Kotlin API of :graphlava plus the
// Android lifecycle hook, with libgraphlava.so for arm64-v8a and x86_64 linked
// for 16 KB pages (scripts/build-android-libs.sh, gated by
// scripts/check-16k-alignment.sh).
plugins {
    id("com.android.library")
    kotlin("android")
    `maven-publish`
}

android {
    namespace = "org.graphlava.android"
    compileSdk = 34
    defaultConfig {
        minSdk = 24
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    sourceSets["main"].java.srcDirs("../graphlava/src/main/kotlin", "src/main/kotlin")
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    packaging { jniLibs { useLegacyPackaging = false } }
    publishing { singleVariant("release") }
}

dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation("androidx.lifecycle:lifecycle-process:2.8.7")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("org.jetbrains.kotlin:kotlin-test:2.1.21")
}

// The native libraries: built with the NDK, then checked for 16 KB alignment.
val nativeLibs by tasks.registering(Exec::class) {
    inputs.file("../graphlava/src/main/cpp/graphlava_jni.c")
    inputs.file("../../../crates/graphlava-ffi/include/graphlava.h")
    outputs.dir("src/main/jniLibs")
    commandLine(rootDir.resolve("scripts/build-android-libs.sh"))
}
val alignmentGateTest by tasks.registering(Exec::class) {
    commandLine(rootDir.resolve("scripts/test-alignment-check.sh"))
}
tasks.named("preBuild") { dependsOn(nativeLibs) }
tasks.named("check") { dependsOn(alignmentGateTest) }

publishing {
    publications {
        register<MavenPublication>("release") {
            groupId = "org.graphlava"
            artifactId = "graphlava-android"
            afterEvaluate { from(components["release"]) }
        }
    }
}
