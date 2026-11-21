import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
import { createTrustedRulePackPlugin } from './vite.config';

export default defineConfig({
  plugins: [react(), createTrustedRulePackPlugin()],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    exclude: ['e2e/**', 'node_modules/**', 'dist/**', 'src/generated/**'],
  },
});
