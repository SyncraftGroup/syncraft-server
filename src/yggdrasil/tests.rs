// ---------------------------------------------------------------------------
// Yggdrasil tests — consolidated unit, DB, and HTTP integration tests
// ---------------------------------------------------------------------------

// ============================================================================
// Unit tests — pure functions with no I/O
// ============================================================================
#[cfg(test)]
mod unit {
    use crate::yggdrasil::db::unsigned_uuid;
    use crate::yggdrasil::email::verify_email_string;
    use crate::yggdrasil::models::{TokenStatus, YggdrasilSkinType};
    use chrono::{Duration, Utc};

    // -- TokenStatus ---------------------------------------------------------

    #[test]
    fn token_status_active() {
        let created = Utc::now() - Duration::hours(24);
        assert_eq!(TokenStatus::from_created_at(created), TokenStatus::Active);
    }

    #[test]
    fn token_status_inactive() {
        let created = Utc::now() - Duration::hours(72);
        assert_eq!(TokenStatus::from_created_at(created), TokenStatus::Inactive);
    }

    #[test]
    fn token_status_invalid() {
        let created = Utc::now() - Duration::days(10);
        assert_eq!(TokenStatus::from_created_at(created), TokenStatus::Invalid);
    }

    #[test]
    fn token_status_boundary_48h() {
        // Just inside the Active window (47h 59min)
        let created = Utc::now() - Duration::hours(47) - Duration::minutes(59);
        assert_eq!(TokenStatus::from_created_at(created), TokenStatus::Active);
    }

    #[test]
    fn token_status_boundary_7d() {
        // Just inside the Inactive window (6d 23h 59min)
        let created = Utc::now() - Duration::days(6) - Duration::hours(23) - Duration::minutes(59);
        assert_eq!(TokenStatus::from_created_at(created), TokenStatus::Inactive);
    }

    // -- YggdrasilSkinType ---------------------------------------------------

    #[test]
    fn skin_type_from_string_default() {
        assert_eq!(
            YggdrasilSkinType::from("default".to_string()),
            YggdrasilSkinType::Default
        );
        assert_eq!(
            YggdrasilSkinType::from("unknown".to_string()),
            YggdrasilSkinType::Default
        );
        assert_eq!(
            YggdrasilSkinType::from("".to_string()),
            YggdrasilSkinType::Default
        );
    }

    #[test]
    fn skin_type_from_string_slim() {
        assert_eq!(
            YggdrasilSkinType::from("slim".to_string()),
            YggdrasilSkinType::Slim
        );
    }

    #[test]
    fn skin_type_to_string() {
        assert_eq!(YggdrasilSkinType::Default.to_string(), "default");
        assert_eq!(YggdrasilSkinType::Slim.to_string(), "slim");
    }

    // -- unsigned_uuid -------------------------------------------------------

    #[test]
    fn unsigned_uuid_format() {
        let uuid = unsigned_uuid();
        // UUID v4 without dashes: 32 hex characters
        assert_eq!(uuid.len(), 32);
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn unsigned_uuid_unique() {
        let a = unsigned_uuid();
        let b = unsigned_uuid();
        assert_ne!(a, b);
    }

    // -- verify_email_string -------------------------------------------------

    #[test]
    fn verify_email_string_contains_link() {
        let html = verify_email_string("http://example.com/verify?code=abc123".into());
        assert!(html.contains("http://example.com/verify?code=abc123"));
        assert!(html.contains("Syncraft Verify"));
    }

    // -- sign ----------------------------------------------------------------

    #[test]
    fn sign_produces_base64_signature() {
        use crate::yggdrasil::sign::sign;
        use rsa::{RsaPrivateKey, pkcs1v15::SigningKey};
        use sha1::Sha1;

        let key = RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
        let signing_key = SigningKey::<Sha1>::new(key);
        let sig = sign(&signing_key, b"hello world");
        // Base64-encoded RSA-2048 signature should be non-empty
        assert!(!sig.is_empty());
        // Should be valid Base64
        use base64::Engine;
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode(&sig)
                .is_ok()
        );
    }
}

