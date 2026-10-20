plugins { alias(libs.plugins.androidLibrary) }
android {
    namespace = "io.github.nuxoul.nxwallhaven.shared"
    compileSdk { version = release(37) }
    defaultConfig { minSdk = 26 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
}
dependencies {
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation(libs.kotlinx.coroutines.android)
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250517")
}
