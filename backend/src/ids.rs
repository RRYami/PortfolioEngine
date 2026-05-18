use uuid::Uuid;

/// Newtype wrapper around [`Uuid`] for type-safe instrument identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstrumentId(pub Uuid);

/// Newtype wrapper around [`Uuid`] for type-safe portfolio identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortfolioId(pub Uuid);

/// Newtype wrapper around [`Uuid`] for type-safe lot identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LotId(pub Uuid);

/// Newtype wrapper around [`Uuid`] for type-safe transaction identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransactionId(pub Uuid);

impl InstrumentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl PortfolioId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl LotId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl TransactionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InstrumentId {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for PortfolioId {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for LotId {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for TransactionId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let id1 = InstrumentId::new();
        let id2 = InstrumentId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn different_newtypes_are_incompatible() {
        // This test exists to prove the newtype pattern works at compile time.
        // We can't assign an InstrumentId where a PortfolioId is expected.
        // If this compiles, the test is meaningful (we just verify runtime equality).
        let i = InstrumentId::new();
        let p = PortfolioId::new();
        assert_ne!(i.0, p.0);
    }
}