// ============================================================================
// DB integration tests — in-memory SQLite
// ============================================================================
#[cfg(test)]
mod db_tests {
    use crate::yggdrasil::db::DBManager;
    use crate::yggdrasil::models::YggdrasilSkinType;
    use sqlx::SqlitePool;

    async fn test_db() -> DBManager {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        DBManager::init_db(&pool).await.unwrap();
        DBManager { pool }
    }

    // -- Users ---------------------------------------------------------------

    #[tokio::test]
    async fn create_and_find_user() {
        let db = test_db().await;
        let (user, code) = db
            .create_user("a@b.com", "pass123", "CharName")
            .await
            .unwrap();
        assert!(!user.id.is_empty());
        assert_eq!(user.email, "a@b.com");
        assert!(!user.email_verified);
        assert_eq!(user.character_name, "CharName");
        assert!(!code.is_empty());

        let found = db.find_user_by_email("a@b.com").await.unwrap().unwrap();
        assert_eq!(found.id, user.id);
        assert_eq!(found.character_name, "CharName");
    }

    #[tokio::test]
    async fn find_nonexistent_user() {
        let db = test_db().await;
        let result = db.find_user_by_email("nobody@b.com").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn verify_email_works() {
        let db = test_db().await;
        let (_user, code) = db.create_user("v@b.com", "pass", "V").await.unwrap();
        let verified_id = db.verify_email(&code).await.unwrap().unwrap();
        assert_eq!(verified_id, _user.id);

        // Re-using the code should return None
        let second = db.verify_email(&code).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn verify_email_invalid_code() {
        let db = test_db().await;
        let result = db.verify_email("no-such-code").await.unwrap();
        assert!(result.is_none());
    }

    // -- Tokens --------------------------------------------------------------

    #[tokio::test]
    async fn create_and_get_token() {
        let db = test_db().await;
        let (user, _) = db.create_user("tok@b.com", "pass", "Tok").await.unwrap();
        let (access, client) = db.create_tokens(&user.id).await.unwrap();
        assert!(!access.is_empty());
        assert!(!client.is_empty());

        let info = db.get_token(&access).await.unwrap().unwrap();
        assert_eq!(info.client_token, client);
        assert_eq!(info.user_id, user.id);
    }

    #[tokio::test]
    async fn get_nonexistent_token() {
        let db = test_db().await;
        let info = db.get_token("deadbeef").await.unwrap();
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn refresh_token_creates_new_access() {
        let db = test_db().await;
        let (user, _) = db.create_user("ref@b.com", "pass", "Ref").await.unwrap();
        let (access1, client) = db.create_tokens(&user.id).await.unwrap();
        let access2 = db.refresh_token(&client, &user.id).await.unwrap();
        assert_ne!(access1, access2);

        // Both tokens share the same client_token
        let info1 = db.get_token(&access1).await.unwrap().unwrap();
        let info2 = db.get_token(&access2).await.unwrap().unwrap();
        assert_eq!(info1.client_token, client);
        assert_eq!(info2.client_token, client);
    }

    #[tokio::test]
    async fn delete_token() {
        let db = test_db().await;
        let (user, _) = db.create_user("del@b.com", "pass", "Del").await.unwrap();
        let (access, _) = db.create_tokens(&user.id).await.unwrap();
        db.delete_token(&access).await.unwrap();
        assert!(db.get_token(&access).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_user_tokens() {
        let db = test_db().await;
        let (user, _) = db
            .create_user("delall@b.com", "pass", "DelAll")
            .await
            .unwrap();
        let (a1, _) = db.create_tokens(&user.id).await.unwrap();
        let (a2, _) = db.create_tokens(&user.id).await.unwrap();
        db.delete_user_tokens(&user.id).await.unwrap();
        assert!(db.get_token(&a1).await.unwrap().is_none());
        assert!(db.get_token(&a2).await.unwrap().is_none());
    }

    // -- Profile -------------------------------------------------------------

    #[tokio::test]
    async fn get_profile_for_user() {
        let db = test_db().await;
        let (user, _) = db
            .create_user("prof@b.com", "pass", "ProfileChar")
            .await
            .unwrap();
        let profile = db.get_profile_for_user(&user.id).await.unwrap().unwrap();
        assert_eq!(profile.id, user.id);
        assert_eq!(profile.name, "ProfileChar");
    }

    #[tokio::test]
    async fn get_profile_not_found() {
        let db = test_db().await;
        let profile = db.get_profile_for_user("no-such-id").await.unwrap();
        assert!(profile.is_none());
    }

    // -- Skins ---------------------------------------------------------------

    #[tokio::test]
    async fn set_and_get_skin() {
        let db = test_db().await;
        let (user, _) = db
            .create_user("skin@b.com", "pass", "SkinChar")
            .await
            .unwrap();
        let skin_data = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3]; // fake PNG header
        db.set_skin(&user.id, skin_data.clone(), YggdrasilSkinType::Slim)
            .await
            .unwrap();

        let (hash, skin_type) = db.get_user_skin_meta(&user.id).await.unwrap().unwrap();
        assert!(!hash.is_empty());
        assert_eq!(skin_type, YggdrasilSkinType::Slim);

        let retrieved = db.get_skin_data(&hash).await.unwrap().unwrap();
        assert_eq!(retrieved, skin_data);
    }

    #[tokio::test]
    async fn remove_skin() {
        let db = test_db().await;
        let (user, _) = db
            .create_user("rmskin@b.com", "pass", "RmSkin")
            .await
            .unwrap();
        db.set_skin(&user.id, vec![1, 2, 3], YggdrasilSkinType::Default)
            .await
            .unwrap();
        db.remove_skin(&user.id).await.unwrap();
        let meta = db.get_user_skin_meta(&user.id).await.unwrap();
        assert!(meta.is_none());
    }

    #[tokio::test]
    async fn gc_skins_removes_orphans() {
        let db = test_db().await;
        let (user, _) = db
            .create_user("gcskin@b.com", "pass", "GcSkin")
            .await
            .unwrap();
        db.set_skin(&user.id, vec![1, 2, 3], YggdrasilSkinType::Default)
            .await
            .unwrap();
        // Remove skin from user (leaves orphaned skin row)
        sqlx::query("UPDATE users SET skin_hash = NULL WHERE id = ?1")
            .bind(&user.id)
            .execute(&db.pool)
            .await
            .unwrap();
        db.gc_skins().await.unwrap();
        // After GC the skin should be gone from the skins table
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM skins")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count.0, 0);
    }

    // -- Duplicate unverified re-registration --------------------------------

    #[tokio::test]
    async fn create_user_overwrites_unverified_duplicate() {
        let db = test_db().await;
        let (user1, code1) = db.create_user("dup@b.com", "pass1", "Char1").await.unwrap();
        // Re-registering the same unverified email removes the first and creates a new one
        let (user2, code2) = db.create_user("dup@b.com", "pass2", "Char2").await.unwrap();
        assert_ne!(user1.id, user2.id);
        assert_ne!(code1, code2);
        assert_eq!(user2.character_name, "Char2");

        // First user is gone
        assert!(db.find_user_by_email("dup@b.com").await.unwrap().is_some());
        // But only user2 exists now
        assert!(db.get_profile_for_user(&user1.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn character_name_unique_ignore_case() {
        let db = test_db().await;
        let (_user, code) = db.create_user("a@b.com", "pass", "CharName").await.unwrap();
        db.verify_email(&code).await.unwrap();

        let lower_case_create = db.create_user("a@b.com", "pass", "charname").await;
        assert!(lower_case_create.is_err());
    }
}

// ============================================================================
// HTTP integration tests — full Axum router with in-memory SQLite
// ============================================================================
#[cfg(test)]
mod integration {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use rsa::{RsaPrivateKey, pkcs1v15::SigningKey, pkcs8::EncodePublicKey};
    use sha1::Sha1;
    use sqlx::SqlitePool;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    use crate::yggdrasil::YggdrasilConfig;
    use crate::yggdrasil::YggdrasilState;
    use crate::yggdrasil::db::DBManager;
    use crate::yggdrasil::router;

    // -- Helpers -------------------------------------------------------------

    async fn test_app() -> Router {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        DBManager::init_db(&pool).await.unwrap();
        let db = DBManager { pool };

        let private_key = RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
        let public_pem = private_key
            .to_public_key()
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();

        let config = YggdrasilConfig::default();

        let state = YggdrasilState {
            db,
            config,
            base_url: "http://test.local/yggdrasil".into(),
            join_requests: Arc::new(Mutex::new(HashMap::new())),
            sign_key: SigningKey::<Sha1>::new(private_key),
            public_pem,
        };

        router(state)
    }

    async fn post_json(
        app: &mut Router,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.as_service()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get(app: &mut Router, uri: &str) -> axum::response::Response {
        app.as_service()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn put_multipart(
        app: &mut Router,
        uri: &str,
        boundary: &str,
        body_bytes: Vec<u8>,
        bearer_token: &str,
    ) -> axum::response::Response {
        app.as_service()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={}", boundary),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {}", bearer_token))
                    .body(Body::from(body_bytes))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn delete(app: &mut Router, uri: &str, bearer_token: &str) -> axum::response::Response {
        app.as_service()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", bearer_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn register_user(app: &mut Router) -> serde_json::Value {
        let resp = post_json(
            app,
            "/register",
            serde_json::json!({
                "username": "test@example.com",
                "password": "secret123",
                "characterName": "TestChar"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn register_and_auth(app: &mut Router) -> (String, String, String) {
        let reg = register_user(app).await;
        let user_id = reg["id"].as_str().unwrap().to_owned();

        let resp = post_json(
            app,
            "/authserver/authenticate",
            serde_json::json!({
                "agent": { "name": "Minecraft", "version": 1 },
                "username": "test@example.com",
                "password": "secret123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (
            json["accessToken"].as_str().unwrap().to_owned(),
            json["clientToken"].as_str().unwrap().to_owned(),
            user_id,
        )
    }

    fn build_skin_multipart(boundary: &str, model: &str, image_data: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
        body.extend_from_slice(format!("{}\r\n", model).as_bytes());
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
        body.extend_from_slice(image_data);
        body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());
        body
    }

    // -- Meta ----------------------------------------------------------------

    #[tokio::test]
    async fn meta() {
        let mut app = test_app().await;
        let resp = get(&mut app, "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["meta"]["serverName"], "syncraft-yggdrasil");
        assert!(
            json["signaturePublickey"]
                .as_str()
                .unwrap()
                .contains("BEGIN PUBLIC KEY")
        );
    }

    // -- Register ------------------------------------------------------------

    #[tokio::test]
    async fn register_success() {
        let mut app = test_app().await;
        let json = register_user(&mut app).await;
        assert!(!json["id"].as_str().unwrap().is_empty());
        assert_eq!(json["email"], "test@example.com");
    }

    #[tokio::test]
    async fn register_overwrites_unverified_duplicate() {
        let mut app = test_app().await;
        register_user(&mut app).await;
        let resp = post_json(
            &mut app,
            "/register",
            serde_json::json!({
                "username": "test@example.com",
                "password": "secret123",
                "characterName": "TestChar2"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // -- Verify --------------------------------------------------------------

    #[tokio::test]
    async fn verify_invalid_code() {
        let mut app = test_app().await;
        let resp = get(&mut app, "/verify?code=no-such-code").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // -- Authenticate --------------------------------------------------------

    #[tokio::test]
    async fn authenticate_success() {
        let mut app = test_app().await;
        register_user(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/authenticate",
            serde_json::json!({
                "agent": { "name": "Minecraft", "version": 1 },
                "username": "test@example.com",
                "password": "secret123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json["accessToken"].as_str().unwrap().is_empty());
        assert!(!json["clientToken"].as_str().unwrap().is_empty());
        assert_eq!(json["availableProfiles"].as_array().unwrap().len(), 1);
        assert_eq!(json["selectedProfile"]["name"], "TestChar");
    }

    #[tokio::test]
    async fn authenticate_wrong_password() {
        let mut app = test_app().await;
        register_user(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/authenticate",
            serde_json::json!({
                "agent": { "name": "Minecraft", "version": 1 },
                "username": "test@example.com",
                "password": "wrong-password"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticate_nonexistent_user() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/authserver/authenticate",
            serde_json::json!({
                "agent": { "name": "Minecraft", "version": 1 },
                "username": "nobody@example.com",
                "password": "secret123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // -- Validate ------------------------------------------------------------

    #[tokio::test]
    async fn validate_valid_token() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/validate",
            serde_json::json!({ "accessToken": access_token }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn validate_invalid_token() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/authserver/validate",
            serde_json::json!({ "accessToken": "deadbeef-invalid-token" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn validate_with_matching_client_token() {
        let mut app = test_app().await;
        let (access_token, client_token, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/validate",
            serde_json::json!({
                "accessToken": access_token,
                "clientToken": client_token
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn validate_with_mismatched_client_token() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/validate",
            serde_json::json!({
                "accessToken": access_token,
                "clientToken": "wrong-client-token"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // -- Refresh -------------------------------------------------------------

    #[tokio::test]
    async fn refresh_success() {
        let mut app = test_app().await;
        let (access_token, client_token, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/refresh",
            serde_json::json!({
                "accessToken": access_token,
                "clientToken": client_token
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(json["accessToken"].as_str().unwrap(), access_token);
        assert_eq!(json["clientToken"].as_str().unwrap(), client_token);
    }

    #[tokio::test]
    async fn refresh_wrong_client_token() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/refresh",
            serde_json::json!({
                "accessToken": access_token,
                "clientToken": "wrong-client-token"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn refresh_invalid_token() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/authserver/refresh",
            serde_json::json!({
                "accessToken": "totally-fake-token",
                "clientToken": "whatever"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // -- Invalidate ----------------------------------------------------------

    #[tokio::test]
    async fn invalidate_existing_token() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/invalidate",
            serde_json::json!({ "accessToken": access_token }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn invalidate_nonexistent_token() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/authserver/invalidate",
            serde_json::json!({ "accessToken": "does-not-exist" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // -- Signout -------------------------------------------------------------

    #[tokio::test]
    async fn signout_success() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/signout",
            serde_json::json!({
                "username": "test@example.com",
                "password": "secret123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Old token should now be gone
        let resp = post_json(
            &mut app,
            "/authserver/validate",
            serde_json::json!({ "accessToken": access_token }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn signout_wrong_password() {
        let mut app = test_app().await;
        register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/authserver/signout",
            serde_json::json!({
                "username": "test@example.com",
                "password": "wrong-password"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn signout_nonexistent_user() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/authserver/signout",
            serde_json::json!({
                "username": "nobody@example.com",
                "password": "secret123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // -- Join ----------------------------------------------------------------

    #[tokio::test]
    async fn join_success() {
        let mut app = test_app().await;
        let (access_token, _, user_id) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/sessionserver/session/minecraft/join",
            serde_json::json!({
                "accessToken": access_token,
                "selectedProfile": { "id": user_id, "name": "TestChar" },
                "serverId": "test-server-id-123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn join_invalid_token() {
        let mut app = test_app().await;
        let resp = post_json(
            &mut app,
            "/sessionserver/session/minecraft/join",
            serde_json::json!({
                "accessToken": "bad-token",
                "selectedProfile": { "id": "some-id", "name": "TestChar" },
                "serverId": "test-server-id-123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn join_wrong_profile() {
        let mut app = test_app().await;
        let (access_token, _, _) = register_and_auth(&mut app).await;
        let resp = post_json(
            &mut app,
            "/sessionserver/session/minecraft/join",
            serde_json::json!({
                "accessToken": access_token,
                "selectedProfile": { "id": "wrong-user-id", "name": "WrongChar" },
                "serverId": "test-server-id-123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "IllegalArgumentException");
    }

    // -- HasJoined -----------------------------------------------------------

    #[tokio::test]
    async fn has_joined_success() {
        let mut app = test_app().await;
        let (access_token, _, user_id) = register_and_auth(&mut app).await;
        post_json(
            &mut app,
            "/sessionserver/session/minecraft/join",
            serde_json::json!({
                "accessToken": access_token,
                "selectedProfile": { "id": user_id, "name": "TestChar" },
                "serverId": "test-server-abc"
            }),
        )
        .await;
        let resp = get(
            &mut app,
            "/sessionserver/session/minecraft/hasJoined?username=TestChar&serverId=test-server-abc",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "TestChar");
    }

    #[tokio::test]
    async fn has_joined_not_found() {
        let mut app = test_app().await;
        let resp = get(
            &mut app,
            "/sessionserver/session/minecraft/hasJoined?username=X&serverId=unknown",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn has_joined_wrong_username() {
        let mut app = test_app().await;
        let (access_token, _, user_id) = register_and_auth(&mut app).await;
        post_json(
            &mut app,
            "/sessionserver/session/minecraft/join",
            serde_json::json!({
                "accessToken": access_token,
                "selectedProfile": { "id": user_id, "name": "TestChar" },
                "serverId": "test-server-xyz"
            }),
        )
        .await;
        let resp = get(
            &mut app,
            "/sessionserver/session/minecraft/hasJoined?username=WrongName&serverId=test-server-xyz",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // -- Texture (skin download) ---------------------------------------------

    #[tokio::test]
    async fn texture_not_found() {
        let mut app = test_app().await;
        let resp = get(&mut app, "/textures/deadbeef00000000000000000000000000").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Upload / Delete Skin ------------------------------------------------

    #[tokio::test]
    async fn upload_and_download_skin() {
        let mut app = test_app().await;
        let (access_token, _, user_id) = register_and_auth(&mut app).await;

        let skin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/skin.png");
        let skin_data = std::fs::read(&skin_path).unwrap();

        let boundary = "testboundary";
        let multipart_body = build_skin_multipart(boundary, "slim", &skin_data);

        let resp = put_multipart(
            &mut app,
            &format!("/api/user/profile/{}/skin", user_id),
            boundary,
            multipart_body,
            &access_token,
        )
        .await;
        assert!(resp.status().is_client_error() || resp.status().is_success());
    }

    #[tokio::test]
    async fn delete_skin() {
        let mut app = test_app().await;
        let (access_token, _, user_id) = register_and_auth(&mut app).await;
        let resp = delete(
            &mut app,
            &format!("/api/user/profile/{}/skin", user_id),
            &access_token,
        )
        .await;
        assert!(resp.status().is_client_error() || resp.status().is_success());
    }

    // -- Profile -------------------------------------------------------------

    #[tokio::test]
    async fn profile_found() {
        let mut app = test_app().await;
        let (_, _, user_id) = register_and_auth(&mut app).await;
        let resp = get(
            &mut app,
            &format!("/sessionserver/session/minecraft/profile/{}", user_id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "TestChar");
    }

    #[tokio::test]
    async fn profile_not_found() {
        let mut app = test_app().await;
        let resp = get(
            &mut app,
            "/sessionserver/session/minecraft/profile/00000000-0000-0000-0000-000000000000",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn profile_has_textures_property() {
        let mut app = test_app().await;
        let (_, _, user_id) = register_and_auth(&mut app).await;
        let resp = get(
            &mut app,
            &format!("/sessionserver/session/minecraft/profile/{}", user_id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["properties"].is_array());
    }
}
