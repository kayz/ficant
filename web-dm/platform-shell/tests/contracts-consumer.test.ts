import { create } from "@bufbuild/protobuf";
import {
  AppDescriptorSchema,
  PlatformService,
} from "../../packages/contracts-generated/src/ficant/app/v1/registry_pb";
import { SessionSchema } from "../../packages/contracts-generated/src/ficant/app/v1/session_pb";
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
  BondCouponTaxRuleSchema,
  SubjectCouponTaxRateSchema,
  TaxRulePackSchema,
} from "../../packages/contracts-generated/src/ficant/market/v1/tax_rule_pb";
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
    const rulePack = create(MarketRulePackSchema, {
      market: "CFFEX",
      ruleType: "cgb-futures",
      content: {
        typeUrl: "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
        value: new Uint8Array([1]),
      },
    });

    expect(app.$typeName).toBe("ficant.app.v1.AppDescriptor");
    expect(session.$typeName).toBe("ficant.app.v1.Session");
    expect(PlatformService.typeName).toBe("ficant.app.v1.PlatformService");
    expect(cgbPack.$typeName).toBe("ficant.market.v1.CgbFuturesDeliveryRulePack");
    expect(cgbRule.residualUpperBound.case).toBe("residualMaxMonthsUnbounded");
    expect(fundingPack.$typeName).toBe("ficant.market.v1.FundingRulePack");
    expect(fundingRate.$typeName).toBe("ficant.market.v1.FundingTierRate");
    expect(taxPack.$typeName).toBe("ficant.market.v1.TaxRulePack");
    expect(taxRule.$typeName).toBe("ficant.market.v1.BondCouponTaxRule");
    expect(taxRate.$typeName).toBe("ficant.market.v1.SubjectCouponTaxRate");
    expect(taxAttributes.$typeName).toBe("ficant.market.v1.BondTaxAttributes");
    expect(rulePack.content?.typeUrl).toBe(
      "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
    );
  });
});
