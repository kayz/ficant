export interface SnapshotSummaryBanner {
  readonly accountId: string;
  readonly allowedProducts: readonly string[];
}

export function formatSnapshotBanner(summary: SnapshotSummaryBanner): string {
  return `${summary.accountId}: [${summary.allowedProducts.join(', ')}]`;
}
