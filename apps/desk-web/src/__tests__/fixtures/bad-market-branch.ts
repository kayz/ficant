// Plain-text negative fixture kept under fixtures/ (excluded from real-repo layering scan)
// and copied into a temporary repository's apps/desk-web/src/ during layering-purity tests.
export function resolveHardcodedMarketBranch(market: string): string {
  if (market === 'CN') {
    return 'hardcoded-domestic-branch';
  }
  return market;
}
