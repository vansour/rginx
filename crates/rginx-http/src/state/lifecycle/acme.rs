use super::super::*;

impl SharedState {
    pub fn register_acme_http01_challenge(
        &self,
        token: impl Into<String>,
        key_authorization: impl Into<String>,
    ) {
        self.acme_http01_challenges
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token.into(), key_authorization.into());
    }

    pub fn unregister_acme_http01_challenge(&self, token: &str) {
        self.acme_http01_challenges
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(token);
    }

    pub fn acme_http01_response(&self, token: &str) -> Option<String> {
        self.acme_http01_challenges
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(token)
            .cloned()
    }
}
