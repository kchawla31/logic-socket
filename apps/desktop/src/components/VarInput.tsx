import { type ClipboardEvent, type KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react';

import { api, onDbChanged, type VarInfo } from '../lib/api';
import { cn, tokenizeTemplate } from '../lib/utils';

// Context variables cached per context id; invalidated on any DB change.
const cache = new Map<string, Promise<VarInfo[]>>();
const listeners = new Set<() => void>();
let subscribed = false;
function subscribe(cb: () => void) {
  listeners.add(cb);
  if (!subscribed) {
    subscribed = true;
    onDbChanged(() => {
      cache.clear();
      listeners.forEach(l => l());
    }).catch(() => {});
  }
  return () => listeners.delete(cb);
}

export function useContextVars(contextId?: string): VarInfo[] {
  const [vars, setVars] = useState<VarInfo[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => subscribe(() => setTick(t => t + 1)) as unknown as () => void, []);
  useEffect(() => {
    if (!contextId) return;
    let p = cache.get(contextId);
    if (!p) {
      p = api.contextVars(contextId).catch(() => []);
      cache.set(contextId, p);
    }
    let live = true;
    p.then(v => live && setVars(v));
    return () => {
      live = false;
    };
  }, [contextId, tick]);
  return vars;
}

function preview(v: unknown): string {
  const s = typeof v === 'string' ? v : JSON.stringify(v);
  return s.length > 60 ? `${s.slice(0, 60)}…` : s;
}

/**
 * Single-line input that highlights `{{ variables }}` (green = defined,
 * red = undefined) and `{% tags %}`, and autocompletes variables after `{{`.
 */
export function VarInput({
  value,
  onChange,
  contextId,
  placeholder,
  className,
  bare,
  mono = true,
  onEnter,
  onPaste,
  autoFocus,
  inputRef,
}: {
  value: string;
  onChange: (v: string) => void;
  contextId?: string;
  placeholder?: string;
  className?: string;
  bare?: boolean;
  mono?: boolean;
  onEnter?: () => void;
  onPaste?: (e: ClipboardEvent<HTMLInputElement>) => void;
  autoFocus?: boolean;
  inputRef?: React.RefObject<HTMLInputElement | null>;
}) {
  const vars = useContextVars(contextId);
  const names = useMemo(() => new Set(vars.map(v => v.name)), [vars]);
  const localRef = useRef<HTMLInputElement>(null);
  const ref = inputRef ?? localRef;
  const overlay = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState<string | null>(null);
  const [sel, setSel] = useState(0);

  const segments = useMemo(() => tokenizeTemplate(value), [value]);
  const hasTemplate = segments.some(s => s.kind !== 'text');
  const matches = useMemo(() => {
    if (query === null) return [];
    const q = query.toLowerCase();
    return vars.filter(v => v.name.toLowerCase().includes(q)).slice(0, 8);
  }, [query, vars]);

  const detect = () => {
    const el = ref.current;
    if (!el || !contextId) return;
    const before = el.value.slice(0, el.selectionStart ?? el.value.length);
    const m = before.match(/\{\{\s*(?:_\.)?([\w.-]*)$/);
    setQuery(m ? m[1] : null);
    setSel(0);
  };

  const complete = (name: string) => {
    const el = ref.current!;
    const caret = el.selectionStart ?? value.length;
    const before = value.slice(0, caret).replace(/\{\{\s*(?:_\.)?[\w.-]*$/, `{{ _.${name} }}`);
    const after = value.slice(caret).replace(/^\s*\}\}/, '');
    onChange(before + after);
    setQuery(null);
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(before.length, before.length);
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (matches.length > 0 && query !== null) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSel(s => (s + 1) % matches.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSel(s => (s - 1 + matches.length) % matches.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        complete(matches[sel].name);
        return;
      }
      if (e.key === 'Escape') {
        setQuery(null);
        return;
      }
    }
    if (e.key === 'Enter' && onEnter && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      onEnter();
    }
  };

  const font = mono ? 'font-mono text-[12.5px]' : 'text-[13px]';
  return (
    <div
      className={cn(
        'relative h-8 min-w-0',
        !bare && 'rounded-md border border-app bg-app focus-within:border-accent',
        className,
      )}
    >
      {hasTemplate && (
        <div
          ref={overlay}
          aria-hidden
          className={cn('pointer-events-none absolute inset-0 flex items-center overflow-hidden px-2.5 whitespace-pre', font)}
        >
          <span className="leading-normal">
          {segments.map((s, i) =>
            s.kind === 'text' ? (
              <span key={i}>{s.text}</span>
            ) : (
              <span
                key={i}
                className={cn(
                  'rounded-[3px]',
                  s.kind === 'tag'
                    ? 'bg-violet-500/20 text-violet-700 dark:text-violet-300'
                    : names.has((s.name || '').split('.')[0]) || !contextId
                      ? 'bg-emerald-500/20 text-emerald-700 dark:text-emerald-300'
                      : 'bg-rose-500/20 text-rose-700 dark:text-rose-300',
                )}
              >
                {s.text}
              </span>
            ),
          )}
          </span>
        </div>
      )}
      <input
        ref={ref}
        value={value}
        autoFocus={autoFocus}
        spellCheck={false}
        placeholder={placeholder}
        onChange={e => {
          onChange(e.target.value);
          requestAnimationFrame(detect);
        }}
        onKeyDown={onKeyDown}
        onKeyUp={e => ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key) && detect()}
        onBlur={() => setTimeout(() => setQuery(null), 120)}
        onScroll={e => overlay.current && (overlay.current.scrollLeft = e.currentTarget.scrollLeft)}
        onPaste={onPaste}
        className={cn(
          'relative h-full w-full bg-transparent px-2.5 outline-none placeholder:text-muted',
          font,
          hasTemplate && 'text-transparent caret-[var(--fg)]',
        )}
      />
      {query !== null && matches.length > 0 && (
        <div
          role="listbox"
          className="absolute top-full left-0 z-40 mt-1 w-80 overflow-hidden rounded-lg border border-app bg-app py-1 shadow-xl"
        >
          {matches.map((m, i) => (
            <button
              key={m.name}
              role="option"
              aria-selected={i === sel}
              onMouseDown={e => {
                e.preventDefault();
                complete(m.name);
              }}
              className={cn('flex w-full items-center gap-2 px-2.5 py-1 text-left', i === sel && 'bg-accent-soft')}
            >
              <span className="font-mono text-[12.5px] text-emerald-700 dark:text-emerald-400">{m.name}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-muted">{preview(m.value)}</span>
              <span className="shrink-0 text-[10.5px] text-muted">{m.source}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
