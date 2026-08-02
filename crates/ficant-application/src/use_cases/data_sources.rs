use ficant_domain::VersionedDefinition;
use ficant_domain::market::DataSource;
use ficant_domain::primitives::VersionRef;

use crate::ports::{AccessScope, ApplicationResult, DataSourceRepository, RegisterDataSource};
use crate::{ApplicationError, ApplicationErrorCategory};

/// Typed application boundary for immutable `DataSource` registration and exact lookup.
pub struct DataSourceUseCase<'a> {
    repository: &'a dyn DataSourceRepository,
}

impl<'a> DataSourceUseCase<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn DataSourceRepository) -> Self {
        Self { repository }
    }

    /// Registers one externally supplied, explicitly typed immutable `DataSource` version.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a legacy untyped definition and otherwise propagates the
    /// append-only repository result.
    pub async fn register(&self, command: RegisterDataSource) -> ApplicationResult<DataSource> {
        if command.value().price_source_type().is_none() {
            return Err(validation());
        }
        self.repository.register(command).await
    }

    /// Resolves one exact, authorized, explicitly typed `DataSource` version.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an absent exact version, validation failure for a legacy untyped
    /// version, or the authorization/repository error.
    pub async fn get_exact(
        &self,
        scope: &AccessScope,
        reference: &VersionRef,
    ) -> ApplicationResult<DataSource> {
        let value = self
            .repository
            .get_exact(scope, reference.clone())
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(value.owner())?;
        if value.id() != reference.id()
            || value.version() != reference.version().get()
            || value.price_source_type().is_none()
        {
            return Err(validation());
        }
        Ok(value)
    }
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
