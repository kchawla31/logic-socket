import { javascript } from '@codemirror/lang-javascript';
import { editorTheme } from '../lib/editorTheme';
import { json } from '@codemirror/lang-json';
import { EditorView } from '@codemirror/view';
import CodeMirror from '@uiw/react-codemirror';
import { Plus, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import ReactMarkdown from 'react-markdown';

import type { KeyValue } from '../lib/api';
import { cn } from '../lib/utils';
import { VarInput } from './VarInput';

export function useDark(): boolean {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains('dark'));
  useEffect(() => {
    const o = new MutationObserver(() => setDark(document.documentElement.classList.contains('dark')));
    o.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
    return () => o.disconnect();
  }, []);
  return dark;
}

export function CodeEditor({
  value,
  onChange,
  language = 'json',
  readOnly,
  className,
  placeholder,
}: {
  value: string;
  onChange?: (v: string) => void;
  language?: 'json' | 'javascript' | 'text';
  readOnly?: boolean;
  className?: string;
  placeholder?: string;
}) {
  const extensions = useMemo(() => {
    const ext = [EditorView.lineWrapping];
    if (language === 'json') ext.push(json());
    if (language === 'javascript') ext.push(javascript());
    return ext;
  }, [language]);
  return (
    <div className={cn('min-h-0 flex-1 overflow-hidden', className)}>
      <CodeMirror
        value={value}
        onChange={onChange}
        readOnly={readOnly}
        editable={!readOnly}
        theme={editorTheme}
        extensions={extensions}
        placeholder={placeholder}
        height="100%"
        style={{ height: '100%' }}
        basicSetup={{ foldGutter: true, highlightActiveLine: !readOnly, lineNumbers: true, autocompletion: false }}
      />
    </div>
  );
}

/** Editable list of name/value rows with enable toggles; always keeps one empty row at the end. */
export function KeyValueEditor({
  items,
  onChange,
  contextId,
  namePlaceholder = 'name',
  valuePlaceholder = 'value',
  readOnlyNames,
}: {
  items: KeyValue[];
  onChange: (items: KeyValue[]) => void;
  contextId?: string;
  namePlaceholder?: string;
  valuePlaceholder?: string;
  readOnlyNames?: boolean;
}) {
  const rows = readOnlyNames ? items : [...items, { name: '', value: '' }];
  const update = (i: number, patch: Partial<KeyValue>) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    onChange(next.filter((r, j) => j < next.length - (readOnlyNames ? 0 : 1) || r.name || r.value));
  };
  const remove = (i: number) => onChange(items.filter((_, j) => j !== i));
  return (
    <div className="flex flex-col">
      {rows.map((r, i) => {
        const placeholderRow = !readOnlyNames && i === rows.length - 1;
        return (
          <div key={i} className={cn('group flex items-center gap-1.5 border-b border-app px-2 py-1', r.disabled && 'opacity-50')}>
            <input
              type="checkbox"
              aria-label="Enabled"
              className="accent-[var(--color-accent)]"
              checked={!r.disabled}
              disabled={placeholderRow}
              onChange={e => update(i, { disabled: !e.target.checked })}
            />
            {readOnlyNames ? (
              <div className="w-2/5 truncate px-2 font-mono text-[12.5px]">{r.name}</div>
            ) : (
              <VarInput
                className="w-2/5"
                value={r.name}
                placeholder={namePlaceholder}
                contextId={contextId}
                onChange={v => update(i, { name: v })}
                bare
              />
            )}
            <VarInput
              className="flex-1"
              value={r.value}
              placeholder={valuePlaceholder}
              contextId={contextId}
              onChange={v => update(i, { value: v })}
              bare
            />
            {!placeholderRow && !readOnlyNames && (
              <button
                aria-label="Remove"
                className="invisible rounded p-1 text-muted group-hover:visible hover:text-rose-500"
                onClick={() => remove(i)}
              >
                <Trash2 className="size-3.5" />
              </button>
            )}
            {placeholderRow && <Plus className="mx-1 size-3.5 text-muted opacity-50" />}
          </div>
        );
      })}
    </div>
  );
}

export function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    <div className={cn('md selectable leading-relaxed', className)}>
      <ReactMarkdown
        components={{
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

// ---- JSON tree viewer

function JsonNode({
  k,
  v,
  depth,
  path,
  onPath,
  defaultOpen,
}: {
  k: string | null;
  v: unknown;
  depth: number;
  path: string;
  onPath: (p: string) => void;
  defaultOpen: number;
}) {
  const isObj = v !== null && typeof v === 'object';
  const [open, setOpen] = useState(depth < defaultOpen);
  const [limit, setLimit] = useState(200);
  const key =
    k !== null ? (
      <button className="text-violet-700 hover:underline dark:text-violet-300" title={`Copy path ${path}`} onClick={() => onPath(path)}>
        {k}
      </button>
    ) : null;
  if (!isObj) {
    const cls =
      typeof v === 'string'
        ? 'text-emerald-700 dark:text-emerald-400'
        : typeof v === 'number'
          ? 'text-sky-700 dark:text-sky-400'
          : 'text-amber-700 dark:text-amber-400';
    return (
      <div className="pl-4 whitespace-pre-wrap break-all">
        {key}
        {key && <span className="text-muted">: </span>}
        <span className={cls}>{JSON.stringify(v)}</span>
      </div>
    );
  }
  const entries = Array.isArray(v) ? v.map((x, i) => [String(i), x] as const) : Object.entries(v as object);
  const [o, c] = Array.isArray(v) ? ['[', ']'] : ['{', '}'];
  return (
    <div className="pl-4">
      <button className="-ml-4 w-4 text-muted" aria-label={open ? 'Collapse' : 'Expand'} onClick={() => setOpen(!open)}>
        {open ? '▾' : '▸'}
      </button>
      {key}
      {key && <span className="text-muted">: </span>}
      <span className="text-muted">{o}</span>
      {!open && (
        <button className="text-muted hover:text-app" onClick={() => setOpen(true)}>
          {' '}
          {entries.length} {Array.isArray(v) ? 'items' : 'keys'} {c}
        </button>
      )}
      {open && (
        <>
          {entries.slice(0, limit).map(([ck, cv]) => (
            <JsonNode
              key={ck}
              k={ck}
              v={cv}
              depth={depth + 1}
              path={Array.isArray(v) ? `${path}[${ck}]` : /^[A-Za-z_$][\w$]*$/.test(ck) ? `${path}.${ck}` : `${path}["${ck}"]`}
              onPath={onPath}
              defaultOpen={defaultOpen}
            />
          ))}
          {entries.length > limit && (
            <button className="pl-4 text-accent" onClick={() => setLimit(limit + 500)}>
              Show {Math.min(500, entries.length - limit)} more of {entries.length - limit}…
            </button>
          )}
          <div className="text-muted">{c}</div>
        </>
      )}
    </div>
  );
}

export function JsonTree({ value, onPath, defaultOpen = 2 }: { value: unknown; onPath: (p: string) => void; defaultOpen?: number }) {
  return (
    <div className="selectable -ml-4 font-mono text-[12.5px] leading-[1.6]">
      <JsonNode k={null} v={value} depth={0} path="$" onPath={onPath} defaultOpen={defaultOpen} />
    </div>
  );
}
