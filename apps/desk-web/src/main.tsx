import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';

function DeskWebScaffold() {
  return (
    <main data-testid="desk-web-scaffold" data-desk-web-mode="scaffold">
      <h1>FICANT Desk Web Scaffold</h1>
      <p data-testid="trusted-rulepack-id">
        RulePack: {TRUSTED_RULEPACK_ANCHOR.manifest.rule_pack_id} (v
        {TRUSTED_RULEPACK_ANCHOR.manifest.version})
      </p>
    </main>
  );
}

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Missing #root mount element.');
}

createRoot(rootElement).render(
  <StrictMode>
    <DeskWebScaffold />
  </StrictMode>,
);
