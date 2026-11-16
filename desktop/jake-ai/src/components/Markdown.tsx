import { Fragment } from 'react';

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function inline(text: string): string {
  let h = escapeHtml(text);
  h = h.replace(/`([^`]+)`/g, '<code>$1</code>');
  h = h.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  h = h.replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>');
  h = h.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer noopener">$1</a>');
  return h;
}

/** Renders a safe subset of Markdown: everything is escaped before tags are added. */
export function Markdown({ text }: { text: string }) {
  const blocks: React.ReactNode[] = [];
  const lines = text.split(/\r?\n/);
  let code: string[] | null = null;
  let list: { ordered: boolean; items: string[] } | null = null;

  const flushList = () => {
    if (!list) return;
    const Tag = list.ordered ? 'ol' : 'ul';
    blocks.push(
      <Tag key={`l${blocks.length}`}>
        {list.items.map((it, i) => (
          <li key={i} dangerouslySetInnerHTML={{ __html: inline(it) }} />
        ))}
      </Tag>,
    );
    list = null;
  };

  for (const line of lines) {
    if (line.startsWith('```')) {
      if (code === null) {
        flushList();
        code = [];
      } else {
        blocks.push(
          <pre key={`c${blocks.length}`} className="code">
            <code>{code.join('\n')}</code>
          </pre>,
        );
        code = null;
      }
      continue;
    }
    if (code !== null) {
      code.push(line);
      continue;
    }
    if (/^\s*$/.test(line)) {
      flushList();
      continue;
    }
    const ol = /^\s*\d+\.\s+(.*)$/.exec(line);
    const ul = /^\s*[-*]\s+(.*)$/.exec(line);
    if (ol || ul) {
      const ordered = Boolean(ol);
      if (!list || list.ordered !== ordered) {
        flushList();
        list = { ordered, items: [] };
      }
      list.items.push((ol ?? ul)![1]);
      continue;
    }
    flushList();
    if (line.startsWith('### ')) blocks.push(<h3 key={blocks.length}>{line.slice(4)}</h3>);
    else if (line.startsWith('## ')) blocks.push(<h2 key={blocks.length}>{line.slice(3)}</h2>);
    else if (line.startsWith('# ')) blocks.push(<h1 key={blocks.length}>{line.slice(2)}</h1>);
    else if (line.startsWith('> ')) blocks.push(<blockquote key={blocks.length} dangerouslySetInnerHTML={{ __html: inline(line.slice(2)) }} />);
    else blocks.push(<p key={blocks.length} dangerouslySetInnerHTML={{ __html: inline(line) }} />);
  }
  flushList();
  if (code !== null) {
    blocks.push(
      <pre key={`c${blocks.length}`} className="code">
        <code>{code.join('\n')}</code>
      </pre>,
    );
  }
  return <div className="md text-[13.5px] text-slate-200">{blocks.map((b, i) => <Fragment key={i}>{b}</Fragment>)}</div>;
}
