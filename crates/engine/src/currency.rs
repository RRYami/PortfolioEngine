use std::fmt;

/// A three-letter ISO-4217-like currency code, stored as ASCII bytes.
///
/// Construction is validated: only exactly three uppercase ASCII letters are accepted.
/// Direct construction via [`Currency::new`] bypasses validation and should only be used
/// with known-valid literals (e.g. the provided `const` constructors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Currency([u8; 3]);

impl Currency {
    pub const USD: Self = Self(*b"USD");
    pub const EUR: Self = Self(*b"EUR");
    pub const GBP: Self = Self(*b"GBP");
    pub const JPY: Self = Self(*b"JPY");
    pub const CHF: Self = Self(*b"CHF");

    /// Direct construction from bytes.
    /// **Caller must ensure the bytes are three uppercase ASCII letters.**
    pub const fn new(bytes: [u8; 3]) -> Self {
        Self(bytes)
    }

    /// Returns the currency code as a string slice.
    ///
    /// # Safety
    /// The internal bytes are always valid ASCII by construction.
    pub fn as_str(&self) -> &str {
        // SAFETY: [u8; 3] is valid ASCII by construction.
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }
}

impl TryFrom<&str> for Currency {
    type Error = crate::error::DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() != 3 {
            return Err(crate::error::DomainError::InvalidCurrencyCode(
                value.to_string(),
            ));
        }
        if !value.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(crate::error::DomainError::InvalidCurrencyCode(
                value.to_string(),
            ));
        }
        let mut bytes = [0u8; 3];
        bytes.copy_from_slice(value.as_bytes());
        Ok(Self(bytes))
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl AsRef<str> for Currency {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Currency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Currency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Visitor;

        struct CurrencyVisitor;

        impl Visitor<'_> for CurrencyVisitor {
            type Value = Currency;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a three-letter uppercase ASCII currency code")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Currency::try_from(value).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(CurrencyVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_currency() {
        let c = Currency::try_from("USD").unwrap();
        assert_eq!(c.as_str(), "USD");
        assert_eq!(c.to_string(), "USD");
    }

    #[test]
    fn lowercase_rejected() {
        assert!(matches!(
            Currency::try_from("usd"),
            Err(crate::error::DomainError::InvalidCurrencyCode(_))
        ));
    }

    #[test]
    fn non_ascii_rejected() {
        assert!(Currency::try_from("US1").is_err());
        assert!(Currency::try_from("U$1").is_err());
    }

    #[test]
    fn wrong_length_rejected() {
        assert!(Currency::try_from("US").is_err());
        assert!(Currency::try_from("USDD").is_err());
        assert!(Currency::try_from("").is_err());
    }

    #[test]
    fn const_constructors_match_try_from() {
        assert_eq!(Currency::USD, Currency::try_from("USD").unwrap());
        assert_eq!(Currency::EUR, Currency::try_from("EUR").unwrap());
        assert_eq!(Currency::GBP, Currency::try_from("GBP").unwrap());
        assert_eq!(Currency::JPY, Currency::try_from("JPY").unwrap());
        assert_eq!(Currency::CHF, Currency::try_from("CHF").unwrap());
    }

    #[test]
    fn as_ref_str() {
        let c = Currency::USD;
        assert_eq!(c.as_ref(), "USD");
    }
}
