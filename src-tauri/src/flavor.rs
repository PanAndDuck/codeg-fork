//! Compile-time install identity. Runtime environment cannot switch a running app's state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Official,
    Fork,
}

pub const fn current() -> Flavor {
    Flavor::from_value(option_env!("CODEG_FLAVOR"))
}

impl Flavor {
    pub const fn from_value(value: Option<&str>) -> Self {
        match value {
            Some(value) if matches!(value.as_bytes(), b"fork") => Self::Fork,
            _ => Self::Official,
        }
    }

    pub const fn app_identifier(self) -> &'static str {
        match self {
            Self::Official => "app.codeg",
            Self::Fork => "app.codeg.fork",
        }
    }

    pub const fn product_name(self) -> &'static str {
        match self {
            Self::Official => "codeg",
            Self::Fork => "Codeg-Fork",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Official => "Codeg",
            Self::Fork => "Codeg-Fork",
        }
    }

    pub const fn home_dir_name(self) -> &'static str {
        match self {
            Self::Official => ".codeg",
            Self::Fork => ".codeg-fork",
        }
    }

    pub const fn keyring_service(self) -> &'static str {
        match self {
            Self::Official => "codeg",
            Self::Fork => "codeg-fork",
        }
    }

    pub const fn deep_link_scheme(self) -> &'static str {
        match self {
            Self::Official => "codeg",
            Self::Fork => "codeg-fork",
        }
    }

    pub const fn default_web_port(self) -> u16 {
        match self {
            Self::Official => 3080,
            Self::Fork => 3090,
        }
    }

    pub const fn updater_enabled(self) -> bool {
        self.updater_enabled_for(option_env!("CODEG_UPDATER"))
    }

    const fn updater_enabled_for(self, setting: Option<&str>) -> bool {
        let disabled = matches!(setting, Some(value) if matches!(value.as_bytes(), b"off"));
        matches!(self, Self::Official) && !disabled
    }

    pub const fn shared_config_writes_enabled(self) -> bool {
        match self {
            Self::Official => true,
            Self::Fork => false,
        }
    }

    pub const fn computer_helper_bundle_id(self) -> &'static str {
        match self {
            Self::Official => "app.codeg.computer-helper",
            Self::Fork => "app.codeg.fork.computer-helper",
        }
    }

    pub const fn server_data_dir_name(self) -> &'static str {
        match self {
            Self::Official => "codeg",
            Self::Fork => "codeg-fork",
        }
    }

    pub const fn server_data_fallback(self) -> &'static str {
        match self {
            Self::Official => ".codeg-data",
            Self::Fork => ".codeg-fork-data",
        }
    }

    pub const fn scratch_namespace(self) -> &'static str {
        match self {
            Self::Official => "codeg-acp",
            Self::Fork => "codeg-fork-acp",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_fork_selects_fork() {
        assert_eq!(Flavor::from_value(Some("fork")), Flavor::Fork);
        for value in [
            None,
            Some(""),
            Some("official"),
            Some("FORK"),
            Some("fork "),
        ] {
            assert_eq!(Flavor::from_value(value), Flavor::Official);
        }
    }

    #[test]
    fn official_literals_are_unchanged() {
        assert_eq!(Flavor::Official.app_identifier(), "app.codeg");
        assert_eq!(Flavor::Official.product_name(), "codeg");
        assert_eq!(Flavor::Official.display_name(), "Codeg");
        assert_eq!(Flavor::Official.home_dir_name(), ".codeg");
        assert_eq!(Flavor::Official.keyring_service(), "codeg");
        assert_eq!(Flavor::Official.deep_link_scheme(), "codeg");
        assert_eq!(Flavor::Official.default_web_port(), 3080);
        assert!(Flavor::Official.updater_enabled_for(None));
        assert!(Flavor::Official.shared_config_writes_enabled());
        assert_eq!(
            Flavor::Official.computer_helper_bundle_id(),
            "app.codeg.computer-helper"
        );
        assert_eq!(Flavor::Official.server_data_dir_name(), "codeg");
        assert_eq!(Flavor::Official.server_data_fallback(), ".codeg-data");
        assert_eq!(Flavor::Official.scratch_namespace(), "codeg-acp");
    }

    #[test]
    fn fork_contract_is_isolated() {
        assert_eq!(Flavor::Fork.app_identifier(), "app.codeg.fork");
        assert_eq!(Flavor::Fork.product_name(), "Codeg-Fork");
        assert_eq!(Flavor::Fork.display_name(), "Codeg-Fork");
        assert_eq!(Flavor::Fork.home_dir_name(), ".codeg-fork");
        assert_eq!(Flavor::Fork.keyring_service(), "codeg-fork");
        assert_eq!(Flavor::Fork.deep_link_scheme(), "codeg-fork");
        assert_eq!(Flavor::Fork.default_web_port(), 3090);
        assert!(!Flavor::Fork.updater_enabled());
        assert!(!Flavor::Fork.shared_config_writes_enabled());
        assert_eq!(
            Flavor::Fork.computer_helper_bundle_id(),
            "app.codeg.fork.computer-helper"
        );
        assert_eq!(Flavor::Fork.server_data_dir_name(), "codeg-fork");
        assert_eq!(Flavor::Fork.server_data_fallback(), ".codeg-fork-data");
        assert_eq!(Flavor::Fork.scratch_namespace(), "codeg-fork-acp");
    }

    #[test]
    fn build_environment_selects_identity() {
        assert_eq!(current(), Flavor::from_value(option_env!("CODEG_FLAVOR")));
    }

    #[test]
    fn build_environment_can_disable_official_updates() {
        assert_eq!(
            Flavor::Official.updater_enabled(),
            option_env!("CODEG_UPDATER") != Some("off")
        );
        assert!(!Flavor::Fork.updater_enabled());
    }

    #[test]
    fn updater_switch_preserves_default_and_never_enables_fork() {
        for value in [None, Some(""), Some("on"), Some("OFF"), Some("off ")] {
            assert!(Flavor::Official.updater_enabled_for(value));
            assert!(!Flavor::Fork.updater_enabled_for(value));
        }
        assert!(!Flavor::Official.updater_enabled_for(Some("off")));
        assert!(!Flavor::Fork.updater_enabled_for(Some("off")));
    }
}
