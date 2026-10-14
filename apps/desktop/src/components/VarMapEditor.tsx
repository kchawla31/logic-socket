import { Eye, EyeOff, Lock, LockOpen, Plus, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';

import { errorText } from '../lib/api';
import { cn } from '../lib/utils';
import { CodeEditor } from './editors';
import { IconButton, Toggle, useDialog, useToast } from './ui';
import { VarInput } from './VarInput';

const VAULT_PREFIX = 'vault:v1:';

export function isSealedValue(v: unknown): boolean {
  return typeof v === 'string' && v.startsWith(VAULT_PREFIX);
}

/** What a row shows. Strings stay unquoted so `${var}` reads as a variable. */
export function formatVar(v: unknown): string {
  if (v === undefined) return '';
  if (typeof v === 'string') return isSealedValue(v) ? '' : v;
  return JSON.stringify(v);
}

/**
 * Value to store for a row. Typed strings stay strings. An existing object,
 * array, or number stays JSON while the text parses, and is kept as-is while
 * it does not. A masked secret keeps its stored value.
 */
export function commitValue(text: string, raw: unknown, masked: boolean): unknown {
  if (masked) return raw;
  if (typeof raw === 'string' || raw === undefined) return text;
  try {
    return JSON.parse(text);
  } catch {
    return raw;
  }
}

export function plainObject(data: Record<string, unknown>, secretKeys: string[]): Record<string, unknown> {
  const skip = new Set(secretKeys);
  return Object.fromEntries(Object.entries(data).filter(([k]) => !skip.has(k)));
}

/** Replace plain variables from JSON. Secret values stay, unless the JSON names the same key. */
export function applyJsonText(data: Record<string, unknown>, secretKeys: string[], text: string): Record<string, unknown> {
  const parsed = JSON.parse(text || '{}') as unknown;
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('must be a JSON object');
  const secrets = Object.fromEntries(secretKeys.filter(k => k in data).map(k => [k, data[k]]));
  return { ...secrets, ...(parsed as Record<string, unknown>) };
}

interface Row {
  name: string;
  text: string;
  secret: boolean;
  raw: unknown;
  masked: boolean;
}

function load(data: Record<string, unknown>, secretKeys: string[]): Row[] {
  const names = Object.keys(data);
  for (const k of secretKeys) if (!(k in data)) names.push(k);
  const secrets = new Set(secretKeys);
  return names.map(name => {
    const raw = data[name];
    const secret = secrets.has(name);
    const sealed = isSealedValue(raw);
    return { name, text: sealed ? '' : formatVar(raw), secret, raw, masked: secret && (sealed || raw === undefined) };
  });
}

export function buildVars(rows: Row[]): { data: Record<string, unknown>; secretKeys: string[] } {
  const data: Record<string, unknown> = {};
  const secretKeys: string[] = [];
  for (const r of rows) {
    if (!r.name) continue;
    const at = secretKeys.indexOf(r.name);
    if (r.secret && at < 0) secretKeys.push(r.name);
    if (!r.secret && at >= 0) secretKeys.splice(at, 1);
    const value = commitValue(r.text, r.raw, r.masked);
    if (value !== undefined) data[r.name] = value;
  }
  return { data, secretKeys };
}

function blank(): Row {
  return { name: '', text: '', secret: false, raw: undefined, masked: false };
}

function withBlank(rows: Row[]): Row[] {
  const last = rows[rows.length - 1];
  if (last && !last.name && !last.text && !last.secret) return rows;
  return [...rows, blank()];
}

function clean(rows: Row[]): Row[] {
  return rows.filter(r => r.name || r.text || r.secret);
}

function invalidJson(r: Row): boolean {
  if (r.masked || typeof r.raw === 'string' || r.raw === undefined) return false;
  try {
    JSON.parse(r.text);
    return false;
  } catch {
    return true;
  }
}

/**
 * Name/value editor for an environment or folder variable map.
 * Pass `onReveal` to allow locking a value as a secret. JSON edits objects and pasted maps.
 */
export function VarMapEditor({
  docId,
  data,
  secretKeys = [],
  contextId,
  hint,
  error,
  onChange,
  onReveal,
}: {
  docId: string;
  data: Record<string, unknown>;
  secretKeys?: string[];
  contextId?: string;
  hint: string;
  error?: string | null;
  onChange: (data: Record<string, unknown>, secretKeys: string[]) => void;
  onReveal?: (key: string) => Promise<string>;
}) {
  const dialog = useDialog();
  const toast = useToast();
  const [rows, setRows] = useState<Row[]>(() => load(data, secretKeys));
  const [jsonOn, setJsonOn] = useState(false);
  const [jsonText, setJsonText] = useState('');
  const [jsonErr, setJsonErr] = useState<string | null>(null);
  const [replacing, setReplacing] = useState<number | null>(null);

  useEffect(() => {
    setRows(load(data, secretKeys));
    setJsonOn(false);
    setJsonErr(null);
    setReplacing(null);
    // Only when the document changes. Row edits must not reload from props.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docId]);

  const emit = (next: Row[]) => {
    const cleaned = clean(next);
    setRows(cleaned);
    const built = buildVars(cleaned);
    onChange(built.data, built.secretKeys);
  };

  const update = (i: number, patch: Partial<Row>) => {
    emit(withBlank(rows).map((r, j) => (j === i ? { ...r, ...patch } : r)));
  };

  const remove = async (i: number) => {
    const r = withBlank(rows)[i];
    if (r.secret && !(await dialog.confirm(`Delete secret “${r.name}”?`, { danger: true, confirmLabel: 'Delete' }))) return;
    emit(withBlank(rows).filter((_, j) => j !== i));
  };

  const toggleSecret = async (i: number) => {
    const r = withBlank(rows)[i];
    if (!r.name) return;
    if (r.secret) {
      if (!(await dialog.confirm(`Store “${r.name}” unencrypted?`, { message: 'It will be visible in the editor, exports and Git.', confirmLabel: 'Make plain' }))) return;
      try {
        const plain = await readSecret(r);
        if (plain === undefined) throw new Error('could not read the secret');
        update(i, { secret: false, masked: false, text: plain, raw: plain });
      } catch (e) {
        toast(errorText(e), 'error');
      }
      return;
    }
    // A new row's raw is unset, and a masked row stores raw rather than text.
    const stored = commitValue(r.text, r.raw, false);
    update(i, { secret: true, masked: true, text: '', raw: stored });
  };

  const readSecret = async (r: Row): Promise<string | undefined> => {
    if (isSealedValue(r.raw)) return onReveal?.(r.name);
    if (typeof r.raw === 'string') return r.raw;
    if (r.raw === undefined) return r.text;
    return formatVar(r.raw);
  };

  const reveal = async (i: number) => {
    const r = withBlank(rows)[i];
    if (!r.masked) {
      setRows(withBlank(rows).map((row, j) => (j === i ? { ...row, masked: true, text: '' } : row)).filter(row => row.name || row.text || row.secret));
      return;
    }
    try {
      const plain = await readSecret(r);
      if (plain === undefined) throw new Error('could not read the secret');
      setRows(rows.map(row => (row.name === r.name ? { ...row, masked: false, text: plain, raw: plain } : row)));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  const showJson = (on: boolean) => {
    if (!on) {
      setJsonErr(null);
      setJsonOn(false);
      return;
    }
    const built = buildVars(rows);
    setJsonText(JSON.stringify(plainObject(built.data, built.secretKeys), null, 2));
    setJsonErr(null);
    setJsonOn(true);
  };

  const editJson = (text: string) => {
    setJsonText(text);
    try {
      const built = buildVars(rows);
      const next = applyJsonText(built.data, built.secretKeys, text);
      setJsonErr(null);
      setRows(load(next, built.secretKeys));
      onChange(next, built.secretKeys);
    } catch (e) {
      setJsonErr((e as Error).message);
    }
  };

  const visible = withBlank(rows);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-3 border-b border-app px-3 py-1.5 text-[12px] text-muted">
        <span className="min-w-0 flex-1">
          {hint}{' '}
          {jsonOn && secretKeys.length > 0 && 'Secrets stay encrypted and are left out of this JSON.'}
          {(jsonErr || error) && <span className="ml-2 text-rose-600 dark:text-rose-400">⚠ {jsonErr || error}</span>}
        </span>
        <Toggle checked={jsonOn} onChange={showJson} label="JSON" />
      </div>
      {jsonOn ? (
        <CodeEditor value={jsonText} onChange={editJson} contextId={contextId} />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {visible.map((r, i) => {
            const placeholder = i === visible.length - 1 && !r.name && !r.text && !r.secret;
            const bad = invalidJson(r);
            const showValue = !r.masked || replacing === i;
            return (
              <div key={i} className="group flex items-center gap-1.5 border-b border-app px-2 py-1">
                <input
                  value={r.name}
                  aria-label={r.name ? `Name for ${r.name}` : 'New variable name'}
                  placeholder="name"
                  spellCheck={false}
                  onChange={e => update(i, { name: e.target.value })}
                  className="h-8 w-2/5 min-w-0 bg-transparent px-2.5 font-mono text-[12.5px] outline-none placeholder:text-muted"
                />
                {showValue ? (
                  <VarInput
                    bare
                    autoFocus={replacing === i}
                    className={cn('min-w-0 flex-1', bad && 'rounded-md ring-1 ring-rose-500')}
                    value={r.text}
                    placeholder={replacing === i ? 'New value' : 'value'}
                    contextId={contextId}
                    onChange={v => update(i, { text: v, masked: false, raw: r.masked ? undefined : r.raw })}
                    onBlur={() => {
                      if (replacing === i && r.masked) setReplacing(null);
                    }}
                  />
                ) : (
                  <button
                    type="button"
                    className="h-8 min-w-0 flex-1 truncate px-2.5 text-left font-mono text-[12.5px] text-muted"
                    onClick={() => setReplacing(i)}
                  >
                    {r.raw === undefined ? '(not set)' : '••••••••'}
                  </button>
                )}
                {onReveal && (
                  <>
                    <IconButton
                      label={r.secret ? 'Make plain' : 'Make secret'}
                      className={r.secret ? 'visible text-amber-600' : undefined}
                      disabled={placeholder}
                      onClick={() => toggleSecret(i)}
                    >
                      {r.secret ? <Lock className="size-3.5" /> : <LockOpen className="size-3.5" />}
                    </IconButton>
                    {r.secret && (
                      <IconButton label={r.masked ? 'Reveal' : 'Hide'} onClick={() => reveal(i)}>
                        {r.masked ? <Eye className="size-3.5" /> : <EyeOff className="size-3.5" />}
                      </IconButton>
                    )}
                  </>
                )}
                {placeholder ? (
                  <Plus className="mx-1 size-3.5 text-muted opacity-50" />
                ) : (
                  <button
                    type="button"
                    aria-label={`Remove ${r.name || 'variable'}`}
                    className="invisible rounded p-1 text-muted group-hover:visible hover:text-rose-500"
                    onClick={() => remove(i)}
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
