//! Guard the login entry point against reverting to two separate sessions.
#[test]
fn login_entry_uses_in_app_browser_not_password_http_submission() {
    let ui = include_str!("../ui/app.slint");
    let controller = include_str!("website_controller.rs");
    assert!(
        !ui.contains("placeholder-text: \"网站密码（不保存）\""),
        "login still collects a password outside the verification browser"
    );
    assert!(
        !controller.contains("client.login(&username, &password)"),
        "login still creates an unrelated HTTP cookie session"
    );
    assert!(
        controller.contains("show_login()"),
        "no in-app verification browser entry"
    );
    assert!(
        !controller.contains("浏览器登录不会自动导入客户端 Cookie"),
        "external-browser dead end remains the only verification path"
    );
}

#[test]
fn browser_window_class_must_not_collide_with_slint_winit() {
    let native = include_str!("browser_session.rs");
    assert!(
        native.contains(".with_window_classname(\"WallHavenStudio.WebLogin.WebView2\")"),
        "Tao and winit default to the same Window Class with incompatible callbacks"
    );
}

#[test]
fn persistent_session_keeps_browser_identity_without_cookie_export_or_stealth() {
    let native = include_str!("browser_session.rs");
    let profile = include_str!("browser_profile.rs");
    let controller = include_str!("website_controller.rs");
    let implementation = native.split("#[cfg(test)]\nmod tests").next().unwrap();
    assert!(native.contains("BrowserProfile::persistent(browser_profile::saved_profile_path()?"));
    assert!(!native.contains("wh-webview-session-"));
    assert!(controller.contains("begin_restore(ui, dialog, &state, &tx)"));
    for forbidden in [
        "with_user_agent(",
        "disable-blink-features",
        "AutomationControlled",
        "document.cookie",
        "cookies_for_url(",
    ] {
        assert!(
            !implementation.contains(forbidden),
            "login must not export cookies or spoof verification environment"
        );
    }
    assert!(profile.contains("SHGetKnownFolderPath"));
}
