import { app, BrowserWindow, Menu, session, shell } from 'electron';
import path from 'node:path';
import { Database } from './services/db';
import { AppContext } from './services/context';
import { ChatAgent } from './services/agent';
import { registerIpc } from './ipc';

const DEV_URL = process.env.VITE_DEV_SERVER_URL;
let win: BrowserWindow | null = null;
let ctx: AppContext | null = null;
let db: Database | null = null;

function isAllowedSender(url: string): boolean {
  if (DEV_URL && url.startsWith(DEV_URL)) return true;
  return url.startsWith('file://') && url.includes('/dist/');
}

function hardenSession(): void {
  const ses = session.defaultSession;
  ses.setPermissionRequestHandler((_wc, _permission, callback) => callback(false));
  ses.setPermissionCheckHandler(() => false);
  ses.webRequest.onHeadersReceived((details, callback) => {
    const csp = DEV_URL
      ? "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://127.0.0.1:5199; worker-src 'self' blob:;"
      : "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none';";
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [csp],
        'X-Content-Type-Options': ['nosniff'],
      },
    });
  });
}

function createWindow(): void {
  win = new BrowserWindow({
    width: 1480,
    height: 940,
    minWidth: 1080,
    minHeight: 680,
    backgroundColor: '#0b0f14',
    title: 'JAKE AI',
    show: false,
    autoHideMenuBar: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      spellcheck: false,
      devTools: Boolean(DEV_URL),
    },
  });

  win.once('ready-to-show', () => win?.show());
  win.webContents.on('did-finish-load', async () => {
    try {
      const info = (await win?.webContents.executeJavaScript(
        '({ title: document.title, nodes: document.getElementById("root")?.childElementCount ?? 0, hash: location.hash })',
      )) as { title: string; nodes: number; hash: string };
      console.log(`[JAKE] Renderer loaded: ${JSON.stringify(info)}`);
    } catch (err) {
      console.error('[JAKE] Renderer self-check failed:', (err as Error).message);
    }
  });
  win.webContents.on('console-message', (_event, level, message) => {
    if (level >= 2) console.error(`[renderer:${level}] ${message}`);
  });
  win.webContents.on('render-process-gone', (_event, details) => {
    console.error('[JAKE] Renderer process gone:', details.reason);
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (url.startsWith('https://')) void shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (event, url) => {
    const allowed = DEV_URL ? url.startsWith(DEV_URL) : url.startsWith('file://');
    if (!allowed) {
      event.preventDefault();
      if (url.startsWith('https://')) void shell.openExternal(url);
    }
  });
  win.webContents.on('will-attach-webview', (event) => event.preventDefault());

  if (DEV_URL) void win.loadURL(DEV_URL);
  else void win.loadFile(path.join(__dirname, '..', 'dist', 'index.html'));

  win.on('closed', () => {
    win = null;
  });
}

function buildMenu(): void {
  const template: Electron.MenuItemConstructorOptions[] = [
    {
      label: 'File',
      submenu: [{ role: 'quit' }],
    },
    {
      label: 'Security',
      submenu: [
        {
          label: 'Emergency Stop',
          accelerator: 'CommandOrControl+Shift+X',
          click: () => ctx?.stopAll(),
        },
        {
          label: 'Stop AI response',
          click: () => ctx?.stopAll(),
        },
      ],
    },
    {
      label: 'View',
      submenu: [{ role: 'reload', enabled: Boolean(DEV_URL) }, { role: 'toggleDevTools', enabled: Boolean(DEV_URL) }, { type: 'separator' }, { role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }],
    },
    {
      label: 'Help',
      submenu: [
        {
          label: 'About JAKE AI',
          click: () => ctx?.emit({ type: 'info', message: `JAKE AI ${app.getVersion()} — authorized security assessment platform.` }),
        },
      ],
    },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

// A single writer protects the SQLite file from concurrent instances.
const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (win) {
      if (win.isMinimized()) win.restore();
      win.focus();
    }
  });
}

app.whenReady().then(async () => {
  app.setAppUserModelId('ai.jake.desktop');
  hardenSession();
  // JAKE_DATA_DIR allows an isolated data directory (portable use and automated tests).
  const dataDir = process.env.JAKE_DATA_DIR ? path.resolve(process.env.JAKE_DATA_DIR) : app.getPath('userData');
  db = await Database.open(path.join(dataDir, 'jake-ai.sqlite'));
  console.log(`[JAKE] Database ready at ${path.join(dataDir, 'jake-ai.sqlite')}`);
  ctx = new AppContext(db, dataDir);
  const agent = new ChatAgent(ctx);
  registerIpc(ctx, agent, () => win, isAllowedSender);
  ctx.emitter.on('event', (event) => {
    if (win && !win.isDestroyed()) win.webContents.send('jake:event', event);
  });
  buildMenu();
  createWindow();

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  ctx?.stopAll();
  if (process.platform !== 'darwin') app.quit();
});

app.on('before-quit', () => {
  try {
    ctx?.access.close();
    db?.close();
  } catch (err) {
    console.error('Failed to persist database on quit', err);
  }
});

process.on('uncaughtException', (err) => {
  console.error('Uncaught exception in main process:', err);
});
