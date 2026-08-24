import { create } from "@bufbuild/protobuf";
import {
  FormalInputKind,
} from "../../packages/contracts-generated/src/ficant/core/v1/evidence_pb";
import {
  AppDescriptorSchema,
  PlatformService,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { SessionSchema } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";
import {
  DecimalValueSchema,
  MarketTimeSchema,
  OwnerRefSchema,
  Sha256Schema,
  UlidSchema,
  UnitRefSchema,
} from "../../packages/contracts-generated/src/ficant/core/v1/common_pb";
import {
  RegisterSubjectRequestSchema,
  SubjectSchema,
} from "../../packages/contracts-generated/src/ficant/core/v1/subject_pb";
import {
  RegisterSubjectStateRequestSchema,
  SubjectStateSnapshotSchema,
} from "../../packages/contracts-generated/src/ficant/core/v1/subject_state_pb";
import {
  ChangeJustificationSchema,
  FoundationChangeRecordSchema,
  FoundationChangeService,
  PlatformRole,
} from "../../packages/contracts-generated/src/ficant/core/v1/governance_pb";
import {
  CgbFuturesDeliveryRulePackSchema,
  CgbFuturesProductRuleSchema,
} from "../../packages/contracts-generated/src/ficant/market/v1/cgb_futures_rule_pb";
import {
  FundingRulePackSchema,
  FundingTierRateSchema,
} from "../../packages/contracts-generated/src/ficant/market/v1/funding_rule_pb";
import {
  BondTaxAttributesSchema,
  CompleteInstrumentDefinitionSchema,
  IncomeTaxStatus,
  MarketDefinitionSchema,
  MarketDefinitionService,
  ValueAddedTaxStatus,
} from "../../packages/contracts-generated/src/ficant/market/v1/definition_pb";
import {
  DataSourceAuthorizationSchema,
  DataSourceRegistryService,
  InstrumentMappingSchema,
} from "../../packages/contracts-generated/src/ficant/market/v1/data_source_pb";
import {
  CashflowSchema,
  CashflowType,
  CurvePointSetSchema,
  CurveSnapshotInputSchema,
  GetCurveSnapshotRequestSchema,
  MarketFactSchema,
  MarketFactService,
  PublishCurveSnapshotRequestSchema,
  QueryInstrumentFactsRequestSchema,
  ValuationSchema,
  ValuationValueRole,
} from "../../packages/contracts-generated/src/ficant/market/v1/fact_pb";
import { MarketRulePackSchema } from "../../packages/contracts-generated/src/ficant/market/v1/rule_pb";
import {
  BondCouponTaxTreatmentRuleSchema,
  BondCouponTaxRuleSchema,
  CouponTaxClaimScope,
  GrossCouponTaxBasis,
  SubjectCouponTaxTreatmentSchema,
  SubjectCouponTaxRateSchema,
  TaxRoundingMode,
  TaxRulePackSchema,
  TaxRulePackV2Schema,
} from "../../packages/contracts-generated/src/ficant/market/v1/tax_rule_pb";
import {
  AlgorithmBindingSchema,
  AnalysisContextSchema,
  AnalysisInputBindingSchema,
  AnalysisInputRole,
  AnalyzeBondRequestSchema,
  AnalyzeCarryRollRequestSchema,
  AnalyzeFuturesDeliveryRequestSchema,
  AnalyzeFuturesDeliveryResultSchema,
  AnalyzeFuturesHedgeRequestSchema,
  ArtifactBindingSchema,
  CurveNodeBindingSchema,
  FuturesDeliveryCandidateResultSchema,
  FuturesDeliveryMeasuresSchema,
  InterpolateYieldCurveRequestSchema,
  ObjectBindingSchema,
  ParameterDigestSchema,
  ResultMetadataSchema,
  SnapshotBindingSchema,
  TaxAdjustedBondAnalyticsSchema,
} from "../../packages/contracts-generated/src/ficant/rates/v1/analytics_pb";
import {
  BookSchema,
  D01ProjectionSchema,
  PortfolioAggregationService,
  PortfolioCatalogService,
  PortfolioCoverageSchema,
  PortfolioOverviewSchema,
  PortfolioPageDataMode,
  PortfolioPageEnvelopeSchema,
  PortfolioPerformanceCoverageSchema,
  PortfolioPerformanceSeriesSchema,
  PortfolioPerformanceService,
  PortfolioStatus,
  PortfolioWorkbenchService,
} from "../../packages/contracts-generated/src/ficant/portfolio/v1/portfolio_pb";
import {
  DataSnapshotSchema,
  SnapshotService,
} from "../../packages/contracts-generated/src/ficant/research/v1/snapshot_pb";
import {
  ArtifactKind,
  ArtifactSchema,
  ArtifactService,
  GetArtifactResponseSchema,
  LineagePageSchema,
  ReadArtifactLineageResponseSchema,
} from "../../packages/contracts-generated/src/ficant/research/v1/artifact_pb";
import { describe, expect, it } from "vitest";

describe("Q2-CTR-03 TypeScript 生成契约消费", () => {
  it("直接导入生成 schema 与 service descriptor，不复制 DTO", () => {
    const app = create(AppDescriptorSchema, {
      appId: "consumer-proof",
      displayName: "契约消费证明",
      entrypoint: "/consumer-proof",
      allowedOrigin: "https://apps.example.invalid",
    });
    const session = create(SessionSchema, {
      sessionId: "session-proof",
      activeRole: PlatformRole.RESEARCHER,
    });
    const change = create(ChangeJustificationSchema, {
      reason: "human-approved",
    });
    const changeRecord = create(FoundationChangeRecordSchema, {
      activeRole: PlatformRole.RESEARCHER,
      change,
    });
    const subjectOwner = create(OwnerRefSchema, {
      tenantId: { value: "01ARZ3NDEKTSV4RRFFQ69G5FAT" },
      ownerId: { value: "01ARZ3NDEKTSV4RRFFQ69G5FAP" },
    });
    const subjectRequest = create(RegisterSubjectRequestSchema, {
      subject: create(SubjectSchema, {
        subjectId: { value: "01ARZ3NDEKTSV4RRFFQ69G5FAS" },
        displayName: "consumer Subject",
        owner: subjectOwner,
      }),
      idempotencyKey: "fixture",
    });
    const subjectStateRequest = create(RegisterSubjectStateRequestSchema, {
      snapshot: create(SubjectStateSnapshotSchema, { owner: subjectOwner }),
      idempotencyKey: "subject-state-consumer-v1",
    });
    const completeInstrument = create(CompleteInstrumentDefinitionSchema, {});
    const marketDefinition = create(MarketDefinitionSchema, {
      definition: { case: "instrument", value: completeInstrument },
    });
    const marketFact = create(MarketFactSchema, {});
    const cashflow = create(CashflowSchema, {
      cashflowType: CashflowType.COUPON,
    });
    const valuation = create(ValuationSchema, {
      values: [create(DecimalValueSchema), create(DecimalValueSchema)],
      valueRoles: [
        ValuationValueRole.YIELD,
        ValuationValueRole.REMAINING_YEARS,
      ],
    });
    const curveInput = create(CurveSnapshotInputSchema, {});
    const curvePublish = create(PublishCurveSnapshotRequestSchema, {
      points: create(CurvePointSetSchema, {}),
      curve: curveInput,
    });
    const knowledgeAt = create(MarketTimeSchema, {
      marketTimezone: "Asia/Shanghai",
      localTradingDate: "2026-08-19",
    });
    const factQuery = create(QueryInstrumentFactsRequestSchema, { knowledgeAt });
    const curveQuery = create(GetCurveSnapshotRequestSchema, { knowledgeAt });
    const authorization = create(DataSourceAuthorizationSchema, {});
    const dataSnapshot = create(DataSnapshotSchema, {});
    const mapping = create(InstrumentMappingSchema, {});
    const cgbRule = create(CgbFuturesProductRuleSchema, {
      productCode: "T",
      originalTermMaxMonths: 120,
      residualMinMonths: 78,
      residualUpperBound: { case: "residualMaxMonthsUnbounded", value: true },
    });
    const cgbPack = create(CgbFuturesDeliveryRulePackSchema, {
      products: [cgbRule],
      deliveryMonths: [3, 6, 9, 12],
      accruedInterestDayCount: 1,
    });
    const fundingRate = create(FundingTierRateSchema, {});
    const fundingPack = create(FundingRulePackSchema, { rates: [fundingRate] });
    const taxAttributes = create(BondTaxAttributesSchema, {
      valueAddedTaxStatus: ValueAddedTaxStatus.TAXABLE,
      incomeTaxStatus: IncomeTaxStatus.TAXABLE,
    });
    const taxRate = create(SubjectCouponTaxRateSchema, {
      valueAddedTaxProfile: "synthetic-vat",
      incomeTaxProfile: "synthetic-income",
    });
    const taxRule = create(BondCouponTaxRuleSchema, {
      firstIssueFrom: "2000-01-01",
      taxAttributes,
      rates: [taxRate],
    });
    const taxPack = create(TaxRulePackSchema, { couponRules: [taxRule] });
    const rateUnit = create(UnitRefSchema, {
      unitId: create(UlidSchema, { value: "01K2CGBVAT0000000000000000" }),
      version: 1n,
    });
    const vatRate = create(DecimalValueSchema, {
      coefficient: "6",
      scale: 2,
      unit: rateUnit,
    });
    const incomeTaxRate = create(DecimalValueSchema, {
      coefficient: "0",
      scale: 0,
      unit: rateUnit,
    });
    const taxTreatment = create(SubjectCouponTaxTreatmentSchema, {
      valueAddedTaxProfile: "general-taxpayer",
      incomeTaxProfile: "general-enterprise",
      valueAddedTaxRate: vatRate,
      incomeTaxRate,
      grossCouponBasis: GrossCouponTaxBasis.VAT_INCLUDED,
      rounding: TaxRoundingMode.TIES_TO_EVEN,
      claimScope:
        CouponTaxClaimScope.COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    });
    const treatmentRule = create(BondCouponTaxTreatmentRuleSchema, {
      firstIssueFrom: "2025-08-08",
      taxAttributes,
      treatments: [taxTreatment],
    });
    const taxPackV2 = create(TaxRulePackV2Schema, {
      couponRules: [treatmentRule],
    });
    const rulePack = create(MarketRulePackSchema, {
      market: "CFFEX",
      ruleType: "cgb-futures",
      content: {
        typeUrl: "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
        value: new Uint8Array([1]),
      },
    });
    const snapshotBinding = create(SnapshotBindingSchema, {
      snapshotId: create(UlidSchema, { value: "01ARZ3NDEKTSV4RRFFQ69G5FAA" }),
      contentHash: create(Sha256Schema, { value: new Uint8Array(32).fill(1) }),
    });
    const artifactBinding = create(ArtifactBindingSchema, {
      artifactId: create(UlidSchema, { value: "01ARZ3NDEKTSV4RRFFQ69G5FAB" }),
      contentHash: create(Sha256Schema, { value: new Uint8Array(32).fill(2) }),
    });
    const curveNodeBinding = create(CurveNodeBindingSchema, {
      curveNodeId: "cn.gov.yield-curve.10y",
      contentHash: create(Sha256Schema, {
        value: new Uint8Array(32).fill(5),
      }),
    });
    const inputBinding = create(AnalysisInputBindingSchema, {
      role: AnalysisInputRole.DATA_SNAPSHOT,
      binding: { case: "snapshot", value: snapshotBinding },
    });
    const algorithm = create(AlgorithmBindingSchema, {
      algorithmId: "rates-reference",
      algorithmVersion: 1,
      conventionProfile: "r5d",
      abiVersion: 1,
    });
    const parameterDigest = create(ParameterDigestSchema, {
      algorithm,
      canonicalParametersSha256: create(Sha256Schema, {
        value: new Uint8Array(32).fill(3),
      }),
    });
    const metadata = create(ResultMetadataSchema, {
      algorithm,
      consumedInputs: [inputBinding],
      parameterDigest,
      requestFingerprint: create(Sha256Schema, {
        value: new Uint8Array(32).fill(4),
      }),
    });
    const context = create(AnalysisContextSchema, {
      algorithm,
      knowledgeAt: create(MarketTimeSchema, {
        marketTimezone: "Asia/Shanghai",
        localTradingDate: "2026-08-12",
      }),
    });
    const objectBinding = create(ObjectBindingSchema, {});
    const bondRequest = create(AnalyzeBondRequestSchema, {
      context,
      calendar: objectBinding,
      dataSnapshot: snapshotBinding,
      taxRulePack: objectBinding,
    });
    const curveRequest = create(InterpolateYieldCurveRequestSchema, {
      context,
      curve: snapshotBinding,
    });
    const carryRequest = create(AnalyzeCarryRollRequestSchema, {
      context,
      curve: snapshotBinding,
    });
    const deliveryRequest = create(AnalyzeFuturesDeliveryRequestSchema, {
      context,
      dataSnapshot: snapshotBinding,
      fundingRulePack: objectBinding,
      taxRulePack: objectBinding,
    });
    const deliveryMeasures = create(FuturesDeliveryMeasuresSchema, {
      taxAdjustedInterimCoupons: create(DecimalValueSchema, {
        coefficient: "2830188679245",
        scale: 12,
        unit: rateUnit,
      }),
      subjectTaxAdjustedIrr: create(DecimalValueSchema, {
        coefficient: "123456789",
        scale: 12,
        unit: rateUnit,
      }),
    });
    const deliveryCandidate = create(FuturesDeliveryCandidateResultSchema, {
      measures: deliveryMeasures,
      claimScope:
        CouponTaxClaimScope.COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    });
    const deliveryResult = create(AnalyzeFuturesDeliveryResultSchema, {
      candidates: [deliveryCandidate],
      subjectCtdIndex: 1,
    });
    const afterTax = create(TaxAdjustedBondAnalyticsSchema, {
      claimScope:
        CouponTaxClaimScope.COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    });
    const hedgeRequest = create(AnalyzeFuturesHedgeRequestSchema, {
      context,
      targetRiskArtifact: artifactBinding,
      deliveryArtifact: artifactBinding,
      ctdAnalyticsArtifact: artifactBinding,
    });
    const portfolioBook = create(BookSchema, {
      status: PortfolioStatus.ACTIVE,
    });
    const portfolioCoverage = create(PortfolioCoverageSchema, {
      missingReasons: [],
    });
    const portfolioOverview = create(PortfolioOverviewSchema, {
      coverage: portfolioCoverage,
    });
    const portfolioPage = create(PortfolioPageEnvelopeSchema, {
      schemaVersion: "portfolio-workbench.v1",
      dataMode: PortfolioPageDataMode.REAL,
      coverage: portfolioCoverage,
      projection: {
        case: "d01",
        value: create(D01ProjectionSchema, { overview: portfolioOverview }),
      },
    });
    const performanceCoverage = create(PortfolioPerformanceCoverageSchema, {
      expectedSessionCount: 2n,
      observedSessionCount: 2n,
      expectedPortfolioObservationCount: 4n,
      observedPortfolioObservationCount: 4n,
      expectedBenchmarkObservationCount: 2n,
      observedBenchmarkObservationCount: 2n,
    });
    const performanceSeries = create(PortfolioPerformanceSeriesSchema, {
      coverage: performanceCoverage,
    });

    expect(app.$typeName).toBe("ficant.app.v1.AppDescriptor");
    expect(session.$typeName).toBe("ficant.app.v1.Session");
    expect(session.activeRole).toBe(PlatformRole.RESEARCHER);
    expect(changeRecord.change?.reason).toBe("human-approved");
    expect(marketDefinition.definition.case).toBe("instrument");
    expect(marketFact.$typeName).toBe("ficant.market.v1.MarketFact");
    expect(cashflow.cashflowType).toBe(CashflowType.COUPON);
    expect(FormalInputKind.FACT).toBe(21);
    expect(FormalInputKind.PORTFOLIO_VALUATION_SNAPSHOT).toBe(22);
    expect(FormalInputKind.BENCHMARK_LEVEL_SNAPSHOT).toBe(23);
    expect(FormalInputKind.PORTFOLIO_PERFORMANCE_CONVENTION).toBe(24);
    expect(valuation.valueRoles).toEqual([
      ValuationValueRole.YIELD,
      ValuationValueRole.REMAINING_YEARS,
    ]);
    expect("contentHash" in curveInput).toBe(false);
    expect("curveSnapshot" in curvePublish).toBe(false);
    expect(curvePublish.curve?.$typeName).toBe(
      "ficant.market.v1.CurveSnapshotInput",
    );
    expect(factQuery.knowledgeAt?.marketTimezone).toBe("Asia/Shanghai");
    expect(curveQuery.knowledgeAt?.localTradingDate).toBe("2026-08-19");
    expect(authorization.$typeName).toBe(
      "ficant.market.v1.DataSourceAuthorization",
    );
    expect(dataSnapshot.$typeName).toBe("ficant.research.v1.DataSnapshot");
    expect(mapping.$typeName).toBe("ficant.market.v1.InstrumentMapping");
    expect(FoundationChangeService.typeName).toBe(
      "ficant.core.v1.FoundationChangeService",
    );
    expect(DataSourceRegistryService.typeName).toBe(
      "ficant.market.v1.DataSourceRegistryService",
    );
    expect(MarketDefinitionService.typeName).toBe(
      "ficant.market.v1.MarketDefinitionService",
    );
    expect(MarketFactService.typeName).toBe(
      "ficant.market.v1.MarketFactService",
    );
    expect(SnapshotService.typeName).toBe(
      "ficant.research.v1.SnapshotService",
    );
    expect(PlatformService.typeName).toBe("ficant.app.v1.PlatformService");
    expect(subjectRequest.subject?.owner).toEqual(subjectOwner);
    expect(subjectRequest.idempotencyKey).toBe("fixture");
    expect(subjectStateRequest.snapshot?.owner).toEqual(subjectOwner);
    expect(subjectStateRequest.idempotencyKey).toBe(
      "subject-state-consumer-v1",
    );
    expect(cgbPack.$typeName).toBe("ficant.market.v1.CgbFuturesDeliveryRulePack");
    expect(cgbRule.residualUpperBound.case).toBe("residualMaxMonthsUnbounded");
    expect(fundingPack.$typeName).toBe("ficant.market.v1.FundingRulePack");
    expect(fundingRate.$typeName).toBe("ficant.market.v1.FundingTierRate");
    expect(taxPack.$typeName).toBe("ficant.market.v1.TaxRulePack");
    expect(taxPackV2.$typeName).toBe("ficant.market.v1.TaxRulePackV2");
    expect(taxTreatment.valueAddedTaxRate?.unit).toEqual(rateUnit);
    expect(taxTreatment.grossCouponBasis).toBe(
      GrossCouponTaxBasis.VAT_INCLUDED,
    );
    expect(taxTreatment.rounding).toBe(TaxRoundingMode.TIES_TO_EVEN);
    expect(taxTreatment.claimScope).toBe(
      CouponTaxClaimScope.COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    );
    expect(taxRule.$typeName).toBe("ficant.market.v1.BondCouponTaxRule");
    expect(taxRate.$typeName).toBe("ficant.market.v1.SubjectCouponTaxRate");
    expect(taxAttributes.$typeName).toBe("ficant.market.v1.BondTaxAttributes");
    expect(rulePack.content?.typeUrl).toBe(
      "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
    );
    expect(context.knowledgeAt?.localTradingDate).toBe("2026-08-12");
    expect("rulePack" in context).toBe(false);
    expect(inputBinding.binding.case).toBe("snapshot");
    expect(inputBinding.binding.value?.$typeName).toBe(
      "ficant.rates.v1.SnapshotBinding",
    );
    expect(metadata.consumedInputs[0]?.role).toBe(
      AnalysisInputRole.DATA_SNAPSHOT,
    );
    expect(AnalysisInputRole.CURVE_RULE_PACK).toBe(15);
    expect(AnalysisInputRole.CURVE_NODE_DEFINITION).toBe(16);
    const curveNodeInputBinding = create(AnalysisInputBindingSchema, {
      role: AnalysisInputRole.CURVE_NODE_DEFINITION,
      binding: { case: "curveNode", value: curveNodeBinding },
    });
    expect(curveNodeInputBinding.binding.case).toBe("curveNode");
    if (curveNodeInputBinding.binding.case !== "curveNode") {
      throw new Error("curve-node definition evidence must use CurveNodeBinding");
    }
    expect(curveNodeInputBinding.binding.value.curveNodeId).toBe(
      "cn.gov.yield-curve.10y",
    );
    expect(metadata.parameterDigest?.algorithm?.algorithmId).toBe(
      "rates-reference",
    );
    expect(metadata.requestFingerprint?.value).toHaveLength(32);
    expect(bondRequest.calendar?.$typeName).toBe(
      "ficant.rates.v1.ObjectBinding",
    );
    expect(bondRequest.dataSnapshot?.$typeName).toBe(
      "ficant.rates.v1.SnapshotBinding",
    );
    expect("terms" in bondRequest).toBe(false);
    expect(curveRequest.curve?.$typeName).toBe(
      "ficant.rates.v1.SnapshotBinding",
    );
    expect(carryRequest.curve?.$typeName).toBe(
      "ficant.rates.v1.SnapshotBinding",
    );
    expect("calendar" in carryRequest).toBe(false);
    expect(deliveryRequest.dataSnapshot?.$typeName).toBe(
      "ficant.rates.v1.SnapshotBinding",
    );
    expect(deliveryRequest.taxRulePack?.$typeName).toBe(
      "ficant.rates.v1.ObjectBinding",
    );
    expect(deliveryResult.candidates[0]?.measures?.taxAdjustedInterimCoupons?.scale).toBe(12);
    expect(deliveryResult.candidates[0]?.claimScope).not.toBe(
      CouponTaxClaimScope.UNSPECIFIED,
    );
    expect(deliveryResult.subjectCtdIndex).toBe(1);
    expect(afterTax.claimScope).not.toBe(CouponTaxClaimScope.UNSPECIFIED);
    expect("candidates" in deliveryRequest).toBe(false);
    expect(hedgeRequest.targetRiskArtifact?.$typeName).toBe(
      "ficant.rates.v1.ArtifactBinding",
    );
    expect("targetDv01" in hedgeRequest).toBe(false);
    expect(portfolioBook.$typeName).toBe("ficant.portfolio.v1.Book");
    expect(portfolioPage.schemaVersion).toBe("portfolio-workbench.v1");
    expect(portfolioPage.projection.case).toBe("d01");
    expect(portfolioPage.coverage?.missingReasons).toEqual([]);
    expect(performanceSeries.$typeName).toBe(
      "ficant.portfolio.v1.PortfolioPerformanceSeries",
    );
    expect(performanceSeries.coverage?.expectedPortfolioObservationCount).toBe(4n);
    expect("DEMO" in PortfolioPageDataMode).toBe(false);
    expect(PortfolioCatalogService.methods.map((method) => method.localName)).toEqual([
      "listBooksAndPortfolios",
    ]);
    expect(PortfolioAggregationService.methods.map((method) => method.localName)).toEqual([
      "getPortfolioOverview",
    ]);
    expect(PortfolioPerformanceService.methods.map((method) => method.localName)).toEqual([
      "getPortfolioPerformance",
    ]);
    expect(PortfolioWorkbenchService.methods.map((method) => method.localName)).toEqual([
      "getDefaultContext",
      "getPage",
    ]);
    const artifact = create(ArtifactSchema, { kind: ArtifactKind.GENERIC });
    const artifactResponse = create(GetArtifactResponseSchema, {
      result: { case: "artifact", value: artifact },
    });
    const lineagePage = create(LineagePageSchema);
    const lineageResponse = create(ReadArtifactLineageResponseSchema, {
      result: { case: "lineagePage", value: lineagePage },
    });
    expect(artifactResponse.result.case).toBe("artifact");
    expect(lineageResponse.result.case).toBe("lineagePage");
    expect(ArtifactService.methods.map((method) => method.localName).sort()).toEqual([
      "getArtifact",
      "getSignalSet",
      "readArtifactLineage",
      "readSignalSetLineage",
    ]);
    expect(Object.values(ArtifactKind).filter((value) => typeof value === "number")).toEqual([
      0,
      1,
      5,
    ]);
  });
});
