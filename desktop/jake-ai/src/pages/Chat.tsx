import { useEffect, useRef, useState } from 'react';
import { Bot, Loader2, Plus, Send, Square, Trash2, User, Wrench } from 'lucide-react';
import { call, fmtDate, useEvent, useLoad, type SettingsRow } from '../lib/api';
import { Badge, Button, ErrorBox } from '../components/ui';
import { Markdown } from '../components/Markdown';
import { getProvider } from '../../shared/providers';

interface Conversation {
  id: string;
  title: string;
  updated_at: string;
}

interface Msg {
  id: number;
  role: string;
  content: string;
}

const SUGGESTIONS = [
  'Assess the security of my authorized website.',
  'Check my PHP application for SQL injection vulnerabilities.',
  'Review the administrator and viewer permissions.',
  'Inspect my system login security.',
  'Check whether a Viewer account can access restricted pages.',
  'Analyze my API security.',
  'Review my PHP authentication code.',
  'Generate a complete security report.',
];

export default function Chat() {
  const conversations = useLoad(() => call<Conversation[]>('chat:list'), []);
  const settings = useLoad(() => call<SettingsRow>('settings:get'), []);
  const [cid, setCid] = useState<string | null>(null);
  const cidRef = useRef<string | null>(null);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tools, setTools] = useState<Array<{ tool: string; status: string; summary?: string }>>([]);
  const [streamingId, setStreamingId] = useState<number | null>(null);
  const endRef = useRef<HTMLDivElement | null>(null);
  const provider = getProvider(settings.data?.aiProvider);

  useEffect(() => {
    cidRef.current = cid;
    setError(null);
    if (!cid) {
      setMessages([]);
      return;
    }
    call<Msg[]>('chat:history', { conversationId: cid })
      .then(setMessages)
      .catch((e) => setError((e as Error).message));
  }, [cid]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, tools]);

  useEvent((event) => {
    if (event.conversationId !== cidRef.current) return;
    if (event.type === 'chat:delta') {
      const text = String(event.text ?? '');
      setMessages((m) => {
        const copy = [...m];
        const last = copy[copy.length - 1];
        if (last && last.role === 'assistant' && last.id < 0) copy[copy.length - 1] = { ...last, content: last.content + text };
        else copy.push({ id: -1, role: 'assistant', content: text });
        return copy;
      });
    } else if (event.type === 'chat:tool') {
      setTools((t) => {
        const next = [...t];
        const idx = next.findIndex((x) => x.tool === event.tool && x.status === 'started');
        if (event.status === 'started') next.push({ tool: String(event.tool), status: 'started' });
        else if (idx >= 0) next[idx] = { tool: String(event.tool), status: 'finished', summary: String(event.summary ?? '') };
        return next;
      });
    } else if (event.type === 'chat:error') {
      setError(String(event.message ?? 'AI error'));
      setBusy(false);
      setStreamingId(null);
    }
  });

  const newConversation = async () => {
    try {
      const res = await call<{ id: string }>('chat:new');
      conversations.reload();
      setCid(res.id);
      setMessages([]);
      setTools([]);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const deleteConversation = async (id: string) => {
    if (!window.confirm('Delete this conversation?')) return;
    await call('chat:delete', { conversationId: id });
    if (cid === id) setCid(null);
    conversations.reload();
  };

  const send = async (text: string) => {
    const clean = text.trim();
    if (!clean || busy) return;
    setError(null);
    setTools([]);
    setInput('');
    setBusy(true);
    let id = cid;
    try {
      if (!id) {
        const created = await call<{ id: string }>('chat:new');
        id = created.id;
        cidRef.current = id;
        setCid(id);
        conversations.reload();
      }
      setMessages((m) => [...m, { id: Date.now(), role: 'user', content: clean }, { id: -1, role: 'assistant', content: '' }]);
      setStreamingId(-1);
      await call('chat:send', { conversationId: id, text: clean });
      const history = await call<Msg[]>('chat:history', { conversationId: id });
      setMessages(history);
      conversations.reload();
    } catch (e) {
      setError((e as Error).message);
      setMessages((m) => m.filter((x) => x.id !== -1));
    } finally {
      setBusy(false);
      setStreamingId(null);
    }
  };

  const stop = async () => {
    await call('chat:stop');
    setBusy(false);
  };

  return (
    <div className="flex h-[calc(100vh-120px)] gap-4">
      <aside className="flex w-56 shrink-0 flex-col rounded-lg border border-edge bg-panel">
        <div className="flex items-center justify-between border-b border-edge px-3 py-2">
          <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">Conversations</span>
          <button className="rounded p-1 text-cyan-400 hover:bg-slate-800" onClick={() => void newConversation()} title="New conversation">
            <Plus size={14} />
          </button>
        </div>
        <div className="flex-1 overflow-auto p-1.5">
          {(conversations.data ?? []).map((c) => (
            <div key={c.id} className={`group mb-0.5 flex items-center gap-1 rounded px-2 py-1.5 ${cid === c.id ? 'bg-slate-800' : 'hover:bg-slate-800/50'}`}>
              <button className="min-w-0 flex-1 text-left" onClick={() => setCid(c.id)}>
                <div className="truncate text-[12px] text-slate-300">{c.title}</div>
                <div className="text-[9.5px] text-slate-500">{fmtDate(c.updated_at)}</div>
              </button>
              <button className="hidden text-slate-500 hover:text-rose-400 group-hover:block" onClick={() => void deleteConversation(c.id)}>
                <Trash2 size={12} />
              </button>
            </div>
          ))}
          {(conversations.data ?? []).length === 0 && <p className="px-2 py-4 text-[11px] text-slate-500">No conversations yet.</p>}
        </div>
        <div className="border-t border-edge px-3 py-2 text-[10px] leading-relaxed text-slate-600">
          {provider.local
            ? `${provider.label.split(' (')[0]} runs on this PC — prompts stay local. Credentials are still redacted.`
            : `${provider.label.split(' (')[0]} is an external cloud AI. Do not paste confidential production data or credentials.`}
        </div>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col rounded-lg border border-edge bg-panel">
        <div className="flex items-center justify-between border-b border-edge px-4 py-2">
          <div className="flex items-center gap-2 text-[12px] text-slate-300">
            <Bot size={14} className="text-cyan-400" /> JAKE
            {settings.data?.requiresKey && !settings.data.keyConfigured && <Badge tone="warn">{provider.label.split(' (')[0]} · API key needed</Badge>}
            {(!settings.data?.requiresKey || settings.data?.keyConfigured) && <Badge tone="ok">{provider.label.split(' (')[0]} · {settings.data?.aiModel ?? 'not configured'}</Badge>}
            {!settings.data && <Badge tone="warn">AI not configured</Badge>}
          </div>
          {busy && (
            <Button className="btn-sm" onClick={() => void stop()}>
              <Square size={12} /> Stop
            </Button>
          )}
        </div>

        <div className="flex-1 overflow-auto px-4 py-4">
          {messages.length === 0 && (
            <div className="mx-auto max-w-2xl py-8">
              <div className="mb-4 flex items-start gap-3">
                <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-cyan-800 bg-cyan-950/40">
                  <Bot size={18} className="text-cyan-300" />
                </div>
                <div>
                  <p className="text-[13.5px] leading-relaxed text-slate-300">
                    Hello! I'm JAKE, your AI cybersecurity assistant. I can help assess website security, inspect application accounts, identify vulnerabilities, and improve your system's protection.
                  </p>
                  <p className="mt-2 text-[11.5px] text-slate-500">
                    Start by registering an authorized target. I never scan anything outside your registered scope, and I will ask before any active testing.
                  </p>
                </div>
              </div>
              <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                {SUGGESTIONS.map((s) => (
                  <button key={s} className="rounded-md border border-edge bg-panel2 px-3 py-2 text-left text-[12px] text-slate-400 hover:border-slate-600 hover:text-slate-200" onClick={() => void send(s)}>
                    {s}
                  </button>
                ))}
              </div>
            </div>
          )}

          <div className="mx-auto max-w-3xl space-y-4">
            {messages.map((m, i) => {
              if (m.role === 'tool') {
                return (
                  <div key={`${m.id}-${i}`} className="flex items-start gap-2 rounded border border-edge/60 bg-[#0b1219] px-3 py-2">
                    <Wrench size={12} className="mt-0.5 text-slate-500" />
                    <pre className="max-h-32 flex-1 overflow-auto whitespace-pre-wrap font-mono text-[10.5px] text-slate-500">{m.content.slice(0, 1200)}</pre>
                  </div>
                );
              }
              const isUser = m.role === 'user';
              return (
                <div key={`${m.id}-${i}`} className={`flex gap-3 ${isUser ? 'justify-end' : ''}`}>
                  {!isUser && (
                    <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded border border-cyan-900 bg-cyan-950/40">
                      <Bot size={14} className="text-cyan-300" />
                    </div>
                  )}
                  <div className={`max-w-[88%] rounded-lg px-3.5 py-2.5 ${isUser ? 'bg-cyan-900/25 text-slate-100' : 'bg-panel2/70'}`}>
                    {isUser ? <p className="whitespace-pre-wrap text-[13px]">{m.content}</p> : m.content ? <Markdown text={m.content} /> : <Loader2 size={14} className="animate-spin text-cyan-400" />}
                  </div>
                  {isUser && (
                    <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded border border-edge bg-panel2">
                      <User size={14} className="text-slate-400" />
                    </div>
                  )}
                </div>
              );
            })}
            {streamingId !== null && tools.length > 0 && (
              <div className="ml-10 flex flex-wrap gap-1.5">
                {tools.map((t, i) => (
                  <span key={i} className="chip border-cyan-900 text-cyan-300">
                    {t.status === 'started' ? <Loader2 size={10} className="animate-spin" /> : null} {t.tool}
                  </span>
                ))}
              </div>
            )}
            <div ref={endRef} />
          </div>
        </div>

        <div className="border-t border-edge p-3">
          <ErrorBox message={error} />
          <div className="mt-1.5 flex items-end gap-2">
            <textarea
              className="input h-[58px] resize-none"
              placeholder="Ask JAKE to assess a target, review accounts, analyze PHP code, or generate a report…  (Enter to send, Shift+Enter for a new line)"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  void send(input);
                }
              }}
            />
            <Button variant="primary" disabled={busy || !input.trim()} onClick={() => void send(input)}>
              {busy ? <Loader2 size={14} className="animate-spin" /> : <Send size={14} />}
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}
