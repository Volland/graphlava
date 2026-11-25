// The Kotlin bindings of graphlava ([[16-api#Mobile bindings]]):
//   :graphlava          the Kotlin API over the C FFI (JNI), tested on the JVM host
//   :graphlava-android  the AAR org.graphlava:graphlava-android (arm64-v8a, x86_64,
//                       16 KB pages) with the ProcessLifecycleOwner hook
// Pass -Pgraphlava.android=false to build without the Android SDK.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "graphlava-kotlin"
include(":graphlava")
if (providers.gradleProperty("graphlava.android").orNull != "false") {
    include(":graphlava-android")
}
