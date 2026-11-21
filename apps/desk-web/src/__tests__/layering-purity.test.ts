// @vitest-environment node
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  isProductionSourceRelativePath,
  normalizeRelativePath,
  scanSourceTextForPurityViolations,
  type PurityViolation,
} from '../app/purityScanner';

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const FIXTURES_DIR = join(THIS_DIR, 'fixtures');
const DESK_WEB_ROOT = resolve(THIS_DIR, '..', '..');
const DESK_WEB_SRC = join(DESK_WEB_ROOT, 'src');
const REPO_ROOT = resolve(DESK_WEB_ROOT, '..', '..');
const LAYERING_SCRIPT_PATH = join(REPO_ROOT, 'scripts', 'check-layering.ps1');

function collectFilesRecursively(dir: string): string[] {
  const results: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...collectFilesRecursively(fullPath));
    } else if (entry.isFile()) {
      results.push(fullPath);
    }
  }
  return results;
}

function scanDirectoryForPurity(rootDir: string, displayBaseDir: string): {
  readonly scannedFiles: readonly string[];
  readonly violations: readonly PurityViolation[];
} {
  const allFiles = collectFilesRecursively(rootDir);
  const scannedFiles: string[] = [];
  const violations: PurityViolation[] = [];

  for (const fullPath of allFiles) {
    const relFromRoot = normalizeRelativePath(relative(rootDir, fullPath));
    if (!isProductionSourceRelativePath(relFromRoot)) {
      continue;
    }
    const displayRel = normalizeRelativePath(relative(displayBaseDir, fullPath));
    scannedFiles.push(displayRel);
    const content = readFileSync(fullPath, 'utf8');
    violations.push(...scanSourceTextForPurityViolations(displayRel, content));
  }

  return { scannedFiles, violations };
}

function runLayeringScript(repositoryRoot?: string): {
  readonly status: number;
  readonly output: string;
} {
  const args = ['-NoProfile', '-NonInteractive', '-File', LAYERING_SCRIPT_PATH];
  if (repositoryRoot !== undefined) {
    args.push('-RepositoryRoot', repositoryRoot);
  }
  const proc = spawnSync('pwsh', args, {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    windowsHide: true,
    timeout: 60_000,
  });
  if (proc.error) {
    throw proc.error;
  }
  const output = `${proc.stdout ?? ''}\n${proc.stderr ?? ''}`.trim();
  return {
    status: proc.status ?? 1,
    output,
  };
}

function createCleanTempRepoMirror(): string {
  const tmpRepo = mkdtempSync(join(tmpdir(), 'ficant-layering-mirror-'));
  mkdirSync(join(tmpRepo, 'scripts'), { recursive: true });
  mkdirSync(join(tmpRepo, 'crates', 'ficant-domain', 'src'), { recursive: true });
  mkdirSync(join(tmpRepo, 'apps', 'desk-web', 'src'), { recursive: true });
  writeFileSync(join(tmpRepo, 'scripts', 'layering-allowlist.json'), '[]\n', 'utf8');
  copyFileSync(
    join(FIXTURES_DIR, 'clean-sample.ts'),
    join(tmpRepo, 'apps', 'desk-web', 'src', 'clean-sample.ts'),
  );
  return tmpRepo;
}

describe('ADR-0013 frontend purity and repository layering invariants', () => {
  it('passes frontend purity scan across real apps/desk-web/src production files with zero violations', () => {
    const { scannedFiles, violations } = scanDirectoryForPurity(DESK_WEB_SRC, REPO_ROOT);

    expect(scannedFiles.length).toBeGreaterThan(10);
    expect(scannedFiles).toContain('apps/desk-web/src/main.tsx');
    expect(scannedFiles).toContain('apps/desk-web/src/app/DeskApp.tsx');
    expect(violations).toEqual([]);
  });

  it('rejects plain-text negative fixtures copied into a temporary apps/desk-web/src tree', () => {
    const tmpRepo = createCleanTempRepoMirror();
    try {
      const tmpSrc = join(tmpRepo, 'apps', 'desk-web', 'src');

      // Clean baseline passes frontend purity scan.
      const cleanScan = scanDirectoryForPurity(tmpSrc, tmpRepo);
      expect(cleanScan.scannedFiles).toEqual(['apps/desk-web/src/clean-sample.ts']);
      expect(cleanScan.violations).toEqual([]);

      // Copy hardcoded market-branch fixture into production src path.
      copyFileSync(
        join(FIXTURES_DIR, 'bad-market-branch.ts'),
        join(tmpSrc, 'bad-market-branch.ts'),
      );
      const marketScan = scanDirectoryForPurity(tmpSrc, tmpRepo);
      expect(marketScan.violations.length).toBeGreaterThan(0);
      expect(marketScan.violations).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            path: 'apps/desk-web/src/bad-market-branch.ts',
            category: 'market-branch',
          }),
        ]),
      );

      // Copy product constant + browser storage + silent clipboard fixture into production src path.
      copyFileSync(
        join(FIXTURES_DIR, 'bad-product-and-storage.ts'),
        join(tmpSrc, 'bad-product-and-storage.ts'),
      );
      const fullBadScan = scanDirectoryForPurity(tmpSrc, tmpRepo);
      const categoriesForProductFixture = fullBadScan.violations
        .filter((v) => v.path === 'apps/desk-web/src/bad-product-and-storage.ts')
        .map((v) => v.category);

      expect(categoriesForProductFixture).toContain('futures-product-branch');
      expect(categoriesForProductFixture).toContain('hardcoded-notional-constant');
      expect(categoriesForProductFixture).toContain('browser-storage-leak');
      expect(categoriesForProductFixture).toContain('silent-clipboard-read');
    } finally {
      rmSync(tmpRepo, { recursive: true, force: true });
    }
  });

  it('passes real-repository check-layering.ps1 and fails non-zero when bad-market-branch fixture is placed in temp mirror src', () => {
    // 1. Real repository check passes (fixtures/ is excluded by check-layering.ps1).
    const realRepoResult = runLayeringScript();
    expect(realRepoResult.status).toBe(0);
    expect(realRepoResult.output).toContain('AC03=0');

    // 2. Clean temp mirror passes, then injecting bad-market-branch.ts triggers AC03 failure naming that exact file.
    const tmpRepo = createCleanTempRepoMirror();
    try {
      const cleanMirrorResult = runLayeringScript(tmpRepo);
      expect(cleanMirrorResult.status).toBe(0);
      expect(cleanMirrorResult.output).toContain('AC03=0');

      copyFileSync(
        join(FIXTURES_DIR, 'bad-market-branch.ts'),
        join(tmpRepo, 'apps', 'desk-web', 'src', 'bad-market-branch.ts'),
      );

      const badMirrorResult = runLayeringScript(tmpRepo);
      expect(badMirrorResult.status).not.toBe(0);
      expect(badMirrorResult.output).toContain('apps/desk-web/src/bad-market-branch.ts');
      expect(badMirrorResult.output).toContain('AC03');
    } finally {
      rmSync(tmpRepo, { recursive: true, force: true });
    }
  }, 60_000);
});
