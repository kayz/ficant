#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const SCRIPTS_DIR = dirname(SCRIPT_PATH);
const DESK_WEB_ROOT = resolve(SCRIPTS_DIR, '..');
const REPO_ROOT = resolve(DESK_WEB_ROOT, '..', '..');

const FROZEN_ROOTS = [
  'interface',
  'crates/ficant-contracts',
  'python/node-contracts',
];

function fail(message) {
  console.error(`[verify-codegen] ERROR: ${message}`);
  process.exit(1);
}

function runCommand(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    encoding: 'utf8',
    windowsHide: true,
    ...options,
  });
  if (result.error) {
    throw new Error(`Failed to spawn "${cmd}": ${result.error.message}`);
  }
  return result;
}

function runBufCommand(args, options = {}) {
  if (process.platform !== 'win32') {
    return runCommand('buf', args, options);
  }
  const pathDirs = (process.env['PATH'] || '').split(';').filter(Boolean);
  for (const dir of pathDirs) {
    const exePath = join(dir, 'buf.exe');
    if (existsSync(exePath)) {
      return runCommand(exePath, args, options);
    }
    const cmdPath = join(dir, 'buf.cmd');
    if (existsSync(cmdPath)) {
      const comspec = process.env['ComSpec'] || 'cmd.exe';
      return runCommand(comspec, ['/d', '/c', cmdPath, ...args], options);
    }
  }
  return runCommand('buf', args, options);
}

function assertFrozenRootsClean(repoRoot, phase) {
  const res = runCommand(
    'git',
    [
      '-C',
      repoRoot,
      'status',
      '--porcelain=v1',
      '--untracked-files=all',
      '--',
      ...FROZEN_ROOTS,
    ],
  );
  if (res.status !== 0) {
    throw new Error(
      `git status check failed (${phase}) with exit ${String(res.status)}: ${(res.stderr || res.stdout || '').trim()}`,
    );
  }
  const output = (res.stdout || '').trim();
  if (output.length > 0) {
    throw new Error(
      `Frozen root drift detected (${phase}) in [${FROZEN_ROOTS.join(', ')}]:\n${output}`,
    );
  }
}

function resolveFrozenPluginBin(deskWebRoot) {
  const pkgRoot = resolve(
    deskWebRoot,
    'node_modules',
    '@bufbuild',
    'protoc-gen-es',
  );
  const pkgJsonPath = join(pkgRoot, 'package.json');
  if (!existsSync(pkgJsonPath)) {
    throw new Error(
      `Frozen plugin package not found at ${pkgJsonPath}. Run pnpm --dir apps/desk-web install --frozen-lockfile first.`,
    );
  }
  const pkg = JSON.parse(readFileSync(pkgJsonPath, 'utf8'));
  const binRel =
    typeof pkg.bin === 'string'
      ? pkg.bin
      : pkg.bin && typeof pkg.bin['protoc-gen-es'] === 'string'
        ? pkg.bin['protoc-gen-es']
        : null;
  if (!binRel) {
    throw new Error(`Could not resolve protoc-gen-es bin entry from ${pkgJsonPath}.`);
  }
  const resolvedBin = resolve(pkgRoot, binRel);
  const relToPkg = relative(pkgRoot, resolvedBin);
  if (relToPkg.startsWith('..') || isAbsolute(relToPkg)) {
    throw new Error(
      `Resolved protoc-gen-es bin (${resolvedBin}) escapes frozen package root (${pkgRoot}).`,
    );
  }
  if (!existsSync(resolvedBin)) {
    throw new Error(`Frozen protoc-gen-es binary does not exist at ${resolvedBin}.`);
  }
  return resolvedBin;
}

