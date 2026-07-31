pub mod artifacts;
mod codec;
pub(crate) mod common;
pub mod data_sources;
pub mod definitions;
mod execution;
pub mod facts;
pub mod journal;
pub mod phase4_execution;
pub mod positions;
pub mod runs;
pub mod signals;
pub mod snapshots;
pub mod subjects;

use std::sync::Arc;

use ficant_application::ports::AeadCursorCodec;
use sqlx::PgPool;

/// PostgreSQL-backed implementation of the Phase 1 application persistence ports.
#[derive(Clone)]
pub struct PostgresRepository {
    pool: PgPool,
    cursor_codec: Arc<AeadCursorCodec>,
}

impl PostgresRepository {
    #[must_use]
    pub fn new(pool: PgPool, cursor_codec: Arc<AeadCursorCodec>) -> Self {
        Self { pool, cursor_codec }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    #[must_use]
    pub fn cursor_codec(&self) -> &AeadCursorCodec {
        &self.cursor_codec
    }
}
