import { useEffect, useRef, useState } from 'react';
import {
  LayoutDashboard,
  MessageSquare,
  PlusCircle,
  Crosshair,
  Radar,
  Bug,
  Users,
  KeyRound,
  LogIn,
  FileCode2,
  Braces,
  FileText,
  Wrench,
  ScrollText,
  Settings as SettingsIcon,
  Shield,
  OctagonX,
  X,
  ShieldCheck,
  type LucideIcon,
} from 'lucide-react';
import { call, useEvent } from './lib/api';
import { Button } from './components/ui';
import Dashboard from './pages/Dashboard';
import Chat from './pages/Chat';
import NewAssessment from './pages/NewAssessment';
import Targets from './pages/Targets';
import Recon from './pages/Recon';
import Vulnerability from './pages/Vulnerability';
import Accounts from './pages/Accounts';
import Rbac from './pages/Rbac';
import Access from './pages/Access';
import PhpMysql from './pages/PhpMysql';
import ApiSecurity from './pages/ApiSecurity';
import FindingsPage from './pages/Findings';
import ReportsPage from './pages/Reports';
import ToolsPage from './pages/Tools';
import AuditLogPage from './pages/AuditLogs';
import SettingsPage from './pages/Settings';

export type PageKey =
  | 'dashboard'
  | 'chat'
  | 'assessment'
  | 'targets'
  | 'recon'
  | 'vulnerability'
  | 'accounts'
  | 'rbac'
  | 'access'
  | 'php'
  | 'api'
  | 'findings'
  | 'reports'
  | 'tools'
  | 'audit'
  | 'settings';

const NAV: Array<{ key: PageKey; label: string; icon: LucideIcon }> = [
  { key: 'dashboard', label: 'Dashboard', icon: LayoutDashboard },
  { key: 'chat', label: 'AI Assistant', icon: MessageSquare },
  { key: 'assessment', label: 'New Assessment', icon: PlusCircle },
  { key: 'targets', label: 'Target Management', icon: Crosshair },
  { key: 'recon', label: 'Reconnaissance', icon: Radar },
  { key: 'vulnerability', label: 'Vulnerability Scanner', icon: Bug },
  { key: 'accounts', label: 'Account Security', icon: Users },
  { key: 'rbac', label: 'RBAC Auditor', icon: KeyRound },
  { key: 'access', label: 'Authorized System Access', icon: LogIn },
  { key: 'php', label: 'PHP / MySQL Auditor', icon: FileCode2 },
  { key: 'api', label: 'API Security', icon: Braces },
  { key: 'findings', label: 'Findings', icon: Shield },
  { key: 'reports', label: 'Reports', icon: FileText },
  { key: 'tools', label: 'Tool Manager', icon: Wrench },
  { key: 'audit', label: 'Audit Logs', icon: ScrollText },
  { key: 'settings', label: 'Settings', icon: SettingsIcon },
];

interface Approval {
  id: string;
  level: number;
  title: string;
  detail: string;
}

interface Toast {
  id: number;
  text: string;
  tone: 'info' | 'error' | 'ok';
}

const LEVEL_LABEL: Record<number, string> = {
  1: 'Passive inspection',
  2: 'Safe active assessment',
  3: 'Authenticated testing',
  4: 'Administrative change',
  5: 'Destructive operation',
};

