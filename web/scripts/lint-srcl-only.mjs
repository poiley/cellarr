#!/usr/bin/env node
// SRCL-only lint (docs/10-ui.md): every UI primitive under web/app must come from
// the vendored SRCL components (@components/*). The only allowed non-component
// imports are framework/data modules, the theme controller, the API client, and
// the app's own glue. Anything else — a second component library, a bespoke UI
// primitive, an ad-hoc design-system module — fails the build.
//
// Usage:
//   node scripts/lint-srcl-only.mjs              # lint web/app
//   node scripts/lint-srcl-only.mjs --self-test  # prove it flags a violation

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = join(__dirname, '..');
const APP_DIR = join(WEB_ROOT, 'app');

// Import specifiers that are allowed from inside web/app.
// Each predicate receives the raw module specifier (the string in `from '...'`).
const ALLOW = [
  // The vendored SRCL component layer — the ONLY source of UI primitives.
  (s) => s === '@components' || s.startsWith('@components/'),
  // SRCL support layers the components themselves depend on.
  (s) => s === '@common' || s.startsWith('@common/'),
  (s) => s === '@modules' || s.startsWith('@modules/'),
  (s) => s === '@root' || s.startsWith('@root/'),
  // Theme controller (the one allowed non-component glue) + API client + types.
  (s) => s === '@lib/theme' || s === '@lib/ThemeProvider',
  (s) => s === '@lib/api/client' || s === '@lib/api/types',
  // The app's own composition glue (shell, placeholders, route components).
  (s) => s === '@app' || s.startsWith('@app/'),
  (s) => s.startsWith('./') || s.startsWith('../'),
  // Framework / data / runtime — not UI primitives.
  (s) => s === 'react' || s.startsWith('react/'),
  (s) => s === 'react-dom' || s.startsWith('react-dom/'),
  (s) => s === 'next' || s.startsWith('next/'),
];

// CSS imports are fine (global stylesheets / co-located modules).
function isStyleImport(spec) {
  return spec.endsWith('.css') || spec.endsWith('.scss');
}

function isAllowed(spec) {
  if (isStyleImport(spec)) return true;
  return ALLOW.some((p) => p(spec));
}

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      out.push(...walk(full));
    } else if (/\.(tsx?|jsx?|mts|mjs)$/.test(entry)) {
      out.push(full);
    }
  }
  return out;
}

// Match `import ... from '...'` and bare `import '...'` and dynamic `import('...')`.
const IMPORT_RE =
  /(?:import\s[^'"]*from\s*|import\s*|import\s*\(\s*)['"]([^'"]+)['"]/g;

function lintFile(file) {
  const src = readFileSync(file, 'utf8');
  const violations = [];
  let m;
  while ((m = IMPORT_RE.exec(src)) !== null) {
    const spec = m[1];
    if (!isAllowed(spec)) {
      violations.push(spec);
    }
  }
  return violations;
}

function lintDir(dir, label) {
  const files = walk(dir);
  const problems = [];
  for (const file of files) {
    for (const spec of lintFile(file)) {
      problems.push({ file: relative(WEB_ROOT, file), spec });
    }
  }
  if (problems.length) {
    console.error(`\nSRCL-only lint FAILED for ${label}:`);
    for (const p of problems) {
      console.error(`  ${p.file}: disallowed import '${p.spec}'`);
    }
    console.error(
      `\nEvery UI primitive must come from @components/* (vendored SRCL).` +
        ` Allowed non-UI: react / next, @lib/theme, @lib/ThemeProvider, @lib/api/*, @app/*, relative, CSS.\n`
    );
  }
  return problems;
}

// --- self-test: prove the rule actually catches a violation -----------------
if (process.argv.includes('--self-test')) {
  const sample = `import Foo from '@some-other-ui/Button';\nimport Bar from 'react';\n`;
  const tmp = join(__dirname, '.srcl-lint-selftest.tsx');
  const { writeFileSync, rmSync } = await import('node:fs');
  writeFileSync(tmp, sample);
  const found = lintFile(tmp);
  rmSync(tmp);
  const caughtBad = found.includes('@some-other-ui/Button');
  const allowedReact = !found.includes('react');
  if (caughtBad && allowedReact) {
    console.log('self-test PASS: flagged @some-other-ui/Button, allowed react');
    process.exit(0);
  }
  console.error('self-test FAIL:', { found });
  process.exit(1);
}

const problems = lintDir(APP_DIR, 'web/app');
if (problems.length) process.exit(1);
console.log(`SRCL-only lint OK: web/app imports only vendored SRCL + allowed glue.`);
