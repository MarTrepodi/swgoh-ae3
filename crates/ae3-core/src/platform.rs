//! The parallel asset sets the game publishes per device family.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// One of the asset sets the game publishes per device family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Platform {
    #[default]
    Windows,
    Android,
    Ios,
}

impl Platform {
    pub const ALL: [Self; 3] = [Self::Windows, Self::Android, Self::Ios];

    /// The Platform's segment of a CDN path: device family, then texture format.
    #[must_use]
    pub fn cdn_path(self) -> &'static str {
        match self {
            Self::Windows => "Windows/ETC",
            Self::Android => "Android/ETC",
            Self::Ios => "iOS/PVRTC",
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Android => "Android",
            Self::Ios => "iOS",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown platform {0:?}; expected windows, android or ios")]
pub struct UnknownPlatform(String);

impl FromStr for Platform {
    type Err = UnknownPlatform;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| UnknownPlatform(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitively_and_round_trips() {
        for p in Platform::ALL {
            assert_eq!(p.to_string().to_lowercase().parse::<Platform>(), Ok(p));
        }
        assert_eq!("IOS".parse::<Platform>(), Ok(Platform::Ios));
        assert!("linux".parse::<Platform>().is_err());
    }
}
