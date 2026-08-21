#![allow(dead_code, clippy::missing_panics_doc, clippy::must_use_candidate)]

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use ficant_application::ports::{AccessScope, AeadCursorCodec, CursorKey};
use ficant_domain::primitives::{OwnerRef, Ulid};
use ficant_storage::postgres::PostgresRepository;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

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
        "DROP SCHEMA IF EXISTS portfolio CASCADE;
         DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS analytics CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
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
    (
        env::var("FICANT_TEST_S3_ENDPOINT").expect("FICANT_TEST_S3_ENDPOINT must be set"),
        env::var("FICANT_TEST_S3_BUCKET").expect("FICANT_TEST_S3_BUCKET must be set"),
        env::var("FICANT_TEST_S3_ACCESS_KEY").expect("FICANT_TEST_S3_ACCESS_KEY must be set"),
        env::var("FICANT_TEST_S3_SECRET_KEY").expect("FICANT_TEST_S3_SECRET_KEY must be set"),
    )
}

pub fn access_scope(owner: &OwnerRef) -> AccessScope {
    AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![owner.owner_id().clone()],
    )
    .unwrap()
}

pub fn repository(pool: PgPool) -> PostgresRepository {
    let cursor = Arc::new(
        AeadCursorCodec::new(CursorKey::new("worker-sit", [9_u8; 32]).unwrap(), vec![]).unwrap(),
    );
    PostgresRepository::new(pool, cursor)
}
