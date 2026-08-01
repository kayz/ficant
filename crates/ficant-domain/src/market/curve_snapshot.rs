use crate::market::require_text;
use crate::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactInputKind {
    ExternalFixture,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurveSnapshot {
    curve_snapshot_id: Ulid,
    owner: OwnerRef,
    as_of: MarketTime,
    currency: UnitRef,
    curve_kind: String,
    calendar: VersionRef,
    rule_pack: VersionRef,
    point_schema: String,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
    input_kind: ArtifactInputKind,
    visible_at: Option<MarketTime>,
    curve_family_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurveSnapshotInput {
    pub curve_snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub as_of: MarketTime,
    pub currency: UnitRef,
    pub curve_kind: String,
    pub calendar: VersionRef,
    pub rule_pack: VersionRef,
    pub point_schema: String,
    pub content_hash: ContentHash,
    pub lineage: Vec<LineageRef>,
    pub input_kind: ArtifactInputKind,
}

impl CurveSnapshot {
    pub fn new(input: CurveSnapshotInput) -> DomainResult<Self> {
        let CurveSnapshotInput {
            curve_snapshot_id,
            owner,
            as_of,
            currency,
            curve_kind,
            calendar,
            rule_pack,
            point_schema,
            content_hash,
            lineage,
            input_kind,
        } = input;
        require_text(&curve_kind)?;
        require_text(&point_schema)?;
        if lineage.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(Self {
            curve_snapshot_id,
            owner,
            as_of,
            currency,
            curve_kind,
            calendar,
            rule_pack,
            point_schema,
            content_hash,
            lineage,
            input_kind,
            visible_at: None,
            curve_family_id: None,
        })
    }

    /// Enriches a legacy-readable fixture with the explicit R4d-a knowledge boundary.
    pub fn with_knowledge_time(
        mut self,
        visible_at: MarketTime,
        curve_family_id: impl Into<String>,
    ) -> DomainResult<Self> {
        let curve_family_id = curve_family_id.into();
        require_text(&curve_family_id)?;
        if self.visible_at.is_some() || self.curve_family_id.is_some() {
            return Err(DomainErrorCode::InvalidStateTransition);
        }
        if visible_at.instant() < self.as_of.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        self.visible_at = Some(visible_at);
        self.curve_family_id = Some(curve_family_id);
        Ok(self)
    }

    pub fn id(&self) -> &Ulid {
        &self.curve_snapshot_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    pub fn currency(&self) -> &UnitRef {
        &self.currency
    }

    pub fn curve_kind(&self) -> &str {
        &self.curve_kind
    }

    pub fn calendar(&self) -> &VersionRef {
        &self.calendar
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub fn point_schema(&self) -> &str {
        &self.point_schema
    }

    pub fn input_kind(&self) -> ArtifactInputKind {
        self.input_kind
    }

    pub fn visible_at(&self) -> Option<&MarketTime> {
        self.visible_at.as_ref()
    }

    pub fn curve_family_id(&self) -> Option<&str> {
        self.curve_family_id.as_deref()
    }
}

impl ContentAddressed for CurveSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for CurveSnapshot {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
