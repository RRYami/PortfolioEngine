use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use tokio::sync::RwLock;

use crate::ids::{InstrumentId, PortfolioId, UserId};
use crate::instrument::Instrument;
use crate::portfolio::Portfolio;
use crate::transaction::Transaction;
use crate::user::User;

use super::error::RepoError;
use super::instrument::InstrumentRepository;
use super::portfolio::PortfolioRepository;
use super::transaction::TransactionRepository;
use super::user::UserRepository;

/// In-memory portfolio repository backed by a [`HashMap`].
#[derive(Debug, Default)]
pub struct InMemoryPortfolioRepository {
    store: Arc<RwLock<HashMap<PortfolioId, Portfolio>>>,
}

impl InMemoryPortfolioRepository {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PortfolioRepository for InMemoryPortfolioRepository {
    async fn create(&self, portfolio: &Portfolio) -> Result<(), RepoError> {
        let mut store = self.store.write().await;
        if store.contains_key(&portfolio.id) {
            return Err(RepoError::AlreadyExists(portfolio.id.to_string()));
        }
        store.insert(portfolio.id, portfolio.clone());
        Ok(())
    }

    async fn get(&self, id: PortfolioId) -> Result<Portfolio, RepoError> {
        let store = self.store.read().await;
        store.get(&id).cloned().ok_or(RepoError::NotFound)
    }

    async fn list(&self, user_id: UserId) -> Result<Vec<Portfolio>, RepoError> {
        let store = self.store.read().await;
        Ok(store
            .values()
            .filter(|p| p.user_id == user_id)
            .cloned()
            .collect())
    }

    async fn update(&self, portfolio: &Portfolio) -> Result<(), RepoError> {
        let mut store = self.store.write().await;
        if !store.contains_key(&portfolio.id) {
            return Err(RepoError::NotFound);
        }
        store.insert(portfolio.id, portfolio.clone());
        Ok(())
    }

    async fn delete(&self, id: PortfolioId) -> Result<(), RepoError> {
        let mut store = self.store.write().await;
        store.remove(&id).ok_or(RepoError::NotFound).map(|_| ())
    }
}

/// In-memory transaction repository backed by a [`HashMap`] of
/// [`Vec<Transaction>`].
///
/// Transactions are stored in insertion order per portfolio.
/// [`list`](TransactionRepository::list) sorts by `trade_date` then by
/// insertion index for deterministic tie-breaking.
#[derive(Debug, Default)]
pub struct InMemoryTransactionRepository {
    store: Arc<RwLock<HashMap<PortfolioId, Vec<Transaction>>>>,
}

impl InMemoryTransactionRepository {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl TransactionRepository for InMemoryTransactionRepository {
    async fn append(
        &self,
        portfolio_id: PortfolioId,
        transaction: &Transaction,
    ) -> Result<(), RepoError> {
        let mut store = self.store.write().await;
        store
            .entry(portfolio_id)
            .or_default()
            .push(transaction.clone());
        Ok(())
    }

    async fn list(&self, portfolio_id: PortfolioId) -> Result<Vec<Transaction>, RepoError> {
        let store = self.store.read().await;
        let mut txs = store.get(&portfolio_id).cloned().unwrap_or_default();
        // Stable sort by trade_date; insertion order already serves as tie-breaker.
        txs.sort_by_key(|a| a.trade_date);
        Ok(txs)
    }

