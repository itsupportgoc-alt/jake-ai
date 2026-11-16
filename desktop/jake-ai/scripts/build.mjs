import { build } from 'esbuild';
import { spawnSync } from 'node:child_process';
import { rmSync } from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
rmSync(path.join(root, 'dist-electron'), { recursive: true, force: true });

const common = {
  bundle: true,
  platform: 'node',
  target: 'node20',
  format: 'cjs',
  sourcemap: false,
  external: ['electron', 'sql.js', 'mysql2', 'playwright-core'],
  logLevel: 'info',
};

await build({ ...common, entryPoints: [path.join(root, 'electron/main.ts')], outfile: path.join(root, 'dist-electron/main.cjs') });
await build({ ...common, entryPoints: [path.join(root, 'electron/preload.ts')], outfile: path.join(root, 'dist-electron/preload.cjs') });

const vite = spawnSync(process.platform === 'win32' ? 'npx.cmd' : 'npx', ['vite', 'build'], { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' });
if (vite.status !== 0) process.exit(vite.status ?? 1);
console.log('Build complete: dist/ (renderer) + dist-electron/ (main, preload)');
