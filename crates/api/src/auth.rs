//! Session-based authentication via `axum-login` + `tower-sessions`.
//!
//! Passwords are hashed with argon2id (`password-auth`). Sessions live
//! server-side (Postgres in production, in-memory for the no-DB dev path);
//! the browser only holds an opaque `HttpOnly` cookie. `login` regenerates the
//! session id (fixation protection) and `logout` destroys the session
//! server-side — real revocation.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum_login::{AuthManagerLayer, AuthManagerLayerBuilder, AuthSession, AuthnBackend};
use ptf_engine::{RepoError, User, UserId, UserRepository};
use serde::{Deserialize, Serialize};
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::Duration;
use tower_sessions::{Expiry, SessionManagerLayer, SessionStore};

use crate::error::ApiError;
use crate::state::AppState;

/// Session idle expiry: rolling 7 days.
const SESSION_IDLE_DAYS: i64 = 7;
/// Password policy (NIST 800-63B): length rules only, no composition rules.
const PASSWORD_MIN: usize = 8;
const PASSWORD_MAX: usize = 128;
const EMAIL_MAX: usize = 254;

/// Wraps the engine [`User`] to satisfy `axum-login`'s `AuthUser` (orphan
/// rule: the trait and the engine type are both foreign to this crate).
#[derive(Debug, Clone)]
pub struct SessionAuthUser(pub User);

impl axum_login::AuthUser for SessionAuthUser {
    type Id = UserId;

    fn id(&self) -> Self::Id {
        self.0.id
    }

    /// Sessions are invalidated when the password hash changes.
    fn session_auth_hash(&self) -> &[u8] {
        self.0.password_hash.as_bytes()
    }
}

/// Login/registration request body.
#[derive(Debug, Clone, Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

/// Public user payload — never contains the password hash.
#[derive(Debug, Clone, Serialize)]
pub struct UserSummary {
    pub id: String,
    pub email: String,
}

impl From<&User> for UserSummary {
    fn from(u: &User) -> Self {
        Self {
            id: u.id.0.to_string(),
            email: u.email.clone(),
        }
    }
}

/// Error type for the auth backend (infra failures only; bad credentials are
/// `Ok(None)`, never an error).
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// `axum-login` backend over the [`UserRepository`].
#[derive(Clone)]
pub struct Backend {
    users: Arc<dyn UserRepository>,
}

impl Backend {
    pub fn new(users: Arc<dyn UserRepository>) -> Self {
        Self { users }
    }
}

impl AuthnBackend for Backend {
    type User = SessionAuthUser;
    type Credentials = Credentials;
    type Error = BackendError;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        let user = match self.users.by_email(&creds.email).await {
            Ok(u) => u,
            Err(RepoError::NotFound) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // Constant-time verification; any failure is "not authenticated".
        if password_auth::verify_password(&creds.password, &user.password_hash).is_ok() {
            Ok(Some(SessionAuthUser(user)))
        } else {
            Ok(None)
        }
    }

