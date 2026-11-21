// Plain-text negative fixture exercising frontend purity rules:
// futures product branch, hardcoded notional constant, browser storage leak, and silent clipboard read.
export function forbiddenFrontendOperations(productCode: string, token: string): Promise<string> {
  const contractNotional = 2_000_000;
  if (productCode === 'TS') {
    window.localStorage.setItem('ficant_token', token);
    return navigator.clipboard.readText().then((clip) => `${clip}:${contractNotional}`);
  }
  return Promise.resolve(`${productCode}:${contractNotional}`);
}
