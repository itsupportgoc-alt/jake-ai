import { build, context } from 'esbuild';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'vite';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
process.chdir(root);

const common = {
  bundle: true,
  platform: 'node',
  target: 'node20',
  format: 'cjs',
  sourcemap: true,
  external: ['electron', 'sql.js', 'mysql2', 'playwright-core'],
  logLevel: 'warning',
};

const mainCtx = await context({ ...common, entryPoints: ['electron/main.ts'], outfile: 'dist-electron/main.cjs' });
const preloadCtx = await context({ ...common, entryPoints: ['electron/preload.ts'], outfile: 'dist-electron/preload.cjs' });
await mainCtx.rebuild();
await preloadCtx.rebuild();
await mainCtx.watch();
await preloadCtx.watch();

const vite = await createServer({ configFile: 'vite.config.ts' });
await vite.listen();
const url = 'http://127.0.0.1:5199';
vite.printUrls();

const child = spawn(process.platform === 'win32' ? 'npx.cmd' : 'npx', ['electron', '.'], {
  cwd: root,
  stdio: 'inherit',
  shell: process.platform === 'win32',
  env: { ...process.env, VITE_DEV_SERVER_URL: url },
});

const shutdown = async () => {
  await mainCtx.dispose();
  await preloadCtx.dispose();
  await vite.close();
  if (!child.killed) child.kill();
  process.exit(0);
};
child.on('exit', shutdown);
process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);
