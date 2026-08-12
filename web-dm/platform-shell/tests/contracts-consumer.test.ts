import { create } from "@bufbuild/protobuf";
import {
  AppDescriptorSchema,
  PlatformService,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { SessionSchema } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";
import {
  DecimalValueSchema,
  MarketTimeSchema,
  Sha256Schema,
  UlidSchema,
  UnitRefSchema,
} from "../../packages/contracts-generated/src/ficant/core/v1/common_pb";
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
  IncomeTaxStatus,
  ValueAddedTaxStatus,
} from "../../packages/contracts-generated/src/ficant/market/v1/definition_pb";
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
import { describe, expect, it } from "vitest";

describe("Q2-CTR-03 TypeScript 生成契约消费", () => {
  it("直接导入生成 schema 与 service descriptor，不复制 DTO", () => {
    const app = create(AppDescriptorSchema, {
      appId: "consumer-proof",
      displayName: "契约消费证明",
      entrypoint: "/consumer-proof",
      allowedOrigin: "https://apps.example.invalid",
    });
    const session = create(SessionSchema, { sessionId: "session-proof" });
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

    expect(app.$typeName).toBe("ficant.app.v1.AppDescriptor");
    expect(session.$typeName).toBe("ficant.app.v1.Session");
    expect(PlatformService.typeName).toBe("ficant.app.v1.PlatformService");
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
  });
});
