import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("jvm")
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.3")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
    testImplementation("org.json:json:20240303")
}

val repo = rootDir.resolve("../..").canonicalFile
val hostJniDir = layout.buildDirectory.dir("host-jni")

// The JNI library for host tests: libgraphlava_ffi.a + the shim.
val hostJni by tasks.registering(Exec::class) {
    inputs.file("src/main/cpp/graphlava_jni.c")
    inputs.file(repo.resolve("crates/graphlava-ffi/include/graphlava.h"))
    outputs.dir(hostJniDir)
    environment("JAVA_HOME", System.getProperty("java.home"))
    commandLine(rootDir.resolve("scripts/build-host-jni.sh"), hostJniDir.get().asFile)
}

tasks.test {
    useJUnitPlatform()
    dependsOn(hostJni)
    val ext = if (System.getProperty("os.name").startsWith("Mac")) "dylib" else "so"
    systemProperty("graphlava.library", hostJniDir.get().file("libgraphlava.$ext").asFile.path)
    systemProperty("graphlava.repo", repo.path)
    environment("PATH", System.getenv("PATH") + ":" + System.getProperty("user.home") + "/.elan/bin")
    testLogging { events("passed", "skipped", "failed"); showStandardStreams = false; exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL }
}
