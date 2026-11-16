import type { ReactNode } from 'react';
import { AlertTriangle, Loader2, X } from 'lucide-react';
import { severityBadge } from '../lib/api';

export function Card({ title, actions, children, className = '' }: { title?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`card ${className}`}>
      {(title || actions) && (
        <div className="flex items-center justify-between border-b border-edge px-4 py-2.5">
          <h2 className="text-[13px] font-semibold uppercase tracking-wider text-slate-300">{title}</h2>
          <div className="flex items-center gap-2">{actions}</div>
        </div>
      )}
      <div className="p-4">{children}</div>
    </section>
  );
}

export function Button({
  children,
  onClick,
  variant = 'default',
  disabled,
  busy,
  className = '',
  title,
  type = 'button',
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: 'default' | 'primary' | 'danger';
  disabled?: boolean;
  busy?: boolean;
  className?: string;
  title?: string;
  type?: 'button' | 'submit';
}) {
  const cls = variant === 'primary' ? 'btn btn-primary' : variant === 'danger' ? 'btn btn-danger' : 'btn';
  return (
    <button type={type} title={title} className={`${cls} ${className}`} onClick={onClick} disabled={disabled || busy}>
      {busy && <Loader2 size={13} className="animate-spin" />}
      {children}
    </button>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block">
      <span className="label">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-slate-500">{hint}</span>}
    </label>
  );
}

export function Select({ value, onChange, children, className = '' }: { value: string; onChange: (v: string) => void; children: ReactNode; className?: string }) {
  return (
    <select className={`input ${className}`} value={value} onChange={(e) => onChange(e.target.value)}>
      {children}
    </select>
  );
}

export function Badge({ children, tone = 'default' }: { children: ReactNode; tone?: 'default' | 'ok' | 'warn' | 'bad' }) {
  const map = {
    default: 'chip text-slate-300',
    ok: 'chip border-emerald-800 bg-emerald-950/50 text-emerald-300',
    warn: 'chip border-amber-800 bg-amber-950/40 text-amber-300',
    bad: 'chip border-rose-800 bg-rose-950/40 text-rose-300',
  };
  return <span className={map[tone]}>{children}</span>;
}

export function Severity({ value }: { value: string }) {
  return <span className={severityBadge(value)}>{value}</span>;
}

export function Empty({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-1 rounded-md border border-dashed border-edge px-6 py-10 text-center">
      <p className="text-sm font-medium text-slate-300">{title}</p>
      {hint && <p className="max-w-md text-xs text-slate-500">{hint}</p>}
    </div>
  );
}

export function ErrorBox({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <div className="flex items-start gap-2 rounded-md border border-rose-900 bg-rose-950/30 px-3 py-2 text-xs text-rose-300">
      <AlertTriangle size={14} className="mt-0.5 shrink-0" />
      <span className="whitespace-pre-wrap break-words">{message}</span>
    </div>
  );
}

export function Progress({ value, label }: { value: number; label?: string }) {
  return (
    <div>
      {label && <div className="mb-1 flex justify-between text-[11px] text-slate-400"><span>{label}</span><span>{value}%</span></div>}
      <div className="h-1.5 overflow-hidden rounded-full bg-slate-800">
        <div className="h-full rounded-full bg-cyan-600 transition-all" style={{ width: `${Math.max(0, Math.min(100, value))}%` }} />
      </div>
    </div>
  );
}

export function Modal({ title, children, onClose, wide }: { title: string; children: ReactNode; onClose: () => void; wide?: boolean }) {
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/70 p-6 overflow-auto" onMouseDown={onClose}>
      <div className={`card my-6 w-full ${wide ? 'max-w-4xl' : 'max-w-xl'}`} onMouseDown={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-slate-100">{title}</h2>
          <button className="text-slate-500 hover:text-slate-200" onClick={onClose} aria-label="Close">
            <X size={16} />
          </button>
        </div>
        <div className="max-h-[75vh] overflow-auto p-4">{children}</div>
      </div>
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-2 text-xs text-slate-400">
      <Loader2 size={14} className="animate-spin" /> {label ?? 'Working…'}
    </div>
  );
}

export function KeyValue({ items }: { items: Array<[string, ReactNode]> }) {
  return (
    <table className="table">
      <tbody>
        {items.map(([k, v]) => (
          <tr key={k}>
            <th className="w-52 bg-panel2/40">{k}</th>
            <td className="break-words">{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
