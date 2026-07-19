use ficant_application::ports::ApplicationResult;
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use sqlx::PgPool;

use super::S3BlobStore;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    deleted_staging: u64,
    deleted_immutable: u64,
}

impl CleanupReport {
    #[must_use]
    pub fn deleted_staging(self) -> u64 {
        self.deleted_staging
    }

    #[must_use]
    pub fn deleted_immutable(self) -> u64 {
        self.deleted_immutable
    }
}

#[derive(Clone)]
pub struct OrphanCleaner {
    store: S3BlobStore,
    pool: PgPool,
}

impl OrphanCleaner {
    #[must_use]
    pub fn new(store: S3BlobStore, pool: PgPool) -> Self {
        Self { store, pool }
    }

    /// Deletes old staging objects and immutable objects with no `PostgreSQL` reference.
    ///
    /// `cutoff_unix_seconds` is supplied by the application clock so cleanup does not
    /// depend on a host-local clock. Referenced immutable objects are never deleted.
    ///
    /// # Errors
    ///
    /// Returns storage unavailable when listing, reference lookup, or deletion fails.
    pub async fn cleanup_before(
        &self,
        cutoff_unix_seconds: i64,
    ) -> ApplicationResult<CleanupReport> {
        let staging: Vec<(String,)> = sqlx::query_as(
            "SELECT object_key FROM storage.staging_uploads
             WHERE updated_at <= to_timestamp($1)
             ORDER BY object_key",
        )
        .bind(cutoff_unix_seconds)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| storage_error())?;
        let mut deleted_staging = 0_u64;
        for (key,) in staging {
            let mut transaction = self.pool.begin().await.map_err(|_| storage_error())?;
            let locked: Option<(String,)> = sqlx::query_as(
                "SELECT object_key FROM storage.staging_uploads
                 WHERE object_key = $1 AND updated_at <= to_timestamp($2)
                 FOR UPDATE",
            )
            .bind(&key)
            .bind(cutoff_unix_seconds)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error())?;
            if locked.is_none() {
                transaction.commit().await.map_err(|_| storage_error())?;
                continue;
            }
            self.store.delete_object(&key).await?;
            sqlx::query("DELETE FROM storage.staging_uploads WHERE object_key = $1")
                .bind(&key)
                .execute(&mut *transaction)
                .await
                .map_err(|_| storage_error())?;
            transaction.commit().await.map_err(|_| storage_error())?;
            deleted_staging = deleted_staging.checked_add(1).ok_or_else(storage_error)?;
        }

        let candidates: Vec<(String, String)> = sqlx::query_as(
            "SELECT content_hash::text, object_key FROM storage.orphan_candidates
             WHERE created_at <= to_timestamp($1)
             ORDER BY content_hash",
        )
        .bind(cutoff_unix_seconds)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| storage_error())?;
        let mut deleted_immutable = 0_u64;
        for (hash, _) in candidates {
            let mut transaction = self.pool.begin().await.map_err(|_| storage_error())?;
            let candidate: Option<(String,)> = sqlx::query_as(
                "SELECT object_key FROM storage.orphan_candidates
                 WHERE content_hash = $1
                 FOR UPDATE",
            )
            .bind(&hash)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error())?;
            let Some((key,)) = candidate else {
                transaction.commit().await.map_err(|_| storage_error())?;
                continue;
            };
            let referenced: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM storage.blobs WHERE content_hash = $1)",
            )
            .bind(&hash)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error())?;
            if !referenced {
                self.store.delete_object(&key).await?;
                deleted_immutable = deleted_immutable.checked_add(1).ok_or_else(storage_error)?;
            }
            sqlx::query("DELETE FROM storage.orphan_candidates WHERE content_hash = $1")
                .bind(&hash)
                .execute(&mut *transaction)
                .await
                .map_err(|_| storage_error())?;
            transaction.commit().await.map_err(|_| storage_error())?;
        }
        Ok(CleanupReport {
            deleted_staging,
            deleted_immutable,
        })
    }
}

fn storage_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
