import java.util.zip.ZipFile

plugins {
    alias(libs.plugins.androidApplication)
    alias(libs.plugins.composeMultiplatform)
    alias(libs.plugins.composeCompiler)
}

android {
    namespace = "io.github.nuxoul.nxwallhaven"
    compileSdk { version = release(37) }
    defaultConfig {
        applicationId = "io.github.nuxoul.nxwallhaven"
        minSdk = 26
        targetSdk = 36
        versionCode = 2
        versionName = "0.1.0-alpha02"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
    packaging { jniLibs { useLegacyPackaging = false } }
    buildTypes { release { isMinifyEnabled = false } }
}

dependencies {
    implementation(project(":shared"))
    implementation(libs.androidx.activity.compose)
    implementation(compose.runtime)
    implementation(compose.foundation)
    implementation(compose.material3)
    implementation(compose.materialIconsExtended)
    implementation(compose.ui)
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    implementation(libs.kotlinx.coroutines.android)
    implementation("io.coil-kt.coil3:coil-compose:3.6.3")
    implementation("com.davemorrissey.labs:subsampling-scale-image-view:3.10.0") {
        exclude(group = "com.android.support")
    }
    implementation("androidx.core:core-ktx:1.18.0")
    implementation("androidx.concurrent:concurrent-futures:1.2.0")
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    implementation("org.rustls:rustls-platform-verifier:0.2.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4:1.12.1")
    androidTestImplementation("androidx.test.uiautomator:uiautomator:2.3.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest:1.12.1")
}

// Preserve the verifier's signed-CRL HTTP allowlist; debug adds localhost only.
val verifierRules = configurations.create("verifierRules") { isTransitive = false }
dependencies { add(verifierRules.name, "org.rustls:rustls-platform-verifier:0.2.0@aar") }
val generateDebugNetworkRules = tasks.register("generateDebugNetworkRules") {
    inputs.files(verifierRules)
    val output = layout.projectDirectory.file("src/debug/res/xml/network_security_debug.xml")
    outputs.file(output)
    doLast {
        ZipFile(verifierRules.singleFile).use { archive ->
            val xml = archive.getInputStream(archive.getEntry("res/xml/network_security_config.xml")).bufferedReader().use { it.readText() }
            check(xml.contains("cleartextTrafficPermitted=\"false\"") && xml.contains("crl.pki.goog"))
            check(xml.split("</network-security-config>").size == 2)
            val local = "<domain-config cleartextTrafficPermitted=\"true\"><domain includeSubdomains=\"false\">127.0.0.1</domain></domain-config>"
            output.asFile.parentFile.mkdirs()
            output.asFile.writeText(xml.replace("</network-security-config>", local + "</network-security-config>"))
        }
    }
}
tasks.configureEach {
    if (name == "preDebugBuild" || name == "generateDebugResources" || name == "packageDebugResources") dependsOn(generateDebugNetworkRules)
}
