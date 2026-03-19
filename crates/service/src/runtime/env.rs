pub(crate) fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_non_empty_or(name: &str, default: &str) -> String {
    env_non_empty(name).unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }

        fn clear(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn env_non_empty_treats_blank_as_missing() {
        let _guard = EnvGuard::set("CODEXMANAGER_TEST_ENV_NON_EMPTY", "   ");
        assert_eq!(env_non_empty("CODEXMANAGER_TEST_ENV_NON_EMPTY"), None);
        assert_eq!(
            env_non_empty_or("CODEXMANAGER_TEST_ENV_NON_EMPTY", "fallback"),
            "fallback"
        );
    }

    #[test]
    fn env_non_empty_trims_values() {
        let _guard = EnvGuard::set("CODEXMANAGER_TEST_ENV_NON_EMPTY", "  value  ");
        assert_eq!(
            env_non_empty("CODEXMANAGER_TEST_ENV_NON_EMPTY"),
            Some("value".to_string())
        );
        assert_eq!(
            env_non_empty_or("CODEXMANAGER_TEST_ENV_NON_EMPTY", "fallback"),
            "value"
        );
    }

    #[test]
    fn env_non_empty_or_uses_default_when_missing() {
        let _guard = EnvGuard::clear("CODEXMANAGER_TEST_ENV_NON_EMPTY");
        assert_eq!(
            env_non_empty_or("CODEXMANAGER_TEST_ENV_NON_EMPTY", "fallback"),
            "fallback"
        );
    }
}
