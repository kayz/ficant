use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use ficant_domain::ContentAddressed;
use ficant_domain::VersionedDefinition;
use ficant_domain::analytics::{AnalyticsMode, AnalyticsObjectRef, FixedDecimal};
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{ValuationValueRole, VerificationStatus};
use ficant_domain::portfolio::{
    Benchmark, Book, Portfolio, PortfolioGroup, PortfolioMetricConvention, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{Position, PositionSnapshot};

use crate::ports::{
    AeadCursorCodec, ApplicationResult, AuthorizedPrincipal, Cursor,
    CurveSnapshotMetadataRepository, DefinitionRepository, DefinitionValue, ExactCatalogRead,
    ExactPortfolioScope, ExactPortfolioScopeKind, InstrumentSubtype, IntegrityEventSink,
    NormalizedPortfolioContext, NormalizedPortfolioContextResolution,
    PORTFOLIO_CATALOG_MAX_PAGE_SIZE, PORTFOLIO_READ_SCOPE, PortfolioAnalyticsAuthorityQuery,
    PortfolioAnalyticsAuthorityRepository, PortfolioAnalyticsEvidenceBinding,
    PortfolioAnalyticsEvidenceKind, PortfolioBondRatesAuthority,
    PortfolioBondRatesAuthorityResolution, PortfolioCatalogEntry, PortfolioCatalogEvidenceBinding,
    PortfolioCatalogEvidenceRole, PortfolioCatalogFilter, PortfolioCatalogPage,
    PortfolioCatalogRepository, PortfolioCatalogSnapshot, PortfolioCatalogSortKey,
    PortfolioCatalogTemporalScope, PortfolioContextInput, PortfolioCurrencyMode,
    PortfolioImmutableSnapshotAuthority, PortfolioLookThroughMode, PortfolioPeriodPreset,
    PortfolioRatesUnitAuthority, PortfolioRatesUnitRole, PortfolioRiskAuthority,
    PortfolioScopeAuthority, PortfolioScopeSelector, RequiredVerifiedBlobRead,
    ResolvedPortfolioAggregationInputs, ResolvedPortfolioAnalyticsAuthority, SafeTraceContext,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, VisibleCatalogRecord, definition_content_hash,
};
use crate::use_cases::verified_reads::{VerifiedSnapshotRead, VerifiedSnapshotReader};
use crate::{ApplicationError, ApplicationErrorCategory};

const CURSOR_SCHEMA: &str = "portfolio-catalog-cursor/v1";

type CatalogPageHierarchy = (
    Vec<VisibleCatalogRecord<Book>>,
    Vec<VisibleCatalogRecord<PortfolioGroup>>,
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListPortfolioCatalogCommand {
    filter: PortfolioCatalogFilter,
    cursor: Option<String>,
    limit: u32,
}

impl ListPortfolioCatalogCommand {
    /// Builds a bounded catalog page command.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless limit is in `1..=1000`.
    pub fn new(
        filter: PortfolioCatalogFilter,
        cursor: Option<String>,
        limit: u32,
    ) -> ApplicationResult<Self> {
        if limit == 0 || limit > PORTFOLIO_CATALOG_MAX_PAGE_SIZE {
            return Err(validation());
        }
        if cursor
            .as_ref()
            .is_some_and(|value| value.is_empty() || value != value.trim())
        {
            return Err(validation());
        }
        Ok(Self {
            filter,
            cursor,
            limit,
        })
    }

    #[must_use]
    pub const fn filter(&self) -> &PortfolioCatalogFilter {
        &self.filter
    }

    #[must_use]
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    #[must_use]
    pub const fn limit(&self) -> u32 {
        self.limit
    }
}

pub struct ListPortfolioCatalog<'a> {
    repository: &'a dyn PortfolioCatalogRepository,
    cursor_codec: &'a AeadCursorCodec,
}

impl<'a> ListPortfolioCatalog<'a> {
    #[must_use]
    pub const fn new(
        repository: &'a dyn PortfolioCatalogRepository,
        cursor_codec: &'a AeadCursorCodec,
    ) -> Self {
        Self {
            repository,
            cursor_codec,
        }
    }

    /// Resolves the caller-visible owner and Subject for one typed catalog selector.
    ///
    /// # Errors
    ///
    /// Returns not-found when no authorized bitemporal row exists and fails closed on ambiguous,
    /// unauthorized, or invalid-time repository results.
    pub async fn resolve_scope_authority(
        &self,
        principal: &AuthorizedPrincipal,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority> {
        authorize_reader(principal)?;
        if knowledge_at.instant() < valuation_at.instant() {
            return Err(validation());
        }
        let candidates = self
            .repository
            .find_scope_authorities(
                principal.access_scope(),
                selector,
                valuation_at,
                knowledge_at,
            )
            .await?;
        for candidate in &candidates {
            principal.access_scope().authorize(candidate.owner())?;
        }
        match candidates.as_slice() {
            [authority] => Ok(authority.clone()),
            [] => Err(not_found()),
            _ => Err(state_conflict()),
        }
    }

    /// Returns one authorized, deterministic catalog page under a frozen knowledge boundary.
    ///
    /// # Errors
    ///
    /// Fails closed on role/scope/owner drift, malformed or filter-drifted cursors, broken exact
    /// hierarchy, duplicate order keys, invalid bitemporal rows, or repository failure.
    pub async fn execute(
        &self,
        principal: &AuthorizedPrincipal,
        command: ListPortfolioCatalogCommand,
    ) -> ApplicationResult<PortfolioCatalogPage> {
        authorize(principal, command.filter().temporal().owner())?;
        let after = command
            .cursor()
            .map(|token| self.resume_cursor(principal, command.filter(), token))
            .transpose()?;
        let snapshot = self
            .repository
            .read_catalog_snapshot(principal.access_scope(), command.filter().temporal())
            .await?;
        validate_snapshot(principal, command.filter(), &snapshot)?;
        let hierarchy = CatalogHierarchy::new(&snapshot)?;
        let mut entries = hierarchy.filtered_portfolios(command.filter())?;
        entries.sort_by(|left, right| left.sort_key().cmp(right.sort_key()));
        if entries
            .windows(2)
            .any(|pair| pair[0].sort_key() == pair[1].sort_key())
        {
            return Err(state_conflict());
        }
        if let Some(after) = after {
            entries.retain(|entry| entry.sort_key() > &after);
        }

        let limit = usize::try_from(command.limit()).map_err(|_| validation())?;
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next_cursor = if has_more {
            let last = entries.last().ok_or_else(state_conflict)?;
            Some(self.issue_cursor(principal, command.filter(), last.sort_key())?)
        } else {
            None
        };
        let (books, groups) = hierarchy.page_hierarchy(&entries)?;
        Ok(PortfolioCatalogPage::new(
            books,
            groups,
            entries,
            next_cursor,
            command.filter().fingerprint().clone(),
        ))
    }

    /// Resolves all six context dimensions to exact immutable catalog bindings.
    ///
    /// # Errors
    ///
    /// Fails closed on authorization, absent/ambiguous hierarchy, mixed conventions, unsupported
    /// currency authority, bitemporal drift, or any non-exact member binding.
    pub async fn normalize_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        self.normalize_context_with_evidence(principal, owner, subject_ref, input)
            .await
            .map(NormalizedPortfolioContextResolution::into_context)
    }

    /// Resolves the context and preserves the exact visible catalog records from the same read.
    ///
    /// # Errors
    ///
    /// Returns the same closed errors as [`Self::normalize_context`] and additionally rejects an
    /// incomplete or duplicate evidence set.
    pub async fn normalize_context_with_evidence(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution> {
        authorize(principal, &owner)?;
        let temporal = PortfolioCatalogTemporalScope::new(
            owner.clone(),
            subject_ref.clone(),
            input.valuation_at.clone(),
            input.knowledge_at.clone(),
        )?;
        let snapshot = self
            .repository
            .read_catalog_snapshot(principal.access_scope(), &temporal)
            .await?;
        let empty_filter = PortfolioCatalogFilter::new(temporal.clone(), Vec::new(), None)?;
        validate_snapshot(principal, &empty_filter, &snapshot)?;
        let hierarchy = CatalogHierarchy::new(&snapshot)?;
        let (selected, members) = hierarchy.resolve_members(&input.scope, input.look_through)?;
        if members.is_empty() {
            return Err(not_found());
        }
        let convention = hierarchy.single_convention(&members)?;
        let benchmark = hierarchy.benchmark_by_id(&input.benchmark_id)?;
        let currency_unit = self
            .repository
            .resolve_currency_unit(principal.access_scope(), &owner, "CNY")
            .await?
            .ok_or_else(not_found)?;
        let (period_from, period_to) = period_window(&input.valuation_at, input.period)?;
        let member_portfolios = members
            .iter()
            .map(|record| exact_lineage(record.value()))
            .collect::<ApplicationResult<Vec<_>>>()?;
        let context = NormalizedPortfolioContext {
            owner,
            subject_ref,
            scope: ExactPortfolioScope::new(selected.clone(), member_portfolios),
            valuation_at: input.valuation_at,
            knowledge_at: input.knowledge_at,
            currency: input.currency,
            currency_unit,
            look_through: input.look_through,
            benchmark: ficant_domain::portfolio::BenchmarkRef::new(
                benchmark.value().reference().clone(),
                benchmark.value().content_hash().clone(),
            ),
            period: input.period,
            period_from,
            period_to,
            metric_convention: ficant_domain::portfolio::PortfolioMetricConventionRef::new(
                convention.value().reference().clone(),
                convention.value().content_hash().clone(),
            ),
        };
        let mut catalog_evidence = vec![hierarchy.selected_evidence(&selected, &temporal)?];
        for member in &members {
            catalog_evidence.push(catalog_record_evidence(
                PortfolioCatalogEvidenceRole::MemberPortfolio,
                member,
                &temporal,
            )?);
        }
        catalog_evidence.push(catalog_record_evidence(
            PortfolioCatalogEvidenceRole::Benchmark,
            benchmark,
            &temporal,
        )?);
        catalog_evidence.push(catalog_record_evidence(
            PortfolioCatalogEvidenceRole::MetricConvention,
            convention,
            &temporal,
        )?);
        validate_catalog_evidence_set(
            &context,
            &members,
            benchmark,
            convention,
            &catalog_evidence,
        )?;
        NormalizedPortfolioContextResolution::new(context, catalog_evidence)
    }

    /// Selects the first visible active Portfolio under the frozen catalog order.
    ///
    /// # Errors
    ///
    /// Returns not-found for an empty authorized catalog and otherwise the same fail-closed
    /// errors as context normalization.
    pub async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        authorize(principal, &owner)?;
        let discovery_temporal = PortfolioCatalogTemporalScope::new(
            owner.clone(),
            subject_ref.clone(),
            knowledge_at.clone(),
            knowledge_at.clone(),
        )?;
        let discovery_filter = PortfolioCatalogFilter::new(
            discovery_temporal.clone(),
            vec![PortfolioStatus::Active],
            None,
        )?;
        let snapshot = self
            .repository
            .read_catalog_snapshot(principal.access_scope(), &discovery_temporal)
            .await?;
        validate_snapshot(principal, &discovery_filter, &snapshot)?;
        let hierarchy = CatalogHierarchy::new(&snapshot)?;
        let mut entries = hierarchy.filtered_portfolios(&discovery_filter)?;
        entries.sort_by(|left, right| left.sort_key().cmp(right.sort_key()));
        let portfolio = entries.first().ok_or_else(not_found)?.record().value();
        self.normalize_context(
            principal,
            owner,
            subject_ref,
            PortfolioContextInput {
                scope: PortfolioScopeSelector::Portfolio(portfolio.reference().id().clone()),
                valuation_at: portfolio.position_snapshot().observed_at().clone(),
                knowledge_at,
                currency: PortfolioCurrencyMode::Cny,
                look_through: PortfolioLookThroughMode::None,
                benchmark_id: portfolio.benchmark().reference().id().clone(),
                period: PortfolioPeriodPreset::OneDay,
            },
        )
        .await
    }

    /// Re-reads every exact normalized catalog binding before aggregation handoff.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure for any missing, owner/Subject/hash/version/time drifted,
    /// duplicated, or convention-inconsistent input.
    pub async fn resolve_aggregation_inputs(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        scope.authorize(&context.owner)?;
        let temporal = PortfolioCatalogTemporalScope::new(
            context.owner.clone(),
            context.subject_ref.clone(),
            context.valuation_at.clone(),
            context.knowledge_at.clone(),
        )?;
        let selected_evidence = resolve_selected_scope_evidence(
            self.repository,
            scope,
            &temporal,
            context.scope.selected(),
        )
        .await?;
        let portfolios =
            read_exact_member_portfolios(self.repository, scope, &temporal, context).await?;
        let convention = self
            .repository
            .read_metric_convention_exact(
                scope,
                &ExactCatalogRead::new(
                    temporal.clone(),
                    context.metric_convention.reference().clone(),
                    context.metric_convention.content_hash().clone(),
                ),
            )
            .await?
            .ok_or_else(integrity)?;
        let benchmark = self
            .repository
            .read_benchmark_exact(
                scope,
                &ExactCatalogRead::new(
                    temporal.clone(),
                    context.benchmark.reference().clone(),
                    context.benchmark.content_hash().clone(),
                ),
            )
            .await?
            .ok_or_else(integrity)?;
        let benchmark_snapshot = benchmark.value().position_snapshot().clone();
        let catalog_evidence = resolved_catalog_evidence(
            selected_evidence,
            &temporal,
            context,
            &portfolios,
            &benchmark,
            &convention,
        )?;
        Ok(ResolvedPortfolioAggregationInputs {
            exact_scope: context.scope.clone(),
            portfolios,
            convention,
            benchmark,
            benchmark_snapshot,
            catalog_evidence,
        })
    }

    /// Required-reads all catalog objects and compares them with the normalization-time set.
    ///
    /// # Errors
    ///
    /// Fails closed when any role, identity, hash, visibility time, or effective time changed
    /// between selector normalization and the numerical handoff boundary.
    pub async fn resolve_aggregation_inputs_with_evidence(
        &self,
        scope: &crate::ports::AccessScope,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        let resolved = self
            .resolve_aggregation_inputs(scope, resolution.context())
            .await?;
        if resolved.catalog_evidence != resolution.catalog_evidence() {
            return Err(integrity());
        }
        Ok(resolved)
    }

    fn issue_cursor(
        &self,
        principal: &AuthorizedPrincipal,
        filter: &PortfolioCatalogFilter,
        key: &PortfolioCatalogSortKey,
    ) -> ApplicationResult<String> {
        let opaque = format!(
            "{CURSOR_SCHEMA}:{}:{}:{}:{}:{}",
            encode_hex(filter.fingerprint().content_hash().as_bytes()),
            encode_hex(key.book_code().as_bytes()),
            encode_hex(key.group_path().as_bytes()),
            encode_hex(key.portfolio_code().as_bytes()),
            key.version(),
        );
        Ok(
            Cursor::issue(self.cursor_codec, principal.access_scope(), opaque)?
                .as_str()
                .to_owned(),
        )
    }

    fn resume_cursor(
        &self,
        principal: &AuthorizedPrincipal,
        filter: &PortfolioCatalogFilter,
        token: &str,
    ) -> ApplicationResult<PortfolioCatalogSortKey> {
        let cursor = Cursor::resume(
            self.cursor_codec,
            principal.access_scope(),
            token.to_owned(),
        )?;
        let mut fields = cursor.opaque_value().split(':');
        let (
            Some(schema),
            Some(fingerprint),
            Some(book),
            Some(group),
            Some(portfolio),
            Some(version),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return Err(forbidden());
        };
        if fields.next().is_some()
            || schema != CURSOR_SCHEMA
            || fingerprint != encode_hex(filter.fingerprint().content_hash().as_bytes())
        {
            return Err(forbidden());
        }
        let book = decode_utf8_hex(book).ok_or_else(forbidden)?;
        let group = decode_utf8_hex(group).ok_or_else(forbidden)?;
        let portfolio = decode_utf8_hex(portfolio).ok_or_else(forbidden)?;
        let version = version.parse::<u64>().map_err(|_| forbidden())?;
        PortfolioCatalogSortKey::new(book, group, portfolio, version).map_err(|_| forbidden())
    }
}

pub struct ResolvePortfolioAnalyticsAuthority<'a> {
    authority: &'a dyn PortfolioAnalyticsAuthorityRepository,
    definitions: &'a dyn DefinitionRepository,
    curves: &'a dyn CurveSnapshotMetadataRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ResolvePortfolioAnalyticsAuthority<'a> {
    #[must_use]
    pub const fn new(
        authority: &'a dyn PortfolioAnalyticsAuthorityRepository,
        definitions: &'a dyn DefinitionRepository,
        curves: &'a dyn CurveSnapshotMetadataRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
    ) -> Self {
        Self {
            authority,
            definitions,
            curves,
            snapshots,
            blobs,
            integrity_events,
        }
    }

    /// Required-reads every selected analytics input before any numerical handoff.
    ///
    /// # Errors
    ///
    /// Fails closed on missing or ambiguous authority, aggregate-hash drift, owner/Subject/time
    /// drift, incomplete Unit roles, or any Definition/Snapshot/Valuation mismatch.
    pub async fn execute(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
        validate_analytics_context(scope, context, snapshot)?;
        let query = analytics_authority_query(context, snapshot)?;
        let candidates = self.authority.read_candidates(scope, &query).await?;
        let candidate = match candidates.as_slice() {
            [candidate] => candidate,
            [] => return Err(not_found()),
            _ => return Err(state_conflict()),
        };
        validate_analytics_candidate(context, snapshot, candidate)?;

        let units = self
            .resolve_units(scope, &context.owner, &candidate.units)
            .await?;
        let currency_unit = unit_for_role(&units, PortfolioRatesUnitRole::CurrencyAmount)?;
        let rate_unit = unit_for_role(&units, PortfolioRatesUnitRole::Rate)?;
        let dv01_unit = unit_for_role(&units, PortfolioRatesUnitRole::Dv01)?;
        if currency_unit.reference != context.currency_unit {
            return Err(integrity());
        }

        let trace = authority_trace(&candidate.content_hash)?;
        let mut evidence = initial_analytics_evidence(snapshot, &units);
        self.validate_curve(
            scope,
            context,
            candidate,
            &currency_unit.reference,
            trace.clone(),
            &mut evidence,
        )
        .await?;
        self.validate_data_snapshots(scope, context, candidate, trace, &mut evidence)
            .await?;
        self.validate_tax_rule(scope, context, candidate, &mut evidence)
            .await?;
        let bond_rates = self
            .resolve_bonds(
                scope,
                context,
                snapshot,
                candidate,
                &units,
                currency_unit,
                rate_unit,
                &mut evidence,
            )
            .await?;
        let risk = PortfolioRiskAuthority {
            curve_snapshot_id: candidate.curve_snapshot.id.clone(),
            dv01_unit: dv01_unit.reference.clone(),
            futures_data_snapshot_id: candidate
                .futures_data_snapshot
                .as_ref()
                .map(|binding| binding.id.clone()),
        };
        ResolvedPortfolioAnalyticsAuthority::new(
            candidate.authority_set_id.clone(),
            &candidate.content_hash,
            risk,
            units,
            bond_rates,
            evidence,
        )
    }
}

impl ResolvePortfolioAnalyticsAuthority<'_> {
    async fn validate_data_snapshots(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
        trace: SafeTraceContext,
        evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<()> {
        let data_snapshot = self
            .read_data_snapshot(
                scope,
                &context.owner,
                &candidate.data_snapshot,
                &context.valuation_at,
                &context.knowledge_at,
                trace.clone(),
            )
            .await?;
        push_evidence(
            evidence,
            snapshot_evidence(
                PortfolioAnalyticsEvidenceKind::DataSnapshot,
                data_snapshot.id(),
                data_snapshot.content_hash(),
                data_snapshot.as_of(),
                data_snapshot.visible_at(),
            ),
        );
        if let Some(binding) = &candidate.futures_data_snapshot {
            let futures = self
                .read_data_snapshot(
                    scope,
                    &context.owner,
                    binding,
                    &context.valuation_at,
                    &context.knowledge_at,
                    trace,
                )
                .await?;
            push_evidence(
                evidence,
                snapshot_evidence(
                    PortfolioAnalyticsEvidenceKind::FuturesDataSnapshot,
                    futures.id(),
                    futures.content_hash(),
                    futures.as_of(),
                    futures.visible_at(),
                ),
            );
        }
        Ok(())
    }

    async fn resolve_units(
        &self,
        scope: &crate::ports::AccessScope,
        owner: &OwnerRef,
        bindings: &[crate::ports::PortfolioUnitAuthorityBinding],
    ) -> ApplicationResult<Vec<PortfolioRatesUnitAuthority>> {
        let expected = all_rates_unit_roles();
        if bindings.len() != expected.len() {
            return Err(integrity());
        }
        let mut resolved = Vec::with_capacity(bindings.len());
        let mut seen_roles = BTreeSet::new();
        let mut seen_units = BTreeSet::new();
        for binding in bindings {
            if !seen_roles.insert(binding.role) || !seen_units.insert(binding.reference.clone()) {
                return Err(integrity());
            }
            let reference = VersionRef::new(
                binding.reference.unit_id().clone(),
                binding.reference.version(),
            );
            let value = self
                .read_exact_definition(scope, owner, &reference, Some(&binding.content_hash))
                .await?;
            let DefinitionValue::Unit(unit) = value else {
                return Err(integrity());
            };
            if unit.dimension() != binding.role.expected_dimension() || unit.scale() > 28 {
                return Err(integrity());
            }
            resolved.push(PortfolioRatesUnitAuthority {
                role: binding.role,
                reference: binding.reference.clone(),
                content_hash: binding.content_hash.clone(),
                dimension: unit.dimension().to_owned(),
                scale: unit.scale(),
            });
        }
        resolved.sort_by_key(|unit| unit.role);
        if resolved.iter().map(|unit| unit.role).ne(expected) {
            return Err(integrity());
        }
        Ok(resolved)
    }

    #[allow(clippy::too_many_arguments)]
    async fn validate_curve(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
        currency_unit: &UnitRef,
        trace: SafeTraceContext,
        evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<()> {
        let metadata = self
            .curves
            .get_curve_snapshot_metadata(scope, candidate.curve_snapshot.id.clone())
            .await?
            .ok_or_else(not_found)?;
        let curve = metadata.snapshot();
        let curve_visible_at = curve.visible_at().ok_or_else(integrity)?;
        if curve.id() != &candidate.curve_snapshot.id
            || curve.content_hash() != &candidate.curve_snapshot.content_hash
            || curve.owner() != &context.owner
            || curve.as_of() != &context.valuation_at
            || curve_visible_at.instant() > context.knowledge_at.instant()
            || curve.currency() != currency_unit
        {
            return Err(integrity());
        }
        let read = RequiredVerifiedBlobRead::new(
            scope.clone(),
            context.owner.clone(),
            VerifiedReadResourceKind::CurveSnapshot,
            curve.id().clone(),
            VerifiedBlobRole::CurvePoints,
            curve.content_hash().clone(),
            metadata.blob_size(),
            trace,
        )?;
        self.blobs
            .read_required(&read, self.integrity_events)
            .await?;
        push_evidence(
            evidence,
            snapshot_evidence(
                PortfolioAnalyticsEvidenceKind::CurveSnapshot,
                curve.id(),
                curve.content_hash(),
                curve.as_of(),
                curve_visible_at,
            ),
        );
        let calendar = self
            .read_exact_definition(scope, &context.owner, curve.calendar(), None)
            .await?;
        let DefinitionValue::Calendar(calendar) = calendar else {
            return Err(integrity());
        };
        validate_effective(
            calendar.effective().from(),
            calendar.effective().to(),
            &context.valuation_at,
        )?;
        push_evidence(
            evidence,
            effective_object_evidence(
                PortfolioAnalyticsEvidenceKind::Calendar,
                curve.calendar(),
                definition_content_hash(&DefinitionValue::Calendar(calendar.clone())),
                calendar.effective().from(),
                calendar.effective().to(),
            ),
        );
        let rule = self
            .read_exact_definition(scope, &context.owner, curve.rule_pack(), None)
            .await?;
        let DefinitionValue::MarketRulePack(rule) = rule else {
            return Err(integrity());
        };
        validate_verified_rule(&rule, &context.valuation_at)?;
        push_evidence(
            evidence,
            effective_object_evidence(
                PortfolioAnalyticsEvidenceKind::CurveRulePack,
                curve.rule_pack(),
                definition_content_hash(&DefinitionValue::MarketRulePack(rule.clone())),
                rule.effective().from(),
                rule.effective().to(),
            ),
        );
        Ok(())
    }

    async fn validate_tax_rule(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
        evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<()> {
        let value = self
            .read_exact_definition(
                scope,
                &context.owner,
                candidate.tax_rule_pack.version_ref(),
                Some(candidate.tax_rule_pack.content_hash()),
            )
            .await?;
        let DefinitionValue::MarketRulePack(rule) = value else {
            return Err(integrity());
        };
        validate_verified_rule(&rule, &context.valuation_at)?;
        push_evidence(
            evidence,
            effective_object_evidence(
                PortfolioAnalyticsEvidenceKind::TaxRulePack,
                candidate.tax_rule_pack.version_ref(),
                candidate.tax_rule_pack.content_hash().clone(),
                rule.effective().from(),
                rule.effective().to(),
            ),
        );
        Ok(())
    }

    async fn read_exact_definition(
        &self,
        scope: &crate::ports::AccessScope,
        owner: &OwnerRef,
        reference: &VersionRef,
        expected_hash: Option<&ContentHash>,
    ) -> ApplicationResult<DefinitionValue> {
        let value = self
            .definitions
            .get_version(scope, reference.id().clone(), reference.version())
            .await?
            .ok_or_else(not_found)?;
        if value.identity() != reference.id().as_str()
            || value.version() != reference.version().get()
            || value.owner() != owner
            || expected_hash.is_some_and(|hash| definition_content_hash(&value) != *hash)
        {
            return Err(integrity());
        }
        Ok(value)
    }

    async fn read_data_snapshot(
        &self,
        scope: &crate::ports::AccessScope,
        owner: &OwnerRef,
        binding: &PortfolioImmutableSnapshotAuthority,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
        trace: SafeTraceContext,
    ) -> ApplicationResult<ficant_domain::research::DataSnapshot> {
        let verified =
            VerifiedSnapshotReader::new(self.snapshots, self.blobs, self.integrity_events)
                .read(scope, binding.id.clone(), trace)
                .await?;
        let VerifiedSnapshotRead::Data { snapshot, .. } = verified else {
            return Err(integrity());
        };
        if snapshot.id() != &binding.id
            || snapshot.owner() != owner
            || snapshot.content_hash() != &binding.content_hash
            || snapshot.as_of() != valuation_at
            || snapshot.visible_at().instant() > knowledge_at.instant()
        {
            return Err(integrity());
        }
        Ok(snapshot)
    }
}

impl ResolvePortfolioAnalyticsAuthority<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn resolve_bonds(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
        units: &[PortfolioRatesUnitAuthority],
        currency_unit: &PortfolioRatesUnitAuthority,
        rate_unit: &PortfolioRatesUnitAuthority,
        evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<Vec<PortfolioBondRatesAuthorityResolution>> {
        let mut bound = BTreeMap::new();
        for binding in &candidate.bond_rates {
            if bound.insert(binding.position_id.clone(), binding).is_some() {
                return Err(state_conflict());
            }
        }
        let mut resolved = Vec::with_capacity(snapshot.positions().len());
        for position in snapshot.positions() {
            let value = self
                .read_exact_definition(scope, &context.owner, position.instrument_ref(), None)
                .await?;
            let DefinitionValue::Instrument(instrument) = value else {
                return Err(integrity());
            };
            let instrument_hash =
                definition_content_hash(&DefinitionValue::Instrument(instrument.clone()));
            push_evidence(
                evidence,
                object_evidence(
                    PortfolioAnalyticsEvidenceKind::Instrument,
                    position.instrument_ref(),
                    instrument_hash.clone(),
                ),
            );
            let Some(InstrumentSubtype::Bond(_)) = instrument.subtype() else {
                if bound.remove(position.id()).is_some() {
                    return Err(integrity());
                }
                resolved.push(PortfolioBondRatesAuthorityResolution::NonBond {
                    position_id: position.id().clone(),
                    instrument_ref: position.instrument_ref().clone(),
                });
                continue;
            };
            let Some(binding) = bound.remove(position.id()) else {
                resolved.push(PortfolioBondRatesAuthorityResolution::Missing {
                    position_id: position.id().clone(),
                    instrument_ref: position.instrument_ref().clone(),
                });
                continue;
            };
            if binding.instrument_ref != *position.instrument_ref() {
                return Err(integrity());
            }
            let calendar_ref = instrument.instrument().calendar().clone();
            resolved.push(
                self.resolve_bound_bond(
                    scope,
                    context,
                    position,
                    binding,
                    &calendar_ref,
                    candidate,
                    units,
                    currency_unit,
                    rate_unit,
                    instrument_hash,
                    evidence,
                )
                .await?,
            );
        }
        if !bound.is_empty() {
            return Err(integrity());
        }
        Ok(resolved)
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_bound_bond(
        &self,
        scope: &crate::ports::AccessScope,
        context: &NormalizedPortfolioContext,
        position: &Position,
        binding: &crate::ports::PortfolioBondRatesAuthorityCandidate,
        calendar_ref: &VersionRef,
        candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
        units: &[PortfolioRatesUnitAuthority],
        currency_unit: &PortfolioRatesUnitAuthority,
        rate_unit: &PortfolioRatesUnitAuthority,
        instrument_hash: ContentHash,
        evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<PortfolioBondRatesAuthorityResolution> {
        let calendar_value = self
            .read_exact_definition(scope, &context.owner, calendar_ref, None)
            .await?;
        let DefinitionValue::Calendar(calendar) = calendar_value else {
            return Err(integrity());
        };
        validate_effective(
            calendar.effective().from(),
            calendar.effective().to(),
            &context.valuation_at,
        )?;
        let calendar_hash = definition_content_hash(&DefinitionValue::Calendar(calendar.clone()));
        push_evidence(
            evidence,
            effective_object_evidence(
                PortfolioAnalyticsEvidenceKind::Calendar,
                calendar_ref,
                calendar_hash.clone(),
                calendar.effective().from(),
                calendar.effective().to(),
            ),
        );
        let valuation = self
            .authority
            .read_valuation_exact(scope, &context.owner, &binding.valuation)
            .await?
            .ok_or_else(not_found)?;
        validate_bound_valuation(position, context, binding, units, rate_unit, &valuation)?;
        push_evidence(
            evidence,
            PortfolioAnalyticsEvidenceBinding {
                kind: PortfolioAnalyticsEvidenceKind::Valuation,
                object_id: binding.valuation.valuation_id.clone(),
                version: Some(
                    Version::new(binding.valuation.source_revision)
                        .map_err(crate::map_domain_error)?,
                ),
                content_hash: binding.valuation.content_hash.clone(),
                observed_at: Some(valuation.valuation_at().clone()),
                visible_at: None,
                effective_from: None,
                effective_to: None,
            },
        );
        Ok(PortfolioBondRatesAuthorityResolution::Bond(Box::new(
            PortfolioBondRatesAuthority {
                position_id: position.id().clone(),
                instrument_ref: position.instrument_ref().clone(),
                bond: AnalyticsObjectRef::new(position.instrument_ref().clone(), instrument_hash),
                calendar: AnalyticsObjectRef::new(calendar_ref.clone(), calendar_hash),
                data_snapshot: candidate.data_snapshot.clone(),
                tax_rule_pack: candidate.tax_rule_pack.clone(),
                currency_unit: currency_unit.reference.clone(),
                rate_unit: rate_unit.reference.clone(),
                result_units: units.to_vec(),
                settlement_date: binding.settlement_date,
                calendar_requirement: binding.calendar_requirement,
                mode: binding.mode,
                input_value: binding.input_value,
                remaining_years: binding.remaining_years,
                valuation: binding.valuation.clone(),
            },
        )))
    }
}

fn validate_bound_valuation(
    position: &Position,
    context: &NormalizedPortfolioContext,
    binding: &crate::ports::PortfolioBondRatesAuthorityCandidate,
    units: &[PortfolioRatesUnitAuthority],
    rate_unit: &PortfolioRatesUnitAuthority,
    valuation: &ficant_domain::market::Valuation,
) -> ApplicationResult<()> {
    if valuation.instrument() != position.instrument_ref()
        || valuation.valuation_at() != &context.valuation_at
        || binding.valuation.value_index == binding.remaining_years_value_index
    {
        return Err(integrity());
    }
    let value_index = usize::try_from(binding.valuation.value_index).map_err(|_| validation())?;
    let remaining_years_index =
        usize::try_from(binding.remaining_years_value_index).map_err(|_| validation())?;
    validate_bound_valuation_roles(
        binding.mode,
        valuation.value_roles(),
        value_index,
        remaining_years_index,
    )?;
    let value = valuation.values().get(value_index).ok_or_else(integrity)?;
    let remaining_years = valuation
        .values()
        .get(remaining_years_index)
        .ok_or_else(integrity)?;
    let expected_unit = match binding.mode {
        ficant_domain::analytics::AnalyticsMode::PriceIn => {
            unit_for_role(units, PortfolioRatesUnitRole::PricePer100)?
        }
        ficant_domain::analytics::AnalyticsMode::YieldIn => rate_unit,
    };
    if value.unit() != &expected_unit.reference
        || decimal_to_fixed_exact(value)? != binding.input_value
        || remaining_years.unit() != &unit_for_role(units, PortfolioRatesUnitRole::Years)?.reference
        || decimal_to_fixed_exact(remaining_years)? != binding.remaining_years
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_bound_valuation_roles(
    mode: AnalyticsMode,
    roles: &[ValuationValueRole],
    value_index: usize,
    remaining_years_index: usize,
) -> ApplicationResult<()> {
    let expected_value_role = match mode {
        AnalyticsMode::PriceIn => ValuationValueRole::Price,
        AnalyticsMode::YieldIn => ValuationValueRole::Yield,
    };
    if roles.get(value_index) != Some(&expected_value_role)
        || roles.get(remaining_years_index) != Some(&ValuationValueRole::RemainingYears)
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_analytics_context(
    scope: &crate::ports::AccessScope,
    context: &NormalizedPortfolioContext,
    snapshot: &PositionSnapshot,
) -> ApplicationResult<()> {
    scope.authorize(&context.owner)?;
    if snapshot.owner() != &context.owner
        || snapshot.subject_ref() != &context.subject_ref
        || snapshot.visible_at().instant() > context.knowledge_at.instant()
        || snapshot.observed_at().instant() > context.valuation_at.instant()
        || context.knowledge_at.instant() < context.valuation_at.instant()
    {
        return Err(integrity());
    }
    Ok(())
}

fn analytics_authority_query(
    context: &NormalizedPortfolioContext,
    snapshot: &PositionSnapshot,
) -> ApplicationResult<PortfolioAnalyticsAuthorityQuery> {
    let binding = ficant_domain::portfolio::PortfolioSnapshotBinding::new(
        snapshot.id().clone(),
        snapshot.content_hash().clone(),
        snapshot.observed_at().clone(),
        snapshot.visible_at().clone(),
    )
    .map_err(crate::map_domain_error)?;
    PortfolioAnalyticsAuthorityQuery::new(
        context.owner.clone(),
        context.subject_ref.clone(),
        binding,
        context.valuation_at.clone(),
        context.knowledge_at.clone(),
    )
}

fn validate_analytics_candidate(
    context: &NormalizedPortfolioContext,
    snapshot: &PositionSnapshot,
    candidate: &crate::ports::PortfolioAnalyticsAuthorityCandidate,
) -> ApplicationResult<()> {
    if candidate.owner != context.owner
        || candidate.subject_ref != context.subject_ref
        || candidate.position_snapshot.id != *snapshot.id()
        || candidate.position_snapshot.content_hash != *snapshot.content_hash()
        || candidate.effective_from.instant() > context.valuation_at.instant()
        || candidate.effective_to.instant() <= context.valuation_at.instant()
        || candidate.visible_at.instant() > context.knowledge_at.instant()
        || candidate.canonical_content_hash() != candidate.content_hash
    {
        return Err(integrity());
    }
    Ok(())
}

const fn all_rates_unit_roles() -> [PortfolioRatesUnitRole; 9] {
    [
        PortfolioRatesUnitRole::CurrencyAmount,
        PortfolioRatesUnitRole::PricePer100,
        PortfolioRatesUnitRole::Rate,
        PortfolioRatesUnitRole::Years,
        PortfolioRatesUnitRole::YearsSquared,
        PortfolioRatesUnitRole::Dv01Per100,
        PortfolioRatesUnitRole::Dv01,
        PortfolioRatesUnitRole::Dimensionless,
        PortfolioRatesUnitRole::ContractCount,
    ]
}

fn unit_for_role(
    units: &[PortfolioRatesUnitAuthority],
    role: PortfolioRatesUnitRole,
) -> ApplicationResult<&PortfolioRatesUnitAuthority> {
    units
        .binary_search_by_key(&role, |unit| unit.role)
        .ok()
        .map(|index| &units[index])
        .ok_or_else(integrity)
}

fn initial_analytics_evidence(
    snapshot: &PositionSnapshot,
    units: &[PortfolioRatesUnitAuthority],
) -> Vec<PortfolioAnalyticsEvidenceBinding> {
    let mut evidence = vec![snapshot_evidence(
        PortfolioAnalyticsEvidenceKind::PositionSnapshot,
        snapshot.id(),
        snapshot.content_hash(),
        snapshot.observed_at(),
        snapshot.visible_at(),
    )];
    for unit in units {
        push_evidence(
            &mut evidence,
            PortfolioAnalyticsEvidenceBinding {
                kind: PortfolioAnalyticsEvidenceKind::Unit,
                object_id: unit.reference.unit_id().clone(),
                version: Some(unit.reference.version()),
                content_hash: unit.content_hash.clone(),
                observed_at: None,
                visible_at: None,
                effective_from: None,
                effective_to: None,
            },
        );
    }
    evidence
}

fn object_evidence(
    kind: PortfolioAnalyticsEvidenceKind,
    reference: &VersionRef,
    content_hash: ContentHash,
) -> PortfolioAnalyticsEvidenceBinding {
    PortfolioAnalyticsEvidenceBinding {
        kind,
        object_id: reference.id().clone(),
        version: Some(reference.version()),
        content_hash,
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn effective_object_evidence(
    kind: PortfolioAnalyticsEvidenceKind,
    reference: &VersionRef,
    content_hash: ContentHash,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
) -> PortfolioAnalyticsEvidenceBinding {
    PortfolioAnalyticsEvidenceBinding {
        effective_from: Some(effective_from.clone()),
        effective_to: Some(effective_to.clone()),
        ..object_evidence(kind, reference, content_hash)
    }
}

fn snapshot_evidence(
    kind: PortfolioAnalyticsEvidenceKind,
    id: &Ulid,
    content_hash: &ContentHash,
    observed_at: &MarketTime,
    visible_at: &MarketTime,
) -> PortfolioAnalyticsEvidenceBinding {
    PortfolioAnalyticsEvidenceBinding {
        kind,
        object_id: id.clone(),
        version: None,
        content_hash: content_hash.clone(),
        observed_at: Some(observed_at.clone()),
        visible_at: Some(visible_at.clone()),
        effective_from: None,
        effective_to: None,
    }
}

fn push_evidence(
    evidence: &mut Vec<PortfolioAnalyticsEvidenceBinding>,
    binding: PortfolioAnalyticsEvidenceBinding,
) {
    if !evidence.contains(&binding) {
        evidence.push(binding);
    }
}

fn authority_trace(content_hash: &ContentHash) -> ApplicationResult<SafeTraceContext> {
    SafeTraceContext::new(encode_hex(&content_hash.as_bytes()[..16]))
}

fn validate_effective(
    from: &MarketTime,
    to: &MarketTime,
    valuation_at: &MarketTime,
) -> ApplicationResult<()> {
    if from.instant() > valuation_at.instant() || to.instant() <= valuation_at.instant() {
        return Err(integrity());
    }
    Ok(())
}

fn validate_verified_rule(
    rule: &ficant_domain::market::MarketRulePack,
    valuation_at: &MarketTime,
) -> ApplicationResult<()> {
    if rule.verification_status() != VerificationStatus::Verified {
        return Err(integrity());
    }
    validate_effective(rule.effective().from(), rule.effective().to(), valuation_at)
}

fn decimal_to_fixed_exact(value: &DecimalValue) -> ApplicationResult<FixedDecimal> {
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| validation())?;
    let target = ficant_domain::analytics::DECIMAL_SCALE;
    let scaled = if value.scale() <= target {
        coefficient
            .checked_mul(checked_power_of_ten(target - value.scale())?)
            .ok_or_else(validation)?
    } else {
        let divisor = checked_power_of_ten(value.scale() - target)?;
        if coefficient % divisor != 0 {
            return Err(validation());
        }
        coefficient / divisor
    };
    Ok(FixedDecimal::from_scaled(scaled))
}

fn checked_power_of_ten(exponent: u32) -> ApplicationResult<i128> {
    (0..exponent).try_fold(1_i128, |value, _| {
        value.checked_mul(10).ok_or_else(validation)
    })
}

trait CatalogEvidenceRecord: ContentAddressed {
    fn catalog_reference(&self) -> &VersionRef;
    fn catalog_owner(&self) -> &OwnerRef;
    fn catalog_subject_ref(&self) -> Option<&VersionRef>;
    fn catalog_effective_from(&self) -> &MarketTime;
    fn catalog_effective_to(&self) -> &MarketTime;
}

macro_rules! impl_subject_catalog_evidence_record {
    ($type:ty) => {
        impl CatalogEvidenceRecord for $type {
            fn catalog_reference(&self) -> &VersionRef {
                self.reference()
            }

            fn catalog_owner(&self) -> &OwnerRef {
                self.owner()
            }

            fn catalog_subject_ref(&self) -> Option<&VersionRef> {
                Some(self.subject_ref())
            }

            fn catalog_effective_from(&self) -> &MarketTime {
                self.effective_from()
            }

            fn catalog_effective_to(&self) -> &MarketTime {
                self.effective_to()
            }
        }
    };
}

impl_subject_catalog_evidence_record!(Book);
impl_subject_catalog_evidence_record!(PortfolioGroup);
impl_subject_catalog_evidence_record!(Portfolio);
impl_subject_catalog_evidence_record!(Benchmark);

impl CatalogEvidenceRecord for PortfolioMetricConvention {
    fn catalog_reference(&self) -> &VersionRef {
        self.reference()
    }

    fn catalog_owner(&self) -> &OwnerRef {
        self.owner()
    }

    fn catalog_subject_ref(&self) -> Option<&VersionRef> {
        None
    }

    fn catalog_effective_from(&self) -> &MarketTime {
        self.effective_from()
    }

    fn catalog_effective_to(&self) -> &MarketTime {
        self.effective_to()
    }
}

fn catalog_record_evidence<T: CatalogEvidenceRecord>(
    role: PortfolioCatalogEvidenceRole,
    record: &VisibleCatalogRecord<T>,
    temporal: &PortfolioCatalogTemporalScope,
) -> ApplicationResult<PortfolioCatalogEvidenceBinding> {
    let value = record.value();
    if value.catalog_owner() != temporal.owner()
        || value
            .catalog_subject_ref()
            .is_some_and(|subject| subject != temporal.subject_ref())
        || record.visible_at().instant() > temporal.knowledge_at().instant()
        || value.catalog_effective_from().instant() > temporal.as_of().instant()
        || value.catalog_effective_to().instant() <= temporal.as_of().instant()
    {
        return Err(integrity());
    }
    PortfolioCatalogEvidenceBinding::new(
        role,
        value.catalog_reference().clone(),
        value.content_hash().clone(),
        record.visible_at().clone(),
        value.catalog_effective_from().clone(),
        value.catalog_effective_to().clone(),
    )
    .map_err(|_| integrity())
}

fn evidence_matches_record<T: CatalogEvidenceRecord>(
    evidence: &PortfolioCatalogEvidenceBinding,
    record: &VisibleCatalogRecord<T>,
) -> bool {
    evidence.reference() == record.value().catalog_reference()
        && evidence.content_hash() == record.value().content_hash()
        && evidence.visible_at() == record.visible_at()
        && evidence.effective_from() == record.value().catalog_effective_from()
        && evidence.effective_to() == record.value().catalog_effective_to()
}

fn evidence_matches_lineage(
    evidence: &PortfolioCatalogEvidenceBinding,
    lineage: &LineageRef,
) -> bool {
    lineage.version() == Some(evidence.reference().version())
        && lineage.object_id() == evidence.reference().id()
        && lineage.content_hash() == Some(evidence.content_hash())
}

const fn selected_scope_lineage(selected: &ExactPortfolioScopeKind) -> &LineageRef {
    match selected {
        ExactPortfolioScopeKind::Book(reference)
        | ExactPortfolioScopeKind::Group(reference)
        | ExactPortfolioScopeKind::Portfolio(reference) => reference,
    }
}

fn validate_catalog_evidence_set(
    context: &NormalizedPortfolioContext,
    portfolios: &[VisibleCatalogRecord<Portfolio>],
    benchmark: &VisibleCatalogRecord<Benchmark>,
    convention: &VisibleCatalogRecord<PortfolioMetricConvention>,
    evidence: &[PortfolioCatalogEvidenceBinding],
) -> ApplicationResult<()> {
    if portfolios.is_empty() || evidence.len() != portfolios.len() + 3 {
        return Err(integrity());
    }
    let mut keys = BTreeSet::new();
    if evidence
        .iter()
        .any(|binding| !keys.insert((binding.role(), binding.reference().clone())))
    {
        return Err(integrity());
    }

    let selected = evidence
        .iter()
        .filter(|binding| {
            matches!(
                binding.role(),
                PortfolioCatalogEvidenceRole::SelectedBook
                    | PortfolioCatalogEvidenceRole::SelectedGroup
                    | PortfolioCatalogEvidenceRole::SelectedPortfolio
            )
        })
        .collect::<Vec<_>>();
    let [selected] = selected.as_slice() else {
        return Err(integrity());
    };
    let selected_role_matches = matches!(
        (context.scope.selected(), selected.role()),
        (
            ExactPortfolioScopeKind::Book(_),
            PortfolioCatalogEvidenceRole::SelectedBook
        ) | (
            ExactPortfolioScopeKind::Group(_),
            PortfolioCatalogEvidenceRole::SelectedGroup
        ) | (
            ExactPortfolioScopeKind::Portfolio(_),
            PortfolioCatalogEvidenceRole::SelectedPortfolio
        )
    );
    if !selected_role_matches
        || !evidence_matches_lineage(selected, selected_scope_lineage(context.scope.selected()))
    {
        return Err(integrity());
    }

    let members = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::MemberPortfolio)
        .collect::<Vec<_>>();
    if members.len() != portfolios.len()
        || members.len() != context.scope.member_portfolios().len()
        || portfolios.iter().any(|record| {
            !members
                .iter()
                .any(|binding| evidence_matches_record(binding, record))
        })
        || context.scope.member_portfolios().iter().any(|lineage| {
            !members
                .iter()
                .any(|binding| evidence_matches_lineage(binding, lineage))
        })
    {
        return Err(integrity());
    }

    let benchmarks = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::Benchmark)
        .collect::<Vec<_>>();
    let conventions = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::MetricConvention)
        .collect::<Vec<_>>();
    let ([benchmark_evidence], [convention_evidence]) =
        (benchmarks.as_slice(), conventions.as_slice())
    else {
        return Err(integrity());
    };
    if !evidence_matches_record(benchmark_evidence, benchmark)
        || benchmark_evidence.reference() != context.benchmark.reference()
        || benchmark_evidence.content_hash() != context.benchmark.content_hash()
        || !evidence_matches_record(convention_evidence, convention)
        || convention_evidence.reference() != context.metric_convention.reference()
        || convention_evidence.content_hash() != context.metric_convention.content_hash()
    {
        return Err(integrity());
    }
    Ok(())
}

struct CatalogHierarchy {
    books: BTreeMap<String, VisibleCatalogRecord<Book>>,
    groups: BTreeMap<String, VisibleCatalogRecord<PortfolioGroup>>,
    portfolios: Vec<VisibleCatalogRecord<Portfolio>>,
    benchmarks: BTreeMap<String, VisibleCatalogRecord<Benchmark>>,
    metric_conventions: BTreeMap<String, VisibleCatalogRecord<PortfolioMetricConvention>>,
}

impl CatalogHierarchy {
    fn new(snapshot: &PortfolioCatalogSnapshot) -> ApplicationResult<Self> {
        Ok(Self {
            books: exact_map(snapshot.books())?,
            groups: exact_map(snapshot.groups())?,
            portfolios: latest_versions(snapshot.portfolios())?,
            benchmarks: exact_map(snapshot.benchmarks())?,
            metric_conventions: exact_map(snapshot.metric_conventions())?,
        })
    }

    fn selected_evidence(
        &self,
        selected: &ExactPortfolioScopeKind,
        temporal: &PortfolioCatalogTemporalScope,
    ) -> ApplicationResult<PortfolioCatalogEvidenceBinding> {
        let evidence = match selected {
            ExactPortfolioScopeKind::Book(reference) => catalog_record_evidence(
                PortfolioCatalogEvidenceRole::SelectedBook,
                self.book(reference)?,
                temporal,
            )?,
            ExactPortfolioScopeKind::Group(reference) => catalog_record_evidence(
                PortfolioCatalogEvidenceRole::SelectedGroup,
                self.group(reference)?,
                temporal,
            )?,
            ExactPortfolioScopeKind::Portfolio(reference) => {
                let record = self
                    .portfolios
                    .iter()
                    .find(|record| {
                        record.value().reference().id() == reference.object_id()
                            && Some(record.value().reference().version()) == reference.version()
                            && Some(record.value().content_hash()) == reference.content_hash()
                    })
                    .ok_or_else(integrity)?;
                catalog_record_evidence(
                    PortfolioCatalogEvidenceRole::SelectedPortfolio,
                    record,
                    temporal,
                )?
            }
        };
        if !evidence_matches_lineage(&evidence, selected_scope_lineage(selected)) {
            return Err(integrity());
        }
        Ok(evidence)
    }

    fn resolve_members(
        &self,
        selector: &PortfolioScopeSelector,
        look_through: PortfolioLookThroughMode,
    ) -> ApplicationResult<(
        ExactPortfolioScopeKind,
        Vec<VisibleCatalogRecord<Portfolio>>,
    )> {
        match selector {
            PortfolioScopeSelector::Portfolio(id) => {
                let portfolio = latest_by_id(&self.portfolios, id.as_str())?;
                Ok((
                    ExactPortfolioScopeKind::Portfolio(exact_lineage(portfolio.value())?),
                    vec![portfolio.clone()],
                ))
            }
            PortfolioScopeSelector::Book(id) => {
                let book = latest_map_by_id(&self.books, id.as_str())?;
                let selected = ExactPortfolioScopeKind::Book(exact_lineage(book.value())?);
                let mut members = self
                    .portfolios
                    .iter()
                    .filter(|record| record.value().book().object_id() == id)
                    .filter(|record| {
                        if look_through == PortfolioLookThroughMode::None {
                            self.group(record.value().group())
                                .is_ok_and(|group| group.value().parent_group().is_none())
                        } else {
                            true
                        }
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                stable_member_sort(&mut members);
                Ok((selected, members))
            }
            PortfolioScopeSelector::Group(id) => {
                let group = latest_map_by_id(&self.groups, id.as_str())?;
                let selected = ExactPortfolioScopeKind::Group(exact_lineage(group.value())?);
                let descendant_ids = if look_through == PortfolioLookThroughMode::None {
                    BTreeSet::from([id.clone()])
                } else {
                    self.descendant_group_ids(id)?
                };
                let mut members = self
                    .portfolios
                    .iter()
                    .filter(|record| descendant_ids.contains(record.value().group().object_id()))
                    .cloned()
                    .collect::<Vec<_>>();
                stable_member_sort(&mut members);
                Ok((selected, members))
            }
        }
    }

    fn descendant_group_ids(
        &self,
        root: &ficant_domain::primitives::Ulid,
    ) -> ApplicationResult<BTreeSet<ficant_domain::primitives::Ulid>> {
        let mut result = BTreeSet::from([root.clone()]);
        loop {
            let before = result.len();
            for record in self.groups.values() {
                if record
                    .value()
                    .parent_group()
                    .is_some_and(|parent| result.contains(parent.object_id()))
                {
                    result.insert(record.value().reference().id().clone());
                }
            }
            if result.len() == before {
                break;
            }
            if result.len() > self.groups.len() {
                return Err(lineage());
            }
        }
        Ok(result)
    }

    fn single_convention(
        &self,
        members: &[VisibleCatalogRecord<Portfolio>],
    ) -> ApplicationResult<&VisibleCatalogRecord<PortfolioMetricConvention>> {
        let first = members.first().ok_or_else(not_found)?;
        if members
            .iter()
            .any(|member| member.value().metric_convention() != first.value().metric_convention())
        {
            return Err(state_conflict());
        }
        self.metric_conventions
            .get(&format!(
                "{}:{}:{}",
                first.value().metric_convention().reference().id(),
                first
                    .value()
                    .metric_convention()
                    .reference()
                    .version()
                    .get(),
                encode_hex(first.value().metric_convention().content_hash().as_bytes())
            ))
            .ok_or_else(integrity)
    }

    fn benchmark_by_id(
        &self,
        id: &ficant_domain::primitives::Ulid,
    ) -> ApplicationResult<&VisibleCatalogRecord<Benchmark>> {
        latest_map_by_id(&self.benchmarks, id.as_str())
    }

    fn filtered_portfolios(
        &self,
        filter: &PortfolioCatalogFilter,
    ) -> ApplicationResult<Vec<PortfolioCatalogEntry>> {
        let mut entries = Vec::new();
        for record in &self.portfolios {
            let portfolio = record.value();
            if !filter.accepts_status(portfolio.status()) {
                continue;
            }
            let group = self.group(portfolio.group())?;
            let book = self.book(portfolio.book())?;
            if group.value().book() != portfolio.book() {
                return Err(lineage());
            }
            let group_path = self.group_path(group.value())?;
            if !matches_search(filter, book.value(), group.value(), portfolio, &group_path) {
                continue;
            }
            entries.push(PortfolioCatalogEntry::new(
                record.clone(),
                PortfolioCatalogSortKey::new(
                    book.value().code().to_owned(),
                    group_path,
                    portfolio.code().to_owned(),
                    portfolio.version(),
                )?,
            ));
        }
        Ok(entries)
    }

    fn page_hierarchy(
        &self,
        entries: &[PortfolioCatalogEntry],
    ) -> ApplicationResult<CatalogPageHierarchy> {
        let mut books = BTreeMap::new();
        let mut groups = BTreeMap::new();
        for entry in entries {
            let portfolio = entry.record().value();
            let book = self.book(portfolio.book())?;
            books.insert(exact_lineage_key(portfolio.book())?, book.clone());
            let mut current = Some(self.group(portfolio.group())?);
            let mut visited = BTreeSet::new();
            while let Some(group) = current {
                let key = exact_version_key(group.value());
                if !visited.insert(key.clone()) {
                    return Err(lineage());
                }
                groups.insert(key, group.clone());
                current = group
                    .value()
                    .parent_group()
                    .map(|parent| self.group(parent))
                    .transpose()?;
            }
        }
        let mut books = books.into_values().collect::<Vec<_>>();
        books.sort_by(|left, right| {
            left.value()
                .code()
                .cmp(right.value().code())
                .then_with(|| left.value().version().cmp(&right.value().version()))
        });
        let mut groups = groups.into_values().collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            self.group_path(left.value())
                .unwrap_or_default()
                .cmp(&self.group_path(right.value()).unwrap_or_default())
                .then_with(|| left.value().version().cmp(&right.value().version()))
        });
        Ok((books, groups))
    }

    fn book(&self, reference: &LineageRef) -> ApplicationResult<&VisibleCatalogRecord<Book>> {
        self.books
            .get(&exact_lineage_key(reference)?)
            .ok_or_else(lineage)
    }

    fn group(
        &self,
        reference: &LineageRef,
    ) -> ApplicationResult<&VisibleCatalogRecord<PortfolioGroup>> {
        self.groups
            .get(&exact_lineage_key(reference)?)
            .ok_or_else(lineage)
    }

    fn group_path(&self, group: &PortfolioGroup) -> ApplicationResult<String> {
        let mut codes = Vec::new();
        let mut current = Some(group);
        let mut visited = BTreeSet::new();
        while let Some(value) = current {
            let key = exact_version_key(value);
            if !visited.insert(key) {
                return Err(lineage());
            }
            codes.push(value.code().to_owned());
            current = value
                .parent_group()
                .map(|parent| self.group(parent).map(VisibleCatalogRecord::value))
                .transpose()?;
        }
        codes.reverse();
        Ok(codes.join("/"))
    }
}

fn authorize(
    principal: &AuthorizedPrincipal,
    owner: &ficant_domain::primitives::OwnerRef,
) -> ApplicationResult<()> {
    authorize_reader(principal)?;
    principal.access_scope().authorize(owner)
}

fn authorize_reader(principal: &AuthorizedPrincipal) -> ApplicationResult<()> {
    principal.require_role(PlatformRole::Researcher)?;
    if !principal.has_scope(PORTFOLIO_READ_SCOPE) {
        return Err(forbidden());
    }
    Ok(())
}

fn validate_snapshot(
    principal: &AuthorizedPrincipal,
    filter: &PortfolioCatalogFilter,
    snapshot: &PortfolioCatalogSnapshot,
) -> ApplicationResult<()> {
    for record in snapshot.books() {
        validate_record(
            principal,
            filter,
            record.value().owner(),
            record.value().subject_ref(),
            record.value().effective_from(),
            record.value().effective_to(),
            record.visible_at(),
        )?;
    }
    for record in snapshot.groups() {
        validate_record(
            principal,
            filter,
            record.value().owner(),
            record.value().subject_ref(),
            record.value().effective_from(),
            record.value().effective_to(),
            record.visible_at(),
        )?;
    }
    for record in snapshot.portfolios() {
        validate_record(
            principal,
            filter,
            record.value().owner(),
            record.value().subject_ref(),
            record.value().effective_from(),
            record.value().effective_to(),
            record.visible_at(),
        )?;
    }
    for record in snapshot.benchmarks() {
        validate_record(
            principal,
            filter,
            record.value().owner(),
            record.value().subject_ref(),
            record.value().effective_from(),
            record.value().effective_to(),
            record.visible_at(),
        )?;
    }
    for record in snapshot.metric_conventions() {
        if record.value().owner() != filter.temporal().owner()
            || record.visible_at().instant() > filter.temporal().knowledge_at().instant()
            || record.value().effective_from().instant() > filter.temporal().as_of().instant()
            || record.value().effective_to().instant() <= filter.temporal().as_of().instant()
        {
            return Err(integrity());
        }
        principal.access_scope().authorize(record.value().owner())?;
    }
    Ok(())
}

fn validate_record(
    principal: &AuthorizedPrincipal,
    filter: &PortfolioCatalogFilter,
    owner: &ficant_domain::primitives::OwnerRef,
    subject_ref: &ficant_domain::primitives::VersionRef,
    effective_from: &ficant_domain::primitives::MarketTime,
    effective_to: &ficant_domain::primitives::MarketTime,
    visible_at: &ficant_domain::primitives::MarketTime,
) -> ApplicationResult<()> {
    if owner != filter.temporal().owner()
        || subject_ref != filter.temporal().subject_ref()
        || visible_at.instant() > filter.temporal().knowledge_at().instant()
        || effective_from.instant() > filter.temporal().as_of().instant()
        || effective_to.instant() <= filter.temporal().as_of().instant()
    {
        return Err(integrity());
    }
    principal.access_scope().authorize(owner)
}

fn exact_map<T>(
    records: &[VisibleCatalogRecord<T>],
) -> ApplicationResult<BTreeMap<String, VisibleCatalogRecord<T>>>
where
    T: Clone + ContentAddressed + VersionedDefinition,
{
    let mut result = BTreeMap::new();
    for record in records {
        let key = exact_version_key(record.value());
        if result.insert(key, record.clone()).is_some() {
            return Err(state_conflict());
        }
    }
    Ok(result)
}

fn latest_versions<T>(
    records: &[VisibleCatalogRecord<T>],
) -> ApplicationResult<Vec<VisibleCatalogRecord<T>>>
where
    T: Clone + ContentAddressed + VersionedDefinition,
{
    let mut latest = BTreeMap::<String, VisibleCatalogRecord<T>>::new();
    for record in records {
        match latest.get(record.value().identity()) {
            Some(previous) if previous.value().version() == record.value().version() => {
                return Err(state_conflict());
            }
            Some(previous) if previous.value().version() > record.value().version() => {}
            _ => {
                latest.insert(record.value().identity().to_owned(), record.clone());
            }
        }
    }
    Ok(latest.into_values().collect())
}

fn latest_by_id<'a, T>(
    records: &'a [VisibleCatalogRecord<T>],
    id: &str,
) -> ApplicationResult<&'a VisibleCatalogRecord<T>>
where
    T: ContentAddressed + VersionedDefinition,
{
    records
        .iter()
        .filter(|record| record.value().identity() == id)
        .max_by_key(|record| record.value().version())
        .ok_or_else(not_found)
}

fn latest_map_by_id<'a, T>(
    records: &'a BTreeMap<String, VisibleCatalogRecord<T>>,
    id: &str,
) -> ApplicationResult<&'a VisibleCatalogRecord<T>>
where
    T: ContentAddressed + VersionedDefinition,
{
    records
        .values()
        .filter(|record| record.value().identity() == id)
        .max_by_key(|record| record.value().version())
        .ok_or_else(not_found)
}

fn stable_member_sort(records: &mut [VisibleCatalogRecord<Portfolio>]) {
    records.sort_by(|left, right| {
        left.value()
            .reference()
            .id()
            .cmp(right.value().reference().id())
            .then_with(|| {
                left.value()
                    .reference()
                    .version()
                    .cmp(&right.value().reference().version())
            })
    });
}

fn exact_lineage<T>(value: &T) -> ApplicationResult<LineageRef>
where
    T: ContentAddressed + VersionedDefinition,
{
    LineageRef::new(
        ficant_domain::primitives::Ulid::new(value.identity().to_owned())
            .map_err(crate::map_domain_error)?,
        Some(
            ficant_domain::primitives::Version::new(value.version())
                .map_err(crate::map_domain_error)?,
        ),
        Some(value.content_hash().clone()),
    )
    .map_err(crate::map_domain_error)
}

fn exact_read(
    temporal: &PortfolioCatalogTemporalScope,
    reference: &LineageRef,
) -> ApplicationResult<ExactCatalogRead> {
    let version = reference.version().ok_or_else(lineage)?;
    let hash = reference.content_hash().ok_or_else(lineage)?;
    Ok(ExactCatalogRead::new(
        temporal.clone(),
        VersionRef::new(reference.object_id().clone(), version),
        hash.clone(),
    ))
}

async fn resolve_selected_scope_evidence(
    repository: &dyn PortfolioCatalogRepository,
    scope: &crate::ports::AccessScope,
    temporal: &PortfolioCatalogTemporalScope,
    selected: &ExactPortfolioScopeKind,
) -> ApplicationResult<PortfolioCatalogEvidenceBinding> {
    let evidence = match selected {
        ExactPortfolioScopeKind::Book(reference) => catalog_record_evidence(
            PortfolioCatalogEvidenceRole::SelectedBook,
            &repository
                .read_book_exact(scope, &exact_read(temporal, reference)?)
                .await?
                .ok_or_else(integrity)?,
            temporal,
        )?,
        ExactPortfolioScopeKind::Group(reference) => catalog_record_evidence(
            PortfolioCatalogEvidenceRole::SelectedGroup,
            &repository
                .read_group_exact(scope, &exact_read(temporal, reference)?)
                .await?
                .ok_or_else(integrity)?,
            temporal,
        )?,
        ExactPortfolioScopeKind::Portfolio(reference) => catalog_record_evidence(
            PortfolioCatalogEvidenceRole::SelectedPortfolio,
            &repository
                .read_portfolio_exact(scope, &exact_read(temporal, reference)?)
                .await?
                .ok_or_else(integrity)?,
            temporal,
        )?,
    };
    if !evidence_matches_lineage(&evidence, selected_scope_lineage(selected)) {
        return Err(integrity());
    }
    Ok(evidence)
}

async fn read_exact_member_portfolios(
    repository: &dyn PortfolioCatalogRepository,
    scope: &crate::ports::AccessScope,
    temporal: &PortfolioCatalogTemporalScope,
    context: &NormalizedPortfolioContext,
) -> ApplicationResult<Vec<VisibleCatalogRecord<Portfolio>>> {
    let mut portfolios = Vec::new();
    let mut seen = BTreeSet::new();
    for member in context.scope.member_portfolios() {
        let record = repository
            .read_portfolio_exact(scope, &exact_read(temporal, member)?)
            .await?
            .ok_or_else(integrity)?;
        if !seen.insert(record.value().reference().id().clone())
            || record.value().metric_convention().reference()
                != context.metric_convention.reference()
            || record.value().metric_convention().content_hash()
                != context.metric_convention.content_hash()
        {
            return Err(integrity());
        }
        portfolios.push(record);
    }
    portfolios.sort_by(|left, right| {
        left.value()
            .reference()
            .id()
            .cmp(right.value().reference().id())
            .then_with(|| {
                left.value()
                    .reference()
                    .version()
                    .cmp(&right.value().reference().version())
            })
    });
    Ok(portfolios)
}

fn resolved_catalog_evidence(
    selected: PortfolioCatalogEvidenceBinding,
    temporal: &PortfolioCatalogTemporalScope,
    context: &NormalizedPortfolioContext,
    portfolios: &[VisibleCatalogRecord<Portfolio>],
    benchmark: &VisibleCatalogRecord<Benchmark>,
    convention: &VisibleCatalogRecord<PortfolioMetricConvention>,
) -> ApplicationResult<Vec<PortfolioCatalogEvidenceBinding>> {
    let mut evidence = vec![selected];
    for portfolio in portfolios {
        evidence.push(catalog_record_evidence(
            PortfolioCatalogEvidenceRole::MemberPortfolio,
            portfolio,
            temporal,
        )?);
    }
    evidence.push(catalog_record_evidence(
        PortfolioCatalogEvidenceRole::Benchmark,
        benchmark,
        temporal,
    )?);
    evidence.push(catalog_record_evidence(
        PortfolioCatalogEvidenceRole::MetricConvention,
        convention,
        temporal,
    )?);
    evidence.sort_by(|left, right| {
        left.role()
            .cmp(&right.role())
            .then_with(|| left.reference().id().cmp(right.reference().id()))
            .then_with(|| left.reference().version().cmp(&right.reference().version()))
    });
    validate_catalog_evidence_set(context, portfolios, benchmark, convention, &evidence)?;
    Ok(evidence)
}

fn period_window(
    valuation_at: &MarketTime,
    preset: PortfolioPeriodPreset,
) -> ApplicationResult<(MarketTime, MarketTime)> {
    let timezone = valuation_at
        .market_timezone()
        .parse::<Tz>()
        .map_err(|_| validation())?;
    let end = valuation_at.clone();
    let start_instant = match preset {
        PortfolioPeriodPreset::OneDay => valuation_at.instant() - Duration::days(1),
        PortfolioPeriodPreset::SevenDays => valuation_at.instant() - Duration::days(7),
        PortfolioPeriodPreset::ThirtyDays => valuation_at.instant() - Duration::days(30),
        PortfolioPeriodPreset::OneYear => valuation_at.instant() - Duration::days(365),
        PortfolioPeriodPreset::YearToDate => timezone
            .with_ymd_and_hms(valuation_at.local_trading_date().year(), 1, 1, 0, 0, 0)
            .single()
            .ok_or_else(validation)?
            .with_timezone(&Utc),
    };
    let local_date = start_instant.with_timezone(&timezone).date_naive();
    let start = MarketTime::new(
        start_instant,
        valuation_at.market_timezone().to_owned(),
        local_date,
    )
    .map_err(crate::map_domain_error)?;
    Ok((start, end))
}

fn exact_version_key<T>(value: &T) -> String
where
    T: ContentAddressed + VersionedDefinition,
{
    format!(
        "{}:{}:{}",
        value.identity(),
        value.version(),
        encode_hex(value.content_hash().as_bytes())
    )
}

fn exact_lineage_key(reference: &LineageRef) -> ApplicationResult<String> {
    let version = reference.version().ok_or_else(lineage)?.get();
    let hash = reference.content_hash().ok_or_else(lineage)?;
    Ok(format!(
        "{}:{}:{}",
        reference.object_id(),
        version,
        encode_hex(hash.as_bytes())
    ))
}

fn matches_search(
    filter: &PortfolioCatalogFilter,
    book: &Book,
    group: &PortfolioGroup,
    portfolio: &Portfolio,
    group_path: &str,
) -> bool {
    let Some(search) = filter.normalized_search() else {
        return true;
    };
    [
        book.code(),
        book.display_name(),
        group.code(),
        group.display_name(),
        portfolio.code(),
        portfolio.display_name(),
        group_path,
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(search))
}

fn encode_hex(value: &[u8]) -> String {
    value.iter().fold(
        String::with_capacity(value.len() * 2),
        |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        },
    )
}

fn decode_utf8_hex(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = nibble(pair[0])?;
            let low = nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

const fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn state_conflict() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_valuation_roles_are_exact_for_mode_and_remaining_years() {
        assert!(
            validate_bound_valuation_roles(
                AnalyticsMode::YieldIn,
                &[
                    ValuationValueRole::Yield,
                    ValuationValueRole::RemainingYears
                ],
                0,
                1,
            )
            .is_ok()
        );
        assert!(
            validate_bound_valuation_roles(
                AnalyticsMode::PriceIn,
                &[
                    ValuationValueRole::Price,
                    ValuationValueRole::RemainingYears
                ],
                0,
                1,
            )
            .is_ok()
        );
        for (mode, roles) in [
            (
                AnalyticsMode::YieldIn,
                [
                    ValuationValueRole::Price,
                    ValuationValueRole::RemainingYears,
                ],
            ),
            (
                AnalyticsMode::PriceIn,
                [
                    ValuationValueRole::Yield,
                    ValuationValueRole::RemainingYears,
                ],
            ),
            (
                AnalyticsMode::YieldIn,
                [ValuationValueRole::Yield, ValuationValueRole::Price],
            ),
        ] {
            assert!(validate_bound_valuation_roles(mode, &roles, 0, 1).is_err());
        }
        assert!(
            validate_bound_valuation_roles(
                AnalyticsMode::YieldIn,
                &[
                    ValuationValueRole::Yield,
                    ValuationValueRole::RemainingYears
                ],
                2,
                1,
            )
            .is_err()
        );
    }
}
