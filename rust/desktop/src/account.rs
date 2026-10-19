//! Account session state and collection routing. Never stores credentials.
#[derive(Default)]
pub struct Session {
    generation: u64,
    pub verified: bool,
    pub busy: bool,
    pub status: String,
}
impl Session {
    pub fn invalidate(&mut self, has_key: bool) {
        self.generation += 1;
        self.verified = false;
        self.busy = false;
        self.status = if has_key {
            "Key 已配置，尚未验证"
        } else {
            "未连接账号，请在设置填写 API Key"
        }
        .into();
    }
    pub fn begin(&mut self, has_key: bool) -> Option<u64> {
        if self.busy {
            return None;
        }
        self.invalidate(has_key);
        if !has_key {
            return None;
        }
        self.busy = true;
        self.status = "正在验证账号凭据…".into();
        Some(self.generation)
    }
    pub fn complete(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.busy = false;
        self.verified = result.is_ok();
        self.status = match result {
            Ok(()) => "账号凭据已验证（API Key）".into(),
            Err(error) => format!("账号验证失败：{error}"),
        };
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectionSource {
    pub username: String,
    pub id: u32,
    pub label: String,
    pub authenticated: bool,
    pub website: bool,
}
impl CollectionSource {
    pub fn new(username: &str, id: u32, label: String) -> Result<Self, String> {
        let username = username.trim();
        validate_username(username)?;
        if id == 0 {
            return Err("收藏夹 ID 无效".into());
        }
        Ok(Self {
            username: username.into(),
            id,
            label,
            authenticated: false,
            website: false,
        })
    }
    #[cfg(test)]
    pub fn web_url(&self) -> String {
        format!(
            "https://wallhaven.cc/user/{}/favorites/{}",
            self.username, self.id
        )
    }
}

pub fn validate_username(username: &str) -> Result<(), String> {
    if username.is_empty()
        || username.len() > 64
        || !username
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err("用户名需为 1–64 位英文字母、数字、下划线或连字符".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_key_is_not_verified_until_request_succeeds() {
        let mut session = Session::default();
        session.invalidate(true);
        assert!(!session.verified);
        let token = session.begin(true).unwrap();
        assert!(session.busy);
        assert!(session.begin(true).is_none());
        assert!(session.complete(token, Ok(())));
        assert!(session.verified);
        assert!(!session.busy);
    }
    #[test]
    fn changed_credentials_reject_old_success_and_failure() {
        let mut session = Session::default();
        let token = session.begin(true).unwrap();
        session.invalidate(false);
        assert!(!session.complete(token, Ok(())));
        assert!(!session.complete(token, Err("old failure".into())));
        assert!(!session.verified);
        assert!(!session.busy);
        assert!(session.begin(false).is_none());
    }
    #[test]
    fn failure_does_not_claim_authenticated() {
        let mut session = Session::default();
        let token = session.begin(true).unwrap();
        session.complete(token, Err("unauthorized: 401".into()));
        assert!(!session.verified);
        assert!(session.status.contains("失败"));
    }
    #[test]
    fn collection_source_requires_safe_owner_and_id() {
        for owner in ["", "../escape", "a/b", "a?apikey=x", "x\ny", " 用户 "] {
            assert!(CollectionSource::new(owner, 1, "x".into()).is_err());
        }
        assert!(CollectionSource::new("user", 0, "x".into()).is_err());
        let source = CollectionSource::new(" user_1 ", 12, "Favourites".into()).unwrap();
        assert_eq!(
            source.web_url(),
            "https://wallhaven.cc/user/user_1/favorites/12"
        );
    }
}
