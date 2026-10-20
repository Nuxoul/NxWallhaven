use crate::{invalid, MobileError};
use reqwest::Url;

pub(crate) fn same_origin(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("wallhaven.cc")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

pub(crate) fn image_url(input: &str) -> Result<Url, MobileError> {
    // No credentials, query strings, percent-encoded traversal, or parser normalization.
    if input.contains(['%', '\\']) || input.split('/').any(|s| s == "." || s == "..") {
        return Err(invalid("unsupported image URL"));
    }
    let url = Url::parse(input).map_err(|_| invalid("invalid image URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "image URL must not contain credentials, query, or fragment",
        ));
    }
    #[cfg(test)]
    if url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
    {
        return Ok(url);
    }
    if url.scheme() != "https" || url.port().is_some() {
        return Err(invalid("image URL must use HTTPS"));
    }
    let allowed = match url.host_str() {
        Some("w.wallhaven.cc") => url.path().starts_with("/full/"),
        Some("th.wallhaven.cc") => true,
        // The current public profile uses /images/user/avatar/, not only /avatar/.
        Some("wallhaven.cc") => {
            url.path().starts_with("/avatar/") || url.path().starts_with("/images/user/avatar/")
        }
        _ => false,
    };
    if !allowed {
        return Err(invalid("unsupported image origin or path"));
    }
    Ok(url)
}

pub(crate) fn avatar(input: &str) -> Option<String> {
    let input = if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_owned()
    };
    let url = image_url(&input).ok()?;
    (same_origin(&url)
        && (url.path().starts_with("/avatar/") || url.path().starts_with("/images/user/avatar/")))
    .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whitelist_and_avatar_hosts_are_strict() {
        for url in [
            "https://w.wallhaven.cc/full/ab/wallhaven-abcdef.jpg",
            "https://th.wallhaven.cc/orig/a.jpg",
            "https://wallhaven.cc/avatar/a.jpg",
            "https://wallhaven.cc/images/user/avatar/200/a.jpg",
        ] {
            assert!(image_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://w.wallhaven.cc/full/a.jpg",
            "https://w.wallhaven.cc/other/a.jpg",
            "https://wallhaven.cc/api/v1/settings",
            "https://evil.test/a.jpg",
            "https://wallhaven.cc@evil.test/avatar/a.jpg",
            "https://th.wallhaven.cc/a?apikey=secret",
            "https://th.wallhaven.cc/%2e%2e/a",
            "https://th.wallhaven.cc/a/../b",
            "https://th.wallhaven.cc:444/a",
            "https://th.wallhaven.cc/a#secret",
        ] {
            assert!(image_url(url).is_err(), "{url}");
        }
        assert!(avatar("//wallhaven.cc/images/user/avatar/200/a.jpg").is_some());
        assert!(avatar("https://th.wallhaven.cc/a.jpg").is_none());
    }
}
