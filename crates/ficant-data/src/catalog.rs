use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ficant_domain::market::{DataSource, DataSourceKind};
use sqlx::postgres::PgPoolOptions;

use crate::{DataError, DataResult, FileNdjsonQuoteSource, PostgresQuoteSource, RawQuoteSource};

/// One adapter registered by trusted server configuration.
pub struct RegisteredQuoteSource {
    kind: DataSourceKind,
    connection_binding: String,
    source: Arc<dyn RawQuoteSource>,
}

impl RegisteredQuoteSource {
    pub fn new(
        kind: DataSourceKind,
        connection_binding: impl Into<String>,
        source: Arc<dyn RawQuoteSource>,
    ) -> DataResult<Self> {
        let connection_binding = connection_binding.into();
        if connection_binding.trim().is_empty()
            || connection_binding != connection_binding.trim()
            || connection_binding.len() > 128
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            kind,
            connection_binding,
            source,
        })
    }
}

/// Server-owned catalog. Public import requests never select an adapter or supply a connection.
pub struct QuoteSourceCatalog {
    sources: BTreeMap<(u8, String), Arc<dyn RawQuoteSource>>,
}

impl QuoteSourceCatalog {
    pub fn new(registrations: Vec<RegisteredQuoteSource>) -> DataResult<Self> {
        if registrations.is_empty() {
            return Err(DataError::InvalidConfiguration);
        }
        let mut sources = BTreeMap::new();
        for registration in registrations {
            let key = (
                source_kind_code(registration.kind),
                registration.connection_binding,
            );
            if sources.insert(key, registration.source).is_some() {
                return Err(DataError::InvalidConfiguration);
            }
        }
        Ok(Self { sources })
    }

    /// Constructs the fixed production adapter catalog from trusted process configuration.
    pub fn production(
        file_connection_binding: impl Into<String>,
        file_root: PathBuf,
        postgres_connection_binding: impl Into<String>,
        postgres_database_url: &str,
    ) -> DataResult<Self> {
        let file_connection_binding = file_connection_binding.into();
        let postgres_connection_binding = postgres_connection_binding.into();
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect_lazy(postgres_database_url)
            .map_err(|_| DataError::InvalidConfiguration)?;
        let file: Arc<dyn RawQuoteSource> = Arc::new(FileNdjsonQuoteSource::new(
            file_connection_binding.clone(),
            file_root,
        )?);
        let postgres: Arc<dyn RawQuoteSource> = Arc::new(PostgresQuoteSource::new(
            postgres_connection_binding.clone(),
            pool,
        )?);
        Self::new(vec![
            RegisteredQuoteSource::new(DataSourceKind::FileNdjson, file_connection_binding, file)?,
            RegisteredQuoteSource::new(
                DataSourceKind::Postgres,
                postgres_connection_binding,
                postgres,
            )?,
        ])
    }

    /// Resolves solely from the exact administrator-registered `DataSource` definition.
    pub fn resolve(&self, source: &DataSource) -> DataResult<Arc<dyn RawQuoteSource>> {
        self.sources
            .get(&(
                source_kind_code(source.kind()),
                source.connection_binding().to_owned(),
            ))
            .cloned()
            .ok_or(DataError::InvalidConfiguration)
    }
}

const fn source_kind_code(kind: DataSourceKind) -> u8 {
    match kind {
        DataSourceKind::FileNdjson => 1,
        DataSourceKind::Postgres => 2,
    }
}