    async fn get_user(
        &self,
        user_id: &axum_login::UserId<Self>,
    ) -> Result<Option<Self::User>, Self::Error> {
        match self.users.get(*user_id).await {
            Ok(u) => Ok(Some(SessionAuthUser(u))),
            Err(RepoError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Request extractor requiring a logged-in user: 401 JSON otherwise.
#[derive(Debug, Clone)]
pub struct SessionUser(pub User);

impl FromRequestParts<AppState> for SessionUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthSession::<Backend>::from_request_parts(parts, state)
            .await
            .map_err(|(status, msg)| ApiError::Internal(format!("{status}: {msg}")))?;
        auth.user
            .map(|u| Self(u.0))
            .ok_or_else(|| ApiError::Unauthorized("not logged in".into()))
    }
}

/// Builds the session layer: opaque `ptf_session` cookie, `HttpOnly`,
/// SameSite=Lax, Secure behind a flag (set `PTF_SECURE_COOKIES=1` behind TLS).
pub fn session_layer<S: SessionStore>(store: S, secure: bool) -> SessionManagerLayer<S> {
    SessionManagerLayer::new(store)
        .with_name("ptf_session")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(Duration::days(SESSION_IDLE_DAYS)))
}

/// Builds the `axum-login` layer over the given session layer.
pub fn auth_layer<S: SessionStore>(
    backend: Backend,
    session_layer: SessionManagerLayer<S>,
) -> AuthManagerLayer<Backend, S> {
    AuthManagerLayerBuilder::new(backend, session_layer).build()
}

fn validate_email(email: &str) -> Result<(), ApiError> {
    let valid = email.len() <= EMAIL_MAX
        && email.len() >= 3
        && email.contains('@')
        && !email.contains(char::is_whitespace);
    if valid {
        Ok(())
    } else {
        Err(ApiError::BadRequest("invalid email address".into()))
    }
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() < PASSWORD_MIN {
        return Err(ApiError::BadRequest(format!(
            "password must be at least {PASSWORD_MIN} characters"
        )));
    }
    if password.len() > PASSWORD_MAX {
        return Err(ApiError::BadRequest(format!(
            "password must be at most {PASSWORD_MAX} characters"
        )));
    }
    Ok(())
}

/// `POST /api/auth/register` — create an account and log in. Disabled with
/// `PTF_DISABLE_REGISTRATION=1`.
pub async fn register(
    State(app): State<AppState>,
    mut auth: AuthSession<Backend>,
    Json(req): Json<Credentials>,
) -> Result<(StatusCode, Json<UserSummary>), ApiError> {
    if !app.registration_open {
        return Err(ApiError::Forbidden("registration is disabled".into()));
    }
    validate_email(&req.email)?;
    validate_password(&req.password)?;

    let today = chrono::Utc::now().date_naive();
    let user = User::new(
        UserId::new(),
        &req.email,
        password_auth::generate_hash(&req.password),
        today,
    );
    app.users.create(&user).await.map_err(|e| match e {
        RepoError::AlreadyExists(_) => ApiError::BadRequest("email already registered".into()),
        other => ApiError::from(other),
    })?;

    auth.login(&SessionAuthUser(user.clone()))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(UserSummary::from(&user))))
}

/// `POST /api/auth/login` — verify credentials, establish a session.
pub async fn login(
    mut auth: AuthSession<Backend>,
    Json(req): Json<Credentials>,
) -> Result<Json<UserSummary>, ApiError> {
    let Some(user) = auth
        .authenticate(req)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    else {
        // Generic message on purpose: no user enumeration.
        return Err(ApiError::Unauthorized("invalid email or password".into()));
    };
    auth.login(&user)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(UserSummary::from(&user.0)))
}

