use async_trait::async_trait;

use crate::ids::InstrumentId;
use crate::instrument::Instrument;

use super::error::RepoError;

/// Repository for instrument reference data.
///
/// Symbol uniqueness is enforced: upserting an instrument with a symbol that
/// already belongs to a *different* [`InstrumentId`] returns
/// [`RepoError::AlreadyExists`].
#[async_trait]
pub trait InstrumentRepository: Send + Sync {
    async fn upsert(&self, instrument: &Instrument) -> Result<(), RepoError>;
    async fn get(&self, id: InstrumentId) -> Result<Instrument, RepoError>;
    async fn by_symbol(&self, symbol: &str) -> Result<Instrument, RepoError>;
    async fn list(&self) -> Result<Vec<Instrument>, RepoError>;
}