function parseWebTemplateOptions(templatePath) {
  if (!existsSync(templatePath)) {
    throw new Error(`Missing Web Buf template: ${templatePath}`);
  }
  const content = readFileSync(templatePath, 'utf8');
  if (/^\s*-?\s*remote\s*:/m.test(content)) {
    throw new Error(
      `Remote Buf plugins are forbidden in ${templatePath} (Strategy A requires local frozen plugin).`,
    );
  }
  if (!/^\s*version\s*:\s*v2\s*$/m.test(content)) {
    throw new Error(`Expected "version: v2" in ${templatePath}.`);
  }
  const outMatch = content.match(/^\s*out\s*:\s*(\S+)\s*$/m);
  if (!outMatch || outMatch[1] !== 'src/generated') {
    throw new Error(
      `Expected "out: src/generated" in ${templatePath}, got "${outMatch ? outMatch[1] : '<missing>'}".`,
    );
  }

  const opts = [];
  const lines = content.split(/\r?\n/);
  let inOpt = false;
  let optIndent = -1;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.length === 0 || trimmed.startsWith('#')) {
      continue;
    }
    const indent = line.search(/\S/);
    if (!inOpt) {
      if (/^opt\s*:\s*$/.test(trimmed)) {
        inOpt = true;
        optIndent = indent;
      }
      continue;
    }
    if (indent <= optIndent) {
      inOpt = false;
      continue;
    }
    const itemMatch = trimmed.match(/^-\s+(.+)$/);
    if (itemMatch && itemMatch[1]) {
      opts.push(itemMatch[1].trim().replace(/^['"]|['"]$/g, ''));
    }
  }
  if (opts.length === 0) {
    throw new Error(`No plugin opt entries parsed from ${templatePath}.`);
  }
  return opts;
}

function buildTempBufTemplate(pluginBinPath, outDirPath, opts) {
  const lines = [
    'version: v2',
    'clean: true',
    'plugins:',
    '  - local:',
    `      - ${JSON.stringify(process.execPath)}`,
    `      - ${JSON.stringify(pluginBinPath)}`,
    `    out: ${JSON.stringify(outDirPath)}`,
    '    opt:',
    ...opts.map((opt) => `      - ${JSON.stringify(opt)}`),
    '',
  ];
  return lines.join('\n');
}

function assertStrictlyInsideDir(parentDir, childPath, label) {
  const absParent = resolve(parentDir);
  const absChild = resolve(childPath);
  const rel = relative(absParent, absChild);
  if (rel.length === 0 || rel.startsWith('..') || isAbsolute(rel)) {
    throw new Error(
      `${label} (${absChild}) must be strictly inside ${absParent}.`,
    );
  }
}

function collectFilesRecursive(rootDir) {
  if (!existsSync(rootDir)) {
    return new Map();
  }
  const results = new Map();
  const walk = (dir) => {
    const entries = readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    );
    for (const entry of entries) {
      const fullPath = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(fullPath);
      } else if (entry.isFile()) {
        const relPosix = relative(rootDir, fullPath).split(sep).join('/');
        results.set(relPosix, fullPath);
      }
    }
  };
  walk(rootDir);
  return results;
}

function normalizeLf(buffer) {
  return buffer.toString('utf8').replace(/\r\n/g, '\n');
}

function generateIntoDirectory({ repoRoot, deskWebRoot, outDir, tempRoot }) {
  const templatePath = join(deskWebRoot, 'buf.gen.web.yaml');
  const opts = parseWebTemplateOptions(templatePath);
  const pluginBinPath = resolveFrozenPluginBin(deskWebRoot);
  const interfaceDir = resolve(repoRoot, 'interface');
  if (!existsSync(join(interfaceDir, 'buf.yaml'))) {
    throw new Error(`Missing read-only interface/buf.yaml at ${interfaceDir}.`);
  }

  const tempTemplatePath = join(tempRoot, 'buf.gen.web.resolved.yaml');
  writeFileSync(
    tempTemplatePath,
    buildTempBufTemplate(pluginBinPath, outDir, opts),
    'utf8',
  );

  const res = runBufCommand(
    ['generate', interfaceDir, '--template', tempTemplatePath],
    { cwd: tempRoot },
  );
  if (res.status !== 0) {
    throw new Error(
      `buf generate failed with exit ${String(res.status)}:\n${(res.stderr || res.stdout || '').trim()}`,
    );
  }
}

