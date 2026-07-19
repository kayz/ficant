#![allow(dead_code)]

use std::env;
use std::path::PathBuf;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

use ficant_application::ports::{AccessScope, AeadCursorCodec, CursorKey};
use ficant_domain::primitives::{OwnerRef, Ulid};
use ficant_storage::postgres::PostgresRepository;

pub async fn postgres_pool() -> PgPool {
    let database_url = env::var("FICANT_TEST_DATABASE_URL")
        .expect("FICANT_TEST_DATABASE_URL must point to the ready PostgreSQL 16 test database");
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("FICANT_TEST_DATABASE_URL must be reachable")
}

pub async fn reset_postgres(pool: &PgPool) {
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(pool)
    .await
    .expect("test database reset must succeed");
}

pub async fn migrate(pool: &PgPool) {
    let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    let migrator = sqlx::migrate::Migrator::new(migrations)
        .await
        .expect("migration directory must be readable");
    migrator
        .run(pool)
        .await
        .expect("forward migrations must apply");
}

pub fn s3_environment() -> (String, String, String, String) {
    let endpoint = env::var("FICANT_TEST_S3_ENDPOINT")
        .expect("FICANT_TEST_S3_ENDPOINT must point to the ready S3 service");
    let bucket = env::var("FICANT_TEST_S3_BUCKET")
        .expect("FICANT_TEST_S3_BUCKET must name the isolated test bucket");
    let access_key = env::var("FICANT_TEST_S3_ACCESS_KEY")
        .expect("FICANT_TEST_S3_ACCESS_KEY must be provided without logging its value");
    let secret_key = env::var("FICANT_TEST_S3_SECRET_KEY")
        .expect("FICANT_TEST_S3_SECRET_KEY must be provided without logging its value");
    (endpoint, bucket, access_key, secret_key)
}

pub fn access_scope(owner: &OwnerRef) -> AccessScope {
    AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![owner.owner_id().clone()],
    )
    .unwrap()
}

pub fn cursor_codec() -> Arc<AeadCursorCodec> {
    Arc::new(
        AeadCursorCodec::new(CursorKey::new("storage-test", [7_u8; 32]).unwrap(), vec![]).unwrap(),
    )
}

pub fn repository(pool: PgPool) -> PostgresRepository {
    PostgresRepository::new(pool, cursor_codec())
}
