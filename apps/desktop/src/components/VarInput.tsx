import { type ClipboardEvent, type KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import { api, onDbChanged, type VarInfo } from '../lib/api';
import { cn, tokenizeTemplate, variableDefined } from '../lib/utils';

// Context variables cached per context id; invalidated on any DB change.
const cache = new Map<string, Promise<VarInfo[]>>();
const listeners = new Set<() => void>();
let subscribed = false;
function subscribe(cb: () => void) {
  listeners.add(cb);
  if (!subscribed) {
    subscribed = true;
    onDbChanged(() => invalidateContextVars()).catch(() => {});
  }
  return () => {
    listeners.delete(cb);
  };
}

/** Drop cached variable lists and tell every open field to load them again. */
export function invalidateContextVars() {
  cache.clear();
  listeners.forEach(l => l());
}

/** Bumps whenever environments or other saved data change, so previews re-render. */
export function useVarRevision(): number {
  const [tick, setTick] = useState(0);
  useEffect(() => subscribe(() => setTick(t => t + 1)), []);
  return tick;
}

function sourceRank(source: string): number {
  if (source.startsWith('Folder:')) return 2;
  if (source.startsWith('Global:')) return 1;
  if (source === 'Base Environment' || !source) return 3;
  return 0;
}

export function useContextVars(contextId?: string): VarInfo[] {
  const [vars, setVars] = useState<VarInfo[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => subscribe(() => setTick(t => t + 1)), []);
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
 * Single-line input that highlights `${variables}` (green = defined,
 * red = undefined) and `{% tags %}`, and autocompletes variables after `${`.
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
  onBlur,
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
  onBlur?: () => void;
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
    return vars
      .filter(v => v.name.toLowerCase().includes(q))
      .sort((a, b) => {
        const as = a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const bs = b.name.toLowerCase().startsWith(q) ? 0 : 1;
        if (as !== bs) return as - bs;
        const ar = sourceRank(a.source);
        const br = sourceRank(b.source);
        if (ar !== br) return ar - br;
        return a.name.localeCompare(b.name);
      })
      .slice(0, 30);
  }, [query, vars]);
  const [box, setBox] = useState<{ top: number; left: number; width: number; above: boolean } | null>(null);
  const placeMenu = () => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const width = Math.min(360, Math.max(r.width, 280));
    const left = Math.min(r.left, window.innerWidth - width - 8);
    const spaceBelow = window.innerHeight - r.bottom;
    const above = spaceBelow < 200 && r.top > spaceBelow;
    setBox({ top: above ? r.top - 4 : r.bottom + 4, left: Math.max(8, left), width, above });
  };
  useEffect(() => {
    if (query === null || matches.length === 0) return;
    placeMenu();
    const on = () => placeMenu();
    window.addEventListener('scroll', on, true);
    window.addEventListener('resize', on);
    return () => {
      window.removeEventListener('scroll', on, true);
      window.removeEventListener('resize', on);
    };
  }, [query, matches.length, value]);

  const detect = () => {
    const el = ref.current;
    if (!el || !contextId) return;
    const before = el.value.slice(0, el.selectionStart ?? el.value.length);
    const m = before.match(/\$\{\s*([\w.-]*)$/);
    setQuery(m ? m[1] : null);
    setSel(0);
  };

  const complete = (name: string) => {
    const el = ref.current!;
    const caret = el.selectionStart ?? value.length;
    const before = value.slice(0, caret).replace(/\$\{\s*[\w.-]*$/, `\${${name}}`);
    const after = value.slice(caret).replace(/^\s*\}/, '');
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
                    : variableDefined(names, s.name) || !contextId
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
        onBlur={() => {
          onBlur?.();
          setTimeout(() => setQuery(null), 120);
        }}
        onScroll={e => overlay.current && (overlay.current.scrollLeft = e.currentTarget.scrollLeft)}
        onPaste={onPaste}
        className={cn(
          'relative h-full w-full bg-transparent px-2.5 outline-none placeholder:text-muted',
          font,
          hasTemplate && 'text-transparent caret-[var(--fg)]',
        )}
      />
      {query !== null && matches.length > 0 && box &&
        createPortal(
          <div
            role="listbox"
            style={{ top: box.top, left: box.left, width: box.width, transform: box.above ? 'translateY(-100%)' : undefined }}
            className="fixed z-50 max-h-64 overflow-auto rounded-lg border border-app bg-app py-1 shadow-xl"
          >
            {matches.map((m, i) => (
              <button
                key={`${m.source}:${m.name}`}
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
          </div>,
          // A modal dialog is a top layer, so a menu on document.body sits underneath it and cannot be clicked.
          ref.current?.closest('dialog') ?? document.body,
        )}
    </div>
  );
}