function verifyCodegen(repoRoot, deskWebRoot) {
  assertFrozenRootsClean(repoRoot, 'pre-generate');

  const tmpBase = tmpdir();
  const tempWorkDir = mkdtempSync(join(tmpBase, 'ficant-desk-web-codegen-'));
  const tempOutDir = join(tempWorkDir, 'generated');
  mkdirSync(tempOutDir, { recursive: true });

  try {
    assertStrictlyInsideDir(tmpBase, tempOutDir, 'Temp codegen output dir');
    generateIntoDirectory({
      repoRoot,
      deskWebRoot,
      outDir: tempOutDir,
      tempRoot: tempWorkDir,
    });

    const expectedFiles = collectFilesRecursive(tempOutDir);
    if (expectedFiles.size === 0) {
      throw new Error('Buf codegen produced an empty file set.');
    }

    const actualRoot = join(deskWebRoot, 'src', 'generated');
    const actualFiles = collectFilesRecursive(actualRoot);

    const missing = [];
    const extra = [];
    const modified = [];

    for (const [relPath, expectedPath] of expectedFiles.entries()) {
      const actualPath = actualFiles.get(relPath);
      if (!actualPath) {
        missing.push(relPath);
        continue;
      }
      const expectedText = normalizeLf(readFileSync(expectedPath));
      const actualText = normalizeLf(readFileSync(actualPath));
      if (expectedText !== actualText) {
        modified.push(relPath);
      }
    }

    for (const relPath of actualFiles.keys()) {
      if (!expectedFiles.has(relPath)) {
        extra.push(relPath);
      }
    }

    if (missing.length > 0 || extra.length > 0 || modified.length > 0) {
      const parts = [];
      if (missing.length > 0) {
        parts.push(`missing (${missing.length}): ${missing.join(', ')}`);
      }
      if (extra.length > 0) {
        parts.push(`extra (${extra.length}): ${extra.join(', ')}`);
      }
      if (modified.length > 0) {
        parts.push(`modified (${modified.length}): ${modified.join(', ')}`);
      }
      throw new Error(`Generated TypeScript drift detected — ${parts.join('; ')}`);
    }

    assertFrozenRootsClean(repoRoot, 'post-generate');
    console.log(
      `[verify-codegen] OK: ${expectedFiles.size} generated files match Strategy A local codegen; frozen roots clean.`,
    );
  } finally {
    rmSync(tempWorkDir, { recursive: true, force: true });
  }
}

function writeCodegen(repoRoot, deskWebRoot) {
  assertFrozenRootsClean(repoRoot, 'pre-write');

  const tmpBase = tmpdir();
  const tempWorkDir = mkdtempSync(join(tmpBase, 'ficant-desk-web-codegen-write-'));
  const targetOutDir = resolve(deskWebRoot, 'src', 'generated');

  try {
    assertStrictlyInsideDir(deskWebRoot, targetOutDir, 'Target codegen output dir');
    mkdirSync(targetOutDir, { recursive: true });
    generateIntoDirectory({
      repoRoot,
      deskWebRoot,
      outDir: targetOutDir,
      tempRoot: tempWorkDir,
    });

    // Normalize line endings to LF for deterministic cross-platform parity.
    const generatedFiles = collectFilesRecursive(targetOutDir);
    if (generatedFiles.size === 0) {
      throw new Error('Buf codegen --write produced an empty file set.');
    }
    for (const filePath of generatedFiles.values()) {
      const lf = normalizeLf(readFileSync(filePath));
      writeFileSync(filePath, lf, 'utf8');
    }

    assertFrozenRootsClean(repoRoot, 'post-write');
    console.log(
      `[verify-codegen] WROTE ${generatedFiles.size} files into apps/desk-web/src/generated; frozen roots clean.`,
    );
  } finally {
    rmSync(tempWorkDir, { recursive: true, force: true });
  }
}