export default function App(): JSX.Element {
  const [page, setPage] = useState<PageKey>(() => (window.location.hash.replace('#', '') as PageKey) || 'dashboard');
  const [approval, setApproval] = useState<Approval | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [progress, setProgress] = useState<{ message: string; value: number } | null>(null);
  const progressTimer = useRef<number | null>(null);
  const toastId = useRef(0);

  const toast = (text: string, tone: Toast['tone'] = 'info') => {
    const id = ++toastId.current;
    setToasts((t) => [...t.slice(-3), { id, text, tone }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 6000);
  };

  useEvent((event) => {
    if (event.type === 'approval') {
      setApproval(event.payload as unknown as Approval);
    } else if (event.type === 'progress') {
      setProgress({ message: String(event.message ?? ''), value: Number(event.progress ?? 0) });
      if (progressTimer.current) window.clearTimeout(progressTimer.current);
      progressTimer.current = window.setTimeout(() => setProgress(null), 12000);
    } else if (event.type === 'stopped') {
      setProgress(null);
      setApproval(null);
      toast('Emergency stop engaged. All operations cancelled.', 'error');
    } else if (event.type === 'access') {
      toast(String(event.message ?? ''), 'info');
    } else if (event.type === 'info') {
      toast(String(event.message ?? ''), 'info');
    }
  });

  useEffect(() => {
    const onHash = () => setPage((window.location.hash.replace('#', '') as PageKey) || 'dashboard');
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const go = (key: PageKey) => {
    window.location.hash = key;
    setPage(key);
  };

  const respond = async (approved: boolean) => {
    if (!approval) return;
    try {
      await call('approval:respond', { id: approval.id, approved });
      toast(approved ? 'Approved.' : 'Denied.', approved ? 'ok' : 'info');
    } catch (e) {
      toast((e as Error).message, 'error');
    }
    setApproval(null);
  };

  const emergency = async () => {
    try {
      await call('emergency:stop');
    } catch (e) {
      toast((e as Error).message, 'error');
    }
  };

  const render = () => {
    switch (page) {
      case 'dashboard':
        return <Dashboard onNavigate={go} />;
      case 'chat':
        return <Chat />;
      case 'assessment':
        return <NewAssessment onNavigate={go} />;
      case 'targets':
        return <Targets onNavigate={go} />;
      case 'recon':
        return <Recon />;
      case 'vulnerability':
        return <Vulnerability />;
      case 'accounts':
        return <Accounts />;
      case 'rbac':
        return <Rbac />;
      case 'access':
        return <Access />;
      case 'php':
        return <PhpMysql />;
      case 'api':
        return <ApiSecurity />;
      case 'findings':
        return <FindingsPage />;
      case 'reports':
        return <ReportsPage />;
      case 'tools':
        return <ToolsPage />;
      case 'audit':
        return <AuditLogPage />;
      case 'settings':
        return <SettingsPage />;
      default:
        return <Dashboard onNavigate={go} />;
    }
  };

  return (
    <div className="flex h-full min-h-screen bg-base">
      <aside className="flex w-[236px] shrink-0 flex-col border-r border-edge bg-[#0a1017]">
        <div className="border-b border-edge px-4 py-4">
          <div className="flex items-center gap-2">
            <div className="flex h-8 w-8 items-center justify-center rounded-md border border-cyan-800 bg-cyan-950/50">
              <ShieldCheck size={17} className="text-cyan-300" />
            </div>
            <div>
              <div className="text-[15px] font-bold tracking-wide text-slate-100">JAKE AI</div>
              <div className="text-[9.5px] uppercase tracking-[0.14em] text-cyan-600">Security Agent</div>
            </div>
          </div>
        </div>
        <nav className="flex-1 overflow-auto px-2 py-3">
          {NAV.map(({ key, label, icon: Icon }) => (
            <button
              key={key}
              onClick={() => go(key)}
              className={`mb-0.5 flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-[12.5px] transition-colors ${
                page === key ? 'bg-cyan-950/40 text-cyan-200' : 'text-slate-400 hover:bg-slate-800/50 hover:text-slate-200'
              }`}
            >
              <Icon size={15} className={page === key ? 'text-cyan-400' : 'text-slate-500'} />
              <span className="truncate">{label}</span>
            </button>
          ))}
        </nav>
        <div className="border-t border-edge px-4 py-3 text-[10px] leading-relaxed text-slate-600">
          Authorized testing only.
          <br />
          Every action is audited.
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center justify-between border-b border-edge bg-panel/70 px-5">
          <div className="flex items-baseline gap-3">
            <h1 className="text-[15px] font-semibold text-slate-100">{NAV.find((n) => n.key === page)?.label}</h1>
            <span className="text-[11px] text-slate-500">Authorized website penetration testing &amp; account security</span>
          </div>
          <div className="flex items-center gap-2">
            {progress && (
              <div className="hidden w-64 items-center gap-2 md:flex">
                <div className="h-1 flex-1 overflow-hidden rounded bg-slate-800">
                  <div className="h-full bg-cyan-600 transition-all" style={{ width: `${progress.value}%` }} />
                </div>
                <span className="truncate text-[10px] text-slate-500" title={progress.message}>
                  {progress.message}
                </span>
              </div>
            )}
            <Button variant="danger" className="btn-sm" onClick={() => void emergency()} title="Cancel every running operation (Ctrl+Shift+X)">
              <OctagonX size={13} /> Emergency Stop
            </Button>
          </div>
        </header>
        <main className="min-h-0 flex-1 overflow-auto p-5">{render()}</main>
      </div>

      {approval && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 p-6">
          <div className="card w-full max-w-2xl border-amber-800">
            <div className="flex items-start justify-between border-b border-edge px-4 py-3">
              <div>
                <div className="text-[10px] font-bold uppercase tracking-widest text-amber-500">Level {approval.level} · {LEVEL_LABEL[approval.level] ?? 'Permission required'}</div>
                <h2 className="mt-0.5 text-sm font-semibold text-slate-100">{approval.title}</h2>
              </div>
              <button className="text-slate-500 hover:text-slate-200" onClick={() => void respond(false)}>
                <X size={16} />
              </button>
            </div>
            <div className="space-y-4 p-4">
              <pre className="code max-h-72 whitespace-pre-wrap text-[11.5px]">{approval.detail}</pre>
              <div className="rounded border border-amber-900/60 bg-amber-950/20 px-3 py-2 text-[11px] text-amber-200/90">
                Approving authorizes this specific action against your registered target. It is recorded in the audit log with the decision.
                {approval.level >= 4 && ' This action modifies a system.'}
              </div>
              <div className="flex justify-end gap-2">
                <Button onClick={() => void respond(false)}>Deny</Button>
                <Button variant="primary" onClick={() => void respond(true)}>
                  Approve level {approval.level}
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      <div className="fixed bottom-4 right-4 z-[70] flex w-80 flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`rounded-md border px-3 py-2 text-xs shadow-lg ${
              t.tone === 'error' ? 'border-rose-800 bg-rose-950/95 text-rose-200' : t.tone === 'ok' ? 'border-emerald-800 bg-emerald-950/95 text-emerald-200' : 'border-edge bg-panel2/95 text-slate-200'
            }`}
          >
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
