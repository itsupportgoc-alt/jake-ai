import { useEffect, useRef, useState } from 'react';

export interface Envelope<T> {
  ok: boolean;
  data?: T;
  error?: string;
}

declare global {
  interface Window {
    jake: {
      invoke: (channel: string, payload?: unknown) => Promise<Envelope<unknown>>;
      onEvent: (cb: (event: Record<string, unknown>) => void) => () => void;
    };
  }
}

export async function call<T>(channel: string, payload: Record<string, unknown> = {}): Promise<T> {
  const res = await window.jake.invoke(channel, payload);
  if (!res.ok) throw new Error(res.error ?? 'Unknown error');
  return res.data as T;
}

export function useEvent(handler: (event: Record<string, unknown>) => void): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => window.jake.onEvent((e) => ref.current(e)), []);
}

export function useLoad<T>(loader: () => Promise<T>, deps: unknown[] = []): { data: T | null; error: string | null; loading: boolean; reload: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    loader()
      .then((d) => {
        if (!cancelled) {
          setData(d);
          setError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError((e as Error).message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, loading, reload: () => setTick((t) => t + 1) };
}

export interface TargetRow {
  id: string;
  label: string;
  url: string;
  kind: string;
  authorization_note: string;
  scope_patterns: string;
  rate_limit: number;
  allow_private: number;
  created_at: string;
}

export interface AssessmentRow {
  id: string;
  target_id: string | null;
  target_label: string;
  type: string;
  status: string;
  progress: number;
  summary: string | null;
  error: string | null;
  started_at: string;
  finished_at: string | null;
}

export interface FindingRow {
  id: string;
  assessment_id: string;
  target: string;
  name: string;
  category: string;
  severity: 'critical' | 'high' | 'medium' | 'low' | 'info';
  confidence: string;
  endpoint: string;
  evidence: string;
  description: string;
  impact: string;
  reproduction: string;
  remediation: string;
  references: string;
  status: string;
  created_at: string;
}

export interface ConnectionRow {
  id: string;
  name: string;
  kind: 'rest' | 'mysql' | 'database';
  base_url: string | null;
  username: string | null;
  has_secret: number;
  created_at: string;
  config?: Record<string, unknown>;
}

export interface SettingsRow {
  aiProvider: string;
  aiModel: string;
  temperature: number;
  maxTokens: number;
  aiBaseUrl: string;
  keyConfigured: boolean;
  requiresKey: boolean;
  secureStorage: boolean;
  defaultRateLimit: number;
  requestTimeoutMs: number;
  autoApproveLevel: number;
  reportsDir: string;
}

export interface ToolRow {
  id: string;
  name: string;
  kind: string;
  installed: boolean;
  version: string | null;
  location: string | null;
  via: string;
  notes: string;
}

export interface AuditRow {
  id: number;
  actor: string;
  action: string;
  target: string | null;
  level: number;
  detail: string | null;
  created_at: string;
}

export interface ReportRow {
  id: string;
  assessment_id: string;
  format: string;
  path: string;
  created_at: string;
}

export interface CapturesRow {
  id: string;
  url: string;
  title: string;
  summary: string;
  htmlPath: string;
  captured_at: string;
}

export function fmtDate(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleString();
}

export const SEVERITY_STYLE: Record<string, string> = {
  critical: 'bg-rose-950 text-rose-300 border-rose-800',
  high: 'bg-orange-950 text-orange-300 border-orange-800',
  medium: 'bg-amber-950 text-amber-300 border-amber-800',
  low: 'bg-cyan-950 text-cyan-300 border-cyan-800',
  info: 'bg-slate-800 text-slate-300 border-slate-600',
};

export function severityBadge(s: string): string {
  return `inline-flex items-center rounded border px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wide ${SEVERITY_STYLE[s] ?? SEVERITY_STYLE.info}`;
}

export function statusStyle(status: string): string {
  const map: Record<string, string> = {
    complete: 'text-emerald-400',
    running: 'text-cyan-400',
    failed: 'text-rose-400',
    stopped: 'text-amber-400',
    pending: 'text-slate-400',
  };
  return map[status] ?? 'text-slate-400';
}