    async fn list_until(
        &self,
        portfolio_id: PortfolioId,
        as_of: NaiveDate,
    ) -> Result<Vec<Transaction>, RepoError> {
        let mut txs = self.list(portfolio_id).await?;
        // Keep only transactions with trade_date <= as_of.
        // The list is already sorted, so we can use partition_point for O(log n).
        let cutoff = txs.partition_point(|tx| tx.trade_date <= as_of);
        txs.truncate(cutoff);
        Ok(txs)
    }
}

/// In-memory instrument repository backed by [`HashMap`]s.
///
/// Enforces symbol uniqueness: an upsert with a symbol that already belongs
/// to a different [`InstrumentId`] returns [`RepoError::AlreadyExists`].
#[derive(Debug, Default)]
pub struct InMemoryInstrumentRepository {
    by_id: Arc<RwLock<HashMap<InstrumentId, Instrument>>>,
    by_symbol: Arc<RwLock<HashMap<String, InstrumentId>>>,
}

impl InMemoryInstrumentRepository {
    pub fn new() -> Self {
        Self {
            by_id: Arc::new(RwLock::new(HashMap::new())),
            by_symbol: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl InstrumentRepository for InMemoryInstrumentRepository {
    async fn upsert(&self, instrument: &Instrument) -> Result<(), RepoError> {
        let mut by_id = self.by_id.write().await;
        let mut by_symbol = self.by_symbol.write().await;

        // If symbol already mapped to a different id, reject.
        if let Some(&existing_id) = by_symbol.get(&instrument.symbol) {
            if existing_id != instrument.id {
                return Err(RepoError::AlreadyExists(instrument.symbol.clone()));
            }
        }

        by_symbol.insert(instrument.symbol.clone(), instrument.id);
        by_id.insert(instrument.id, instrument.clone());
        Ok(())
    }

    async fn get(&self, id: InstrumentId) -> Result<Instrument, RepoError> {
        let by_id = self.by_id.read().await;
        by_id.get(&id).cloned().ok_or(RepoError::NotFound)
    }

    async fn by_symbol(&self, symbol: &str) -> Result<Instrument, RepoError> {
        let by_symbol = self.by_symbol.read().await;
        let by_id = self.by_id.read().await;
        let id = by_symbol.get(symbol).ok_or(RepoError::NotFound)?;
        by_id.get(id).cloned().ok_or(RepoError::NotFound)
    }

    async fn list(&self) -> Result<Vec<Instrument>, RepoError> {
        let by_id = self.by_id.read().await;
        Ok(by_id.values().cloned().collect())
    }
}

/// In-memory user repository backed by a [`HashMap`].
///
/// Email uniqueness is case-insensitive: emails are normalized to lowercase
/// on insert and lookup, mirroring [`User::new`].
#[derive(Debug, Default)]
pub struct InMemoryUserRepository {
    by_id: Arc<RwLock<HashMap<UserId, User>>>,
    by_email: Arc<RwLock<HashMap<String, UserId>>>,
}

impl InMemoryUserRepository {
    pub fn new() -> Self {
        Self {
            by_id: Arc::new(RwLock::new(HashMap::new())),
            by_email: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn create(&self, user: &User) -> Result<(), RepoError> {
        let mut by_id = self.by_id.write().await;
        let mut by_email = self.by_email.write().await;

        if by_id.contains_key(&user.id) {
            return Err(RepoError::AlreadyExists(user.id.to_string()));
        }
        let email = user.email.to_lowercase();
        if by_email.contains_key(&email) {
            return Err(RepoError::AlreadyExists(email));
        }

        by_email.insert(email, user.id);
        by_id.insert(user.id, user.clone());
        Ok(())
    }

    async fn get(&self, id: UserId) -> Result<User, RepoError> {
        let by_id = self.by_id.read().await;
        by_id.get(&id).cloned().ok_or(RepoError::NotFound)
    }

    async fn by_email(&self, email: &str) -> Result<User, RepoError> {
        let by_email = self.by_email.read().await;
        let by_id = self.by_id.read().await;
        let id = by_email
            .get(&email.to_lowercase())
            .ok_or(RepoError::NotFound)?;
        by_id.get(id).cloned().ok_or(RepoError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    use crate::currency::Currency;
    use crate::ids::{InstrumentId, PortfolioId, TransactionId};
    use crate::instrument::{Instrument, InstrumentKind};
    use crate::lot_method::LotMethod;
    use crate::money::Money;
    use crate::portfolio::Portfolio;
    use crate::transaction::TransactionKind;

    use super::*;

    fn portfolio_for(user_id: UserId, name: &str) -> Portfolio {
        Portfolio::new(
            PortfolioId::new(),
            user_id,
            name,
            Currency::USD,
            LotMethod::Fifo,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        )
    }

    fn portfolio(name: &str) -> Portfolio {
        portfolio_for(UserId::new(), name)
    }

    fn instrument(symbol: &str) -> Instrument {
        Instrument {
            id: InstrumentId::new(),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        }
    }

    fn tx(date: NaiveDate, kind: TransactionKind) -> crate::transaction::Transaction {
        crate::transaction::Transaction::new(TransactionId::new(), date, date, kind).unwrap()
    }

    fn usd(dollars: &str) -> Money {
        Money::new(Decimal::from_str_exact(dollars).unwrap(), Currency::USD)
    }

    // ── PortfolioRepository ────────────────────────────────────────────────

    #[tokio::test]
    async fn portfolio_create_and_get() {
        let repo = InMemoryPortfolioRepository::new();
        let p = portfolio("test-portfolio");

        repo.create(&p).await.unwrap();
        let got = repo.get(p.id).await.unwrap();
        assert_eq!(got, p);
    }

    #[tokio::test]
    async fn portfolio_create_duplicate_errors() {
        let repo = InMemoryPortfolioRepository::new();
        let p = portfolio("test-portfolio");

        repo.create(&p).await.unwrap();
        let result = repo.create(&p).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn portfolio_get_missing_returns_not_found() {
        let repo = InMemoryPortfolioRepository::new();
        let result = repo.get(PortfolioId::new()).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_list_returns_only_owner_portfolios() {
        let repo = InMemoryPortfolioRepository::new();
        let alice = UserId::new();
        let bob = UserId::new();
        let p1 = portfolio_for(alice, "alpha");
        let p2 = portfolio_for(alice, "beta");
        let p3 = portfolio_for(bob, "gamma");

        repo.create(&p1).await.unwrap();
        repo.create(&p2).await.unwrap();
        repo.create(&p3).await.unwrap();

        let alices = repo.list(alice).await.unwrap();
        assert_eq!(alices.len(), 2);
        assert!(alices.contains(&p1));
        assert!(alices.contains(&p2));

        let bobs = repo.list(bob).await.unwrap();
        assert_eq!(bobs, vec![p3]);
    }

    #[tokio::test]
    async fn portfolio_update() {
        let repo = InMemoryPortfolioRepository::new();
        let mut p = portfolio("original");
        repo.create(&p).await.unwrap();

        p.name = "updated".to_string();
        repo.update(&p).await.unwrap();

        let got = repo.get(p.id).await.unwrap();
        assert_eq!(got.name, "updated");
    }

    #[tokio::test]
    async fn portfolio_update_missing_returns_not_found() {
        let repo = InMemoryPortfolioRepository::new();
        let p = portfolio("orphan");
        let result = repo.update(&p).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_delete() {
        let repo = InMemoryPortfolioRepository::new();
        let p = portfolio("to-delete");
        repo.create(&p).await.unwrap();

        repo.delete(p.id).await.unwrap();
        assert!(matches!(repo.get(p.id).await, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_delete_missing_returns_not_found() {
        let repo = InMemoryPortfolioRepository::new();
        let result = repo.delete(PortfolioId::new()).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    // ── UserRepository ─────────────────────────────────────────────────────

    fn user(email: &str) -> User {
        User::new(
            UserId::new(),
            email,
            "hash",
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        )
    }

    #[tokio::test]
    async fn user_create_and_get() {
        let repo = InMemoryUserRepository::new();
        let u = user("alice@example.com");

        repo.create(&u).await.unwrap();
        let got = repo.get(u.id).await.unwrap();
        assert_eq!(got, u);
    }

    #[tokio::test]
    async fn user_by_email_is_case_insensitive() {
        let repo = InMemoryUserRepository::new();
        let u = user("Alice@Example.com");

        repo.create(&u).await.unwrap();
        let got = repo.by_email("ALICE@example.COM").await.unwrap();
        assert_eq!(got, u);
    }

    #[tokio::test]
    async fn user_duplicate_email_rejected_case_insensitively() {
        let repo = InMemoryUserRepository::new();
        let u1 = user("alice@example.com");
        let u2 = user("ALICE@example.com");

        repo.create(&u1).await.unwrap();
        let result = repo.create(&u2).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn user_duplicate_id_rejected() {
        let repo = InMemoryUserRepository::new();
        let u1 = user("alice@example.com");
        let mut u2 = user("bob@example.com");
        u2.id = u1.id;

        repo.create(&u1).await.unwrap();
        let result = repo.create(&u2).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn user_get_missing_returns_not_found() {
        let repo = InMemoryUserRepository::new();
        assert!(matches!(
            repo.get(UserId::new()).await,
            Err(RepoError::NotFound)
        ));
        assert!(matches!(
            repo.by_email("nobody@example.com").await,
            Err(RepoError::NotFound)
        ));
    }

    // ── TransactionRepository ──────────────────────────────────────────

    #[tokio::test]
    async fn transaction_append_and_list() {
        let repo = InMemoryTransactionRepository::new();
        let pid = PortfolioId::new();
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let tx = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());

        repo.append(pid, &tx).await.unwrap();
        let list = repo.list(pid).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], tx);
    }

    #[tokio::test]
    async fn transaction_list_in_chronological_order() {
        let repo = InMemoryTransactionRepository::new();
        let pid = PortfolioId::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();

        let tx1 = tx(d1, TransactionKind::deposit(usd("100.00")).unwrap());
        let tx2 = tx(d2, TransactionKind::deposit(usd("200.00")).unwrap());
        let tx3 = tx(d3, TransactionKind::deposit(usd("300.00")).unwrap());

        // Append out of order.
        repo.append(pid, &tx1).await.unwrap();
        repo.append(pid, &tx2).await.unwrap();
        repo.append(pid, &tx3).await.unwrap();

        let list = repo.list(pid).await.unwrap();
        assert_eq!(list[0].trade_date, d2);
        assert_eq!(list[1].trade_date, d3);
        assert_eq!(list[2].trade_date, d1);
    }

    #[tokio::test]
    async fn transaction_list_until_excludes_after_date() {
        let repo = InMemoryTransactionRepository::new();
        let pid = PortfolioId::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let tx1 = tx(d1, TransactionKind::deposit(usd("100.00")).unwrap());
        let tx2 = tx(d2, TransactionKind::deposit(usd("200.00")).unwrap());
        let tx3 = tx(d3, TransactionKind::deposit(usd("300.00")).unwrap());

        repo.append(pid, &tx1).await.unwrap();
        repo.append(pid, &tx2).await.unwrap();
        repo.append(pid, &tx3).await.unwrap();

        let list = repo.list_until(pid, d2).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].trade_date, d1);
        assert_eq!(list[1].trade_date, d2);
    }

    // ── InstrumentRepository ───────────────────────────────────────────

    #[tokio::test]
    async fn instrument_upsert_and_get() {
        let repo = InMemoryInstrumentRepository::new();
        let inst = instrument("AAPL");

        repo.upsert(&inst).await.unwrap();
        let got = repo.get(inst.id).await.unwrap();
        assert_eq!(got, inst);
    }

    #[tokio::test]
    async fn instrument_by_symbol() {
        let repo = InMemoryInstrumentRepository::new();
        let inst = instrument("AAPL");

        repo.upsert(&inst).await.unwrap();
        let got = repo.by_symbol("AAPL").await.unwrap();
        assert_eq!(got, inst);
    }

    #[tokio::test]
    async fn instrument_list() {
        let repo = InMemoryInstrumentRepository::new();
        let i1 = instrument("AAPL");
        let i2 = instrument("TSLA");

        repo.upsert(&i1).await.unwrap();
        repo.upsert(&i2).await.unwrap();

        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn instrument_symbol_uniqueness_rejected() {
        let repo = InMemoryInstrumentRepository::new();
        let i1 = instrument("AAPL");
        let mut i2 = instrument("AAPL");
        i2.id = InstrumentId::new(); // same symbol, different id

        repo.upsert(&i1).await.unwrap();
        let result = repo.upsert(&i2).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn instrument_upsert_same_id_updates() {
        let repo = InMemoryInstrumentRepository::new();
        let mut i1 = instrument("AAPL");
        repo.upsert(&i1).await.unwrap();

        i1.name = "Apple Inc.".to_string();
        repo.upsert(&i1).await.unwrap();

        let got = repo.get(i1.id).await.unwrap();
        assert_eq!(got.name, "Apple Inc.");
    }
}
