import { Search } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';

import { cn } from '../lib/utils';

export interface Command {
  id: string;
  label: string;
  group: string;
  hint?: ReactNode;
  icon?: ReactNode;
  keywords?: string;
  run: () => void;
}

/** Subsequence fuzzy score; higher is better, -1 = no match. */
export function fuzzyScore(text: string, query: string): number {
  if (!query) return 0;
  const t = text.toLowerCase();
  const q = query.toLowerCase();
  const direct = t.indexOf(q);
  if (direct >= 0) return 1000 - direct * 2 - t.length * 0.1;
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const c of q) {
    const found = t.indexOf(c, ti);
    if (found < 0) return -1;
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 5 - (found - ti);
    ti = found + 1;
  }
  return score;
}

export function CommandPalette({ open, onClose, commands }: { open: boolean; onClose: () => void; commands: Command[] }) {
  const [q, setQ] = useState('');
  const [sel, setSel] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setQ('');
      setSel(0);
      setTimeout(() => input.current?.focus(), 0);
    }
  }, [open]);

  const results = useMemo(() => {
    const scored = commands
      .map(c => ({ c, s: Math.max(fuzzyScore(c.label, q), fuzzyScore(`${c.group} ${c.keywords ?? ''}`, q) - 200) }))
      .filter(x => x.s >= 0);
    if (q) scored.sort((a, b) => b.s - a.s);
    return scored.slice(0, 60).map(x => x.c);
  }, [commands, q]);

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [sel]);

  if (!open) return null;
  const run = (c?: Command) => {
    if (!c) return;
    onClose();
    c.run();
  };
  let lastGroup = '';
  return (
    <div className="fixed inset-0 z-50 flex justify-center bg-black/30 pt-[12vh] backdrop-blur-[1px]" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label="Command palette"
        className="flex h-fit max-h-[60vh] w-[620px] max-w-[92vw] flex-col overflow-hidden rounded-xl border border-app bg-app shadow-2xl"
        onMouseDown={e => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-app px-3">
          <Search className="size-4 text-muted" />
          <input
            ref={input}
            autoFocus
            value={q}
            onChange={e => (setQ(e.target.value), setSel(0))}
            onKeyDown={e => {
              if (e.key === 'Escape') onClose();
              if (e.key === 'ArrowDown') (e.preventDefault(), setSel(s => Math.min(s + 1, results.length - 1)));
              if (e.key === 'ArrowUp') (e.preventDefault(), setSel(s => Math.max(s - 1, 0)));
              if (e.key === 'Enter') run(results[sel]);
            }}
            placeholder="Type a command or search requests…"
            className="h-12 flex-1 bg-transparent text-[14px] outline-none"
          />
        </div>
        <div ref={listRef} role="listbox" className="min-h-0 overflow-auto py-1">
          {results.map((c, i) => {
            const header = c.group !== lastGroup && !q;
            lastGroup = c.group;
            return (
              <div key={c.id}>
                {header && <div className="px-3 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted uppercase">{c.group}</div>}
                <button
                  role="option"
                  aria-selected={i === sel}
                  onMouseEnter={() => setSel(i)}
                  onClick={() => run(c)}
                  className={cn('flex w-full items-center gap-2.5 px-3 py-2 text-left', i === sel && 'bg-accent-soft')}
                >
                  <span className="flex w-5 justify-center text-muted">{c.icon}</span>
                  <span className="min-w-0 flex-1 truncate">{c.label}</span>
                  {q && <span className="text-[11px] text-muted">{c.group}</span>}
                  {c.hint && <span className="text-[11px] text-muted">{c.hint}</span>}
                </button>
              </div>
            );
          })}
          {results.length === 0 && <div className="px-3 py-6 text-center text-muted">No results</div>}
        </div>
      </div>
    </div>
  );
}
