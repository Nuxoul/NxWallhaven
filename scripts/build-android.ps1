param([switch]$SkipNative,[switch]$Tests,[string]$JavaHome)
$ErrorActionPreference='Continue' # Cargo progress is stderr; native exit codes are checked explicitly.
$root=Split-Path -Parent $PSScriptRoot
$previousJava=$env:JAVA_HOME
$previousNdk=$env:ANDROID_NDK_HOME
$previousFlags=$env:RUSTFLAGS
try {
    if (!$JavaHome) {
        foreach ($candidate in @('D:\Program Files\Android\Android Studio\jbr','C:\Program Files\Android\Android Studio\jbr',$env:JAVA_HOME)) {
            if ($candidate -and (Test-Path (Join-Path $candidate 'bin\java.exe'))) {$JavaHome=$candidate;break}
        }
    }
    if (!$JavaHome) {throw 'Pass -JavaHome with a JDK 21 installation'}
    $env:JAVA_HOME=$JavaHome
    if (!$env:ANDROID_HOME -and $env:ANDROID_SDK_ROOT) {$env:ANDROID_HOME=$env:ANDROID_SDK_ROOT}
    if (!$env:ANDROID_HOME) {throw 'Set ANDROID_HOME to the installed Android SDK'}
    $ndk=Join-Path $env:ANDROID_HOME 'ndk\27.0.12077973'
    if (!(Test-Path $ndk)) {throw 'Install Android NDK 27.0.12077973 or set the project build script to a verified NDK'}
    $env:ANDROID_NDK_HOME=$ndk
    Push-Location (Join-Path $root 'rust')
    try {
        if (!$SkipNative) {
            & cargo build --locked -p wh-android --features bindgen --lib --bin uniffi-bindgen
            if ($LASTEXITCODE -ne 0) {throw 'Host UniFFI build failed'}
            $bindings=Join-Path $root 'shared\src\main\java'
            & .\target\debug\uniffi-bindgen.exe generate --library .\target\debug\nxwallhaven_core.dll --language kotlin --config .\uniffi-bindings.toml --out-dir $bindings --no-format
            if ($LASTEXITCODE -ne 0) {throw 'Kotlin binding generation failed'}
            $env:RUSTFLAGS=(($previousFlags+' -C link-arg=-Wl,-z,max-page-size=16384').Trim())
            & cargo ndk -t arm64-v8a -t x86_64 --platform 26 -o (Join-Path $root 'shared\src\main\jniLibs') build --locked --release -p wh-android --lib
            if ($LASTEXITCODE -ne 0) {throw 'Android Rust cross build failed'}
        }
    } finally {Pop-Location}
    Push-Location $root
    try {
        $tasks=@(':composeApp:assembleDebug')
        if ($Tests) {$tasks+=@(':shared:testDebugUnitTest',':composeApp:lintDebug')}
        & .\gradlew.bat @tasks --no-daemon --console=plain
        if ($LASTEXITCODE -ne 0) {throw 'Android build/check failed'}
        $out=Join-Path $root 'dist\android'
        New-Item -ItemType Directory -Force $out | Out-Null
        $apk=Join-Path $out 'NxWallhaven-0.1.0-alpha03-debug.apk'
        Copy-Item -LiteralPath (Join-Path $root 'composeApp\build\outputs\apk\debug\composeApp-debug.apk') -Destination $apk -Force -ErrorAction Stop
        $source=Join-Path $root 'composeApp\build\outputs\apk\debug\composeApp-debug.apk'
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash -ne (Get-FileHash -Algorithm SHA256 -LiteralPath $apk).Hash) {
            throw 'Packaged APK does not match the current build'
        }
        Write-Output "APK=$apk"
        Write-Output "BYTES=$((Get-Item $apk).Length)"
        Write-Output "SHA256=$((Get-FileHash -Algorithm SHA256 $apk).Hash)"
        Write-Output 'Development APK signed with the local Android debug key, not a production release signing key.'
    } finally {Pop-Location}
} finally {$env:JAVA_HOME=$previousJava;$env:ANDROID_NDK_HOME=$previousNdk;$env:RUSTFLAGS=$previousFlags}
