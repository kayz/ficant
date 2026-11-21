export type PurityViolationCategory =
  | 'market-branch'
  | 'futures-product-branch'
  | 'issuer-branch'
  | 'hardcoded-notional-constant'
  | 'local-pricing-formula'
  | 'silent-clipboard-read'
  | 'browser-storage-leak';

export interface PurityViolation {
  readonly path: string;
  readonly line: number;
  readonly category: PurityViolationCategory;
  readonly text: string;
}

interface PurityRule {
  readonly category: PurityViolationCategory;
  readonly pattern: RegExp;
}

const PURITY_RULES: readonly PurityRule[] = [
  {
    category: 'market-branch',
    pattern: /\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*(?:===?|!==?)\s*['"](?:CN|US|EU|JP|HK)['"]/i,
  },
  {
    category: 'market-branch',
    pattern: /['"](?:CN|US|EU|JP|HK)['"]\s*(?:===?|!==?)\s*\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?/i,
  },
  {
    category: 'market-branch',
    pattern: /\bcase\s+['"](?:CN|US|EU|JP|HK)['"]\s*:/i,
  },
  {
    category: 'futures-product-branch',
    pattern: /\b(?:productCode|product_code|product)\s*(?:===?|!==?)\s*['"](?:TS|TF|TL|T)['"]/,
  },
  {
    category: 'futures-product-branch',
    pattern: /['"](?:TS|TF|TL|T)['"]\s*(?:===?|!==?)\s*\b(?:productCode|product_code|product)\b/,
  },
  {
    category: 'futures-product-branch',
    pattern: /\bcase\s+['"](?:TS|TF|TL|T)['"]\s*:/,
  },
  {
    category: 'issuer-branch',
    pattern: /\b(?:issuer|issuerCode|issuer_code)\s*(?:===?|!==?)\s*['"](?:CDB|EIBC|ADBC|MOF)['"]/i,
  },
  {
    category: 'issuer-branch',
    pattern: /\bcase\s+['"](?:CDB|EIBC|ADBC|MOF)['"]\s*:/i,
  },
  {
    category: 'hardcoded-notional-constant',
    pattern:
      /\b(?:CGB_FUTURES_CONTRACT_NOTIONAL|contract_notional|contractNotional)\s*[:=]\s*(?:1_?000_?000|2_?000_?000)\b/i,
  },
  {
    category: 'local-pricing-formula',
    pattern: /\b(?:conversion_factor|conversionFactor)\s*[:=]\s*\d+\.\d+\b/i,
  },
  {
    category: 'local-pricing-formula',
    pattern: /\b(?:tax_rate|taxRate|vat_rate|vatRate)\s*[:=]\s*\d+\.\d+\b/i,
  },
  {
    category: 'silent-clipboard-read',
    pattern: /\bnavigator\s*\.\s*clipboard\s*\.\s*read(?:Text)?\s*\(/,
  },
  {
    category: 'browser-storage-leak',
    pattern: /\b(?:localStorage|sessionStorage)\s*\.\s*(?:setItem|getItem)\s*\(/,
  },
  {
    category: 'browser-storage-leak',
    pattern: /\bdocument\s*\.\s*cookie\s*=/,
  },
];

const EXCLUDED_SEGMENT_PATTERN =
  /(^|\/)(?:generated|__tests__|fixtures|dist|node_modules|coverage|\.vite)(\/|$)/i;
const TEST_FILE_PATTERN = /\.(?:test|spec)\.[cm]?[jt]sx?$/i;
const SOURCE_EXTENSION_PATTERN = /\.[cm]?[jt]sx?$/i;

export function normalizeRelativePath(relativePath: string): string {
  return relativePath.replace(/\\/g, '/').replace(/^\.\//, '');
}

export function isProductionSourceRelativePath(relativePath: string): boolean {
  const normalized = normalizeRelativePath(relativePath);
  if (!SOURCE_EXTENSION_PATTERN.test(normalized)) {
    return false;
  }
  if (EXCLUDED_SEGMENT_PATTERN.test(normalized)) {
    return false;
  }
  if (TEST_FILE_PATTERN.test(normalized)) {
    return false;
  }
  return true;
}

export function scanSourceTextForPurityViolations(
  relativePath: string,
  content: string,
): readonly PurityViolation[] {
  const normalizedPath = normalizeRelativePath(relativePath);
  const lines = content.split(/\r?\n/);
  const violations: PurityViolation[] = [];

  for (let idx = 0; idx < lines.length; idx += 1) {
    const lineText = lines[idx] ?? '';
    for (const rule of PURITY_RULES) {
      if (rule.pattern.test(lineText)) {
        violations.push({
          path: normalizedPath,
          line: idx + 1,
          category: rule.category,
          text: lineText.trim(),
        });
      }
    }
  }

  return violations;
}
