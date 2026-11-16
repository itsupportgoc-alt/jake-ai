import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { chromium, type BrowserContext, type Page } from 'playwright-core';
import type { Database } from './db';
import { isoNow } from './db';
import type { Target } from '../../shared/types';
import { assertUrlInScope } from './scope';
import { redact } from './redact';

/**
 * Authorized System Access: a real headed browser session using the system
 * Edge/Chrome through playwright-core. The operator logs in manually with
 * credentials they supply outside of JAKE; JAKE never receives the password.
 * After login the page structure is captured for authorized inspection.
 */
export class SystemAccess {
  private context: BrowserContext | null = null;
  private page: Page | null = null;

  constructor(private db: Database, private userDataDir: string, private onStatus: (s: string) => void) {}

  async openLogin(target: Target, url: string): Promise<{ ok: true; url: string }> {
    const checked = await assertUrlInScope(target, url);
    await this.close();
    fs.mkdirSync(this.userDataDir, { recursive: true });
    const executablePath = findBrowser();
    if (!executablePath) throw new Error('Microsoft Edge or Google Chrome was not found. Install one to use authorized browser sessions.');
    this.context = await chromium.launchPersistentContext(this.userDataDir, {
      executablePath,
      headless: false,
      viewport: { width: 1280, height: 860 },
      args: ['--no-default-browser-check'],
    });
    this.page = this.context.pages()[0] ?? (await this.context.newPage());
    await this.page.goto(checked.toString(), { waitUntil: 'domcontentloaded', timeout: 30000 });
    this.onStatus(`Login window open: ${checked.toString()}`);
    this.context.on('close', () => {
      this.context = null;
      this.page = null;
      this.onStatus('Browser session closed.');
    });
    return { ok: true, url: this.page.url() };
  }

  isOpen(): boolean {
    return Boolean(this.context && this.page);
  }

  async capture(): Promise<{ id: string; url: string; title: string; summary: string; htmlPath: string }> {
    if (!this.page || !this.context) throw new Error('No authorized browser session is open. Open the login page first.');
    const url = this.page.url();
    const title = await this.page.title();
    let html = await this.page.content();
    // Remove password field values and any inline secrets before storing.
    html = html.replace(/(<input[^>]*type=["']?password["']?[^>]*?)\svalue=["'][^"']*["']/gi, '$1');
    html = redact(html).slice(0, 2_000_000);
    const summary = await this.summarize();
    const id = randomUUID();
    const dir = path.join(this.userDataDir, '..', 'captures');
    fs.mkdirSync(dir, { recursive: true });
    const htmlPath = path.join(dir, `capture-${id}.html`);
    fs.writeFileSync(htmlPath, html, 'utf8');
    this.db.run('INSERT INTO access_captures (id, url, title, summary, html_path, captured_at) VALUES (?,?,?,?,?,?)', [
      id,
      url,
      title.slice(0, 300),
      summary,
      htmlPath,
      isoNow(),
    ]);
    return { id, url, title, summary, htmlPath };
  }

  private async summarize(): Promise<string> {
    if (!this.page) return '';
    return this.page.evaluate(() => {
      const forms = Array.from(document.querySelectorAll('form'));
      const navLabels = Array.from(document.querySelectorAll('nav a, aside a, [role="navigation"] a'))
        .map((a) => (a.textContent ?? '').trim())
        .filter(Boolean)
        .slice(0, 30);
      const headings = Array.from(document.querySelectorAll('h1,h2'))
        .map((h) => (h.textContent ?? '').trim())
        .filter(Boolean)
        .slice(0, 15);
      const tableHeaders = Array.from(document.querySelectorAll('table thead th'))
        .map((th) => (th.textContent ?? '').trim())
        .filter(Boolean)
        .slice(0, 40);
      const buttons = Array.from(document.querySelectorAll('button'))
        .map((b) => (b.textContent ?? '').trim())
        .filter(Boolean)
        .slice(0, 30);
      const permissionWords = ['role', 'permission', 'user', 'account', 'admin', 'viewer', 'manager', 'officer', 'stockman', 'audit'];
      const bodyText = (document.body.innerText || '').toLowerCase();
      const mentioned = permissionWords.filter((w) => bodyText.includes(w));
      const passwordInputs = document.querySelectorAll('input[type="password"]').length;
      const currentUserHints = Array.from(document.querySelectorAll('[class*="user"],[id*="user"]'))
        .map((e) => (e.textContent ?? '').trim())
        .filter((t) => t && t.length < 80)
        .slice(0, 8);
      return [
        `URL: ${location.href}`,
        `Title: ${document.title}`,
        `Forms: ${forms.length} (password inputs: ${passwordInputs})`,
        `Headings: ${headings.join(' | ') || 'none'}`,
        `Navigation: ${navLabels.join(' | ') || 'none'}`,
        `Buttons: ${buttons.join(' | ') || 'none'}`,
        `Table headers: ${tableHeaders.join(' | ') || 'none'}`,
        `Permission-related terms on page: ${mentioned.join(', ') || 'none'}`,
        `Account hints: ${currentUserHints.join(' | ') || 'none'}`,
        `Links: ${document.querySelectorAll('a[href]').length}`,
      ].join('\n');
    });
  }

  async close(): Promise<void> {
    if (this.context) {
      try {
        await this.context.close();
      } catch {
        /* ignore */
      }
    }
    this.context = null;
    this.page = null;
  }
}

function findBrowser(): string | null {
  const candidates =
    process.platform === 'win32'
      ? [
          'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
          'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
          'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
        ]
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  return candidates.find((p) => fs.existsSync(p)) ?? null;
}