function runGitInTemp(tmpRepo, args) {
  const res = runCommand(
    'git',
    [
      '-C',
      tmpRepo,
      '-c',
      'user.name=ficant-self-test',
      '-c',
      'user.email=self-test@ficant.local',
      ...args,
    ],
  );
  if (res.status !== 0) {
    throw new Error(
      `Temp git command "git ${args.join(' ')}" failed: ${(res.stderr || res.stdout || '').trim()}`,
    );
  }
}

function runSelfTest() {
  assertFrozenRootsClean(REPO_ROOT, 'self-test-pre');

  const tmpRepo = mkdtempSync(join(tmpdir(), 'ficant-codegen-selftest-'));
  try {
    // 1. Construct minimal isolated Git repository mirroring repo layout.
    writeFileSync(
      join(tmpRepo, '.gitignore'),
      'apps/desk-web/node_modules/\n',
      'utf8',
    );
    cpSync(join(REPO_ROOT, 'interface'), join(tmpRepo, 'interface'), {
      recursive: true,
    });
    mkdirSync(join(tmpRepo, 'crates', 'ficant-contracts'), { recursive: true });
    writeFileSync(
      join(tmpRepo, 'crates', 'ficant-contracts', '.gitkeep'),
      '',
      'utf8',
    );
    mkdirSync(join(tmpRepo, 'python', 'node-contracts'), { recursive: true });
    writeFileSync(
      join(tmpRepo, 'python', 'node-contracts', '.gitkeep'),
      '',
      'utf8',
    );

    const tmpDeskWeb = join(tmpRepo, 'apps', 'desk-web');
    mkdirSync(join(tmpDeskWeb, 'scripts'), { recursive: true });
    mkdirSync(join(tmpDeskWeb, 'src'), { recursive: true });

    cpSync(
      join(DESK_WEB_ROOT, 'buf.gen.web.yaml'),
      join(tmpDeskWeb, 'buf.gen.web.yaml'),
    );
    cpSync(SCRIPT_PATH, join(tmpDeskWeb, 'scripts', 'verify-codegen.mjs'));
    cpSync(
      join(DESK_WEB_ROOT, 'src', 'generated'),
      join(tmpDeskWeb, 'src', 'generated'),
      { recursive: true },
    );

    const realNodeModules = join(DESK_WEB_ROOT, 'node_modules');
    if (!existsSync(realNodeModules) || !statSync(realNodeModules).isDirectory()) {
      throw new Error(`Missing node_modules at ${realNodeModules}.`);
    }
    symlinkSync(
      realNodeModules,
      join(tmpDeskWeb, 'node_modules'),
      process.platform === 'win32' ? 'junction' : 'dir',
    );

    runGitInTemp(tmpRepo, ['init', '-b', 'main']);
    runGitInTemp(tmpRepo, ['add', '-A']);
    runGitInTemp(tmpRepo, ['commit', '-m', 'self-test baseline']);

    const runIsolated = () =>
      runCommand(
        process.execPath,
        [join(tmpDeskWeb, 'scripts', 'verify-codegen.mjs')],
        { cwd: tmpDeskWeb },
      );

    // Scenario 0: clean baseline must pass.
    const clean = runIsolated();
    if (clean.status !== 0) {
      throw new Error(
        `Self-test clean baseline failed (exit ${String(clean.status)}):\n${clean.stderr}\n${clean.stdout}`,
      );
    }

    const generatedMap = collectFilesRecursive(join(tmpDeskWeb, 'src', 'generated'));
    const firstEntry = generatedMap.entries().next().value;
    if (!firstEntry) {
      throw new Error('Self-test found no generated files in temp repo.');
    }
    const [sampleRelPath, sampleFullPath] = firstEntry;
    const sampleOriginal = readFileSync(sampleFullPath);

    // Scenario 1: extra untracked generated file must fail with "extra".
    const extraPath = join(tmpDeskWeb, 'src', 'generated', 'unexpected_extra_pb.ts');
    writeFileSync(extraPath, '// extra file\n', 'utf8');
    const extraRes = runIsolated();
    rmSync(extraPath, { force: true });
    if (extraRes.status === 0 || !/extra/i.test(extraRes.stderr)) {
      throw new Error(
        `Self-test scenario (extra file) did not fail as expected. exit=${String(extraRes.status)}, stderr=${extraRes.stderr}`,
      );
    }

    // Scenario 2: missing generated file must fail with "missing".
    rmSync(sampleFullPath, { force: true });
    const missingRes = runIsolated();
    writeFileSync(sampleFullPath, sampleOriginal);
    if (missingRes.status === 0 || !/missing/i.test(missingRes.stderr)) {
      throw new Error(
        `Self-test scenario (missing file: ${sampleRelPath}) did not fail as expected. exit=${String(missingRes.status)}, stderr=${missingRes.stderr}`,
      );
    }

    // Scenario 3: 1-byte modified generated file must fail with "modified".
    writeFileSync(sampleFullPath, Buffer.concat([sampleOriginal, Buffer.from('\n')]));
    const modifiedRes = runIsolated();
    writeFileSync(sampleFullPath, sampleOriginal);
    if (modifiedRes.status === 0 || !/modified/i.test(modifiedRes.stderr)) {
      throw new Error(
        `Self-test scenario (modified file: ${sampleRelPath}) did not fail as expected. exit=${String(modifiedRes.status)}, stderr=${modifiedRes.stderr}`,
      );
    }

    // Scenario 4: root interface/buf.gen.yaml drift detected when launched from apps/desk-web CWD.
    const tmpBufGenYaml = join(tmpRepo, 'interface', 'buf.gen.yaml');
    const originalBufGenYaml = readFileSync(tmpBufGenYaml);
    writeFileSync(
      tmpBufGenYaml,
      Buffer.concat([originalBufGenYaml, Buffer.from('# simulated drift\n')]),
    );
    const frozenDriftRes = runIsolated();
    writeFileSync(tmpBufGenYaml, originalBufGenYaml);
    if (
      frozenDriftRes.status === 0 ||
      !/frozen root drift/i.test(frozenDriftRes.stderr) ||
      !/interface\/buf\.gen\.yaml/i.test(frozenDriftRes.stderr)
    ) {
      throw new Error(
        `Self-test scenario (root interface/buf.gen.yaml drift from apps/desk-web CWD) did not fail as expected. exit=${String(frozenDriftRes.status)}, stderr=${frozenDriftRes.stderr}`,
      );
    }

    assertFrozenRootsClean(REPO_ROOT, 'self-test-post');
    console.log(
      '[verify-codegen] SELF-TEST PASS: clean=0, extra=fail, missing=fail, modified=fail, root-buf-gen-drift(from apps/desk-web)=fail; real repo untouched.',
    );
  } finally {
    rmSync(tmpRepo, { recursive: true, force: true });
  }
}

try {
  const args = process.argv.slice(2).filter((arg) => arg !== '--');
  if (args.includes('--self-test')) {
    runSelfTest();
  } else if (args.includes('--write')) {
    writeCodegen(REPO_ROOT, DESK_WEB_ROOT);
  } else if (args.length === 0) {
    verifyCodegen(REPO_ROOT, DESK_WEB_ROOT);
  } else {
    fail(`Unknown argument(s): ${args.join(' ')}. Supported: (none), --write, --self-test`);
  }
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}