/// `POST /api/auth/logout` — destroy the session server-side.
pub async fn logout(mut auth: AuthSession<Backend>) -> Result<StatusCode, ApiError> {
    auth.logout()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/auth/me` — the current session user (frontend session probe).
pub async fn me(user: SessionUser) -> Json<UserSummary> {
    Json(UserSummary::from(&user.0))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use ptf_engine::{
        InMemoryInstrumentRepository, InMemoryPortfolioRepository, InMemoryTransactionRepository,
        InMemoryUserRepository,
    };
    use tower::ServiceExt;
    use tower_sessions::MemoryStore;

    use super::*;
    use crate::handlers;
    use crate::price_source::SyntheticPriceSource;

    #[test]
    fn email_validation() {
        assert!(validate_email("alice@example.com").is_ok());
        assert!(validate_email("no-at-sign").is_err());
        assert!(validate_email("has space@example.com").is_err());
        assert!(validate_email("a@").is_err());
        assert!(validate_email(&format!("{}@x.com", "a".repeat(300))).is_err());
    }

    #[test]
    fn password_validation() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password(&"x".repeat(129)).is_err());
    }

    // ── Auth stack integration tests (in-memory repos + memory sessions) ────

    /// Builds the router with the same layering as `main` (governor included)
    /// over in-memory storage.
    fn test_app(registration_open: bool) -> Router {
        let users: Arc<dyn UserRepository> = Arc::new(InMemoryUserRepository::new());
        let state = AppState::new(
            Arc::new(InMemoryPortfolioRepository::new()),
            Arc::new(InMemoryTransactionRepository::new()),
            Arc::new(InMemoryInstrumentRepository::new()),
            users.clone(),
            Arc::new(SyntheticPriceSource),
            None,
            registration_open,
            None,
        );
        let backend = Backend::new(users);
        handlers::router(state).layer(auth_layer(
            backend,
            session_layer(MemoryStore::default(), false),
        ))
    }

    /// Builds a JSON request with a `ConnectInfo` extension (the governor's
    /// per-IP key extractor requires it) and an optional session cookie.
    fn req(
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
        cookie: Option<&str>,
    ) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        if let Some(c) = cookie {
            b = b.header("cookie", c);
        }
        let mut r = b
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        r
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Registers a user and returns the session cookie (`name=value`).
    async fn register_cookie(app: &Router, email: &str) -> String {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/auth/register",
                Some(serde_json::json!({ "email": email, "password": "password123" })),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("register sets a session cookie")
            .to_str()
            .unwrap()
            .to_string();
        set_cookie.split(';').next().unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn register_me_logout_me_unauthorized() {
        let app = test_app(true);
        let cookie = register_cookie(&app, "alice@example.com").await;

        let resp = app
            .clone()
            .oneshot(req("GET", "/api/auth/me", None, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let me = body_json(resp).await;
        assert_eq!(me["email"], "alice@example.com");

        let resp = app
            .clone()
            .oneshot(req("POST", "/api/auth/logout", None, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // The session is destroyed server-side: the old cookie is useless.
        let resp = app
            .clone()
            .oneshot(req("GET", "/api/auth/me", None, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_wrong_password_and_right_password() {
        let app = test_app(true);
        let cookie = register_cookie(&app, "alice@example.com").await;
        app.clone()
            .oneshot(req("POST", "/api/auth/logout", None, Some(&cookie)))
            .await
            .unwrap();

        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/auth/login",
                Some(serde_json::json!({ "email": "alice@example.com", "password": "wrong-password" })),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let err = body_json(resp).await;
        assert_eq!(err["error"], "invalid email or password");

        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/auth/login",
                Some(
                    serde_json::json!({ "email": "Alice@Example.com", "password": "password123" }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("set-cookie").is_some());
    }

    #[tokio::test]
    async fn duplicate_registration_is_rejected() {
        let app = test_app(true);
        register_cookie(&app, "alice@example.com").await;

        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/auth/register",
                Some(
                    serde_json::json!({ "email": "ALICE@example.com", "password": "password123" }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let err = body_json(resp).await;
        assert_eq!(err["error"], "email already registered");
    }

    #[tokio::test]
    async fn portfolios_require_authentication() {
        let app = test_app(true);
        let resp = app
            .clone()
            .oneshot(req("GET", "/api/portfolios", None, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn users_cannot_see_each_others_portfolios() {
        let app = test_app(true);
        let alice = register_cookie(&app, "alice@example.com").await;

        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/portfolios",
                Some(serde_json::json!({ "name": "alice book", "baseCcy": "USD" })),
                Some(&alice),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let created = body_json(resp).await;
        let pid = created["id"].as_str().unwrap().to_string();

        let bob = register_cookie(&app, "bob@example.com").await;

        // Bob's list is empty; Alice's portfolio is invisible to him (404).
        let resp = app
            .clone()
            .oneshot(req("GET", "/api/portfolios", None, Some(&bob)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await, serde_json::json!([]));

        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                &format!("/api/portfolios/{pid}"),
                None,
                Some(&bob),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Alice still sees it.
        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                &format!("/api/portfolios/{pid}"),
                None,
                Some(&alice),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn registration_disabled_returns_forbidden() {
        let app = test_app(false);
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/auth/register",
                Some(
                    serde_json::json!({ "email": "alice@example.com", "password": "password123" }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn login_is_rate_limited_after_burst() {
        let app = test_app(true);
        let attempt = || {
            req(
                "POST",
                "/api/auth/login",
                Some(
                    serde_json::json!({ "email": "alice@example.com", "password": "password123" }),
                ),
                None,
            )
        };
        // burst_size is 5: the first five get through (401 unknown user),
        // the sixth is throttled.
        for _ in 0..5 {
            let resp = app.clone().oneshot(attempt()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }
        let resp = app.clone().oneshot(attempt()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
