import { useState } from 'react';
import { ChevronDown, ChevronRight, ExternalLink } from 'lucide-react';
import { severityBadge, fmtDate, type FindingRow } from '../lib/api';

export function FindingsView({ findings, onUpdate }: { findings: FindingRow[]; onUpdate?: (id: string, status: string) => void }) {
  const [open, setOpen] = useState<string | null>(null);
  if (findings.length === 0) {
    return <p className="py-6 text-center text-xs text-slate-500">No findings recorded for this assessment.</p>;
  }
  return (
    <div className="divide-y divide-edge/60">
      {findings.map((f) => {
        const refs: string[] = (() => {
          try {
            const v = JSON.parse(f.references || '[]');
            return Array.isArray(v) ? v : [];
          } catch {
            return [];
          }
        })();
        const expanded = open === f.id;
        return (
          <div key={f.id} className="py-2">
            <div className="flex items-start gap-2">
              <button className="mt-0.5 text-slate-500 hover:text-slate-200" onClick={() => setOpen(expanded ? null : f.id)}>
                {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </button>
              <div className="min-w-0 flex-1">
                <button className="block w-full text-left" onClick={() => setOpen(expanded ? null : f.id)}>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className={severityBadge(f.severity)}>{f.severity}</span>
                    <span className="text-[12.5px] font-medium text-slate-200">{f.name}</span>
                    <span className="chip">{f.category}</span>
                    <span className="text-[10px] text-slate-500">confidence: {f.confidence}</span>
                  </div>
                  <div className="mt-0.5 truncate font-mono text-[10.5px] text-slate-500" title={f.endpoint}>
                    {f.endpoint}
                  </div>
                </button>
                {expanded && (
                  <div className="mt-2 space-y-2 rounded border border-edge bg-[#0b1219] p-3 text-[11.5px] leading-relaxed">
                    <Block label="Description" text={f.description} />
                    <Block label="Evidence" text={f.evidence} mono />
                    <Block label="Impact" text={f.impact} />
                    <Block label="Safe reproduction" text={f.reproduction} mono />
                    <Block label="Remediation" text={f.remediation} />
                    {refs.length > 0 && (
                      <div>
                        <div className="mb-0.5 text-[10px] font-semibold uppercase tracking-wider text-slate-500">References</div>
                        {refs.map((r) => (
                          <a key={r} className="flex items-center gap-1 break-all text-[11px] text-cyan-400 hover:underline" href={r} target="_blank" rel="noreferrer">
                            <ExternalLink size={10} /> {r}
                          </a>
                        ))}
                      </div>
                    )}
                    <div className="flex items-center justify-between border-t border-edge pt-2 text-[10px] text-slate-500">
                      <span>Recorded {fmtDate(f.created_at)}</span>
                      {onUpdate ? (
                        <select className="rounded border border-edge bg-panel2 px-1.5 py-0.5 text-[10px] text-slate-300" value={f.status} onChange={(e) => onUpdate(f.id, e.target.value)}>
                          {['open', 'confirmed', 'fixed', 'accepted', 'false_positive'].map((s) => (
                            <option key={s} value={s}>
                              {s.replace('_', ' ')}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="capitalize">{f.status}</span>
                      )}
                    </div>
                  </div>
                )}
              </div>
              {onUpdate && !expanded && (
                <select className="rounded border border-edge bg-panel2 px-1.5 py-0.5 text-[10px] text-slate-400" value={f.status} onChange={(e) => onUpdate(f.id, e.target.value)}>
                  {['open', 'confirmed', 'fixed', 'accepted', 'false_positive'].map((s) => (
                    <option key={s} value={s}>
                      {s.replace('_', ' ')}
                    </option>
                  ))}
                </select>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function Block({ label, text, mono }: { label: string; text: string; mono?: boolean }) {
  return (
    <div>
      <div className="mb-0.5 text-[10px] font-semibold uppercase tracking-wider text-slate-500">{label}</div>
      <pre className={`whitespace-pre-wrap break-words ${mono ? 'font-mono text-[10.5px] text-slate-400' : 'font-sans text-slate-300'}`}>{text || '—'}</pre>
    </div>
  );
}
