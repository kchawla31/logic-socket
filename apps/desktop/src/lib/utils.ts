import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

import type { Auth, KeyValue } from './api';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'] as const;

export function methodColor(m?: string | null): string {
  switch ((m || '').toUpperCase()) {
    case 'GET':
      return 'text-emerald-600 dark:text-emerald-400';
    case 'POST':
      return 'text-amber-600 dark:text-amber-400';
    case 'PUT':
    case 'PATCH':
      return 'text-sky-600 dark:text-sky-400';
    case 'DELETE':
      return 'text-rose-600 dark:text-rose-400';
    default:
      return 'text-violet-600 dark:text-violet-400';
  }
}

export function methodShort(m?: string | null): string {
  const u = (m || 'GET').toUpperCase();
  return u === 'DELETE' ? 'DEL' : u === 'OPTIONS' ? 'OPT' : u.slice(0, 5);
}

export function statusTone(code: number): string {
  if (code === 0) return 'bg-rose-500/15 text-rose-600 dark:text-rose-400';
  if (code < 300) return 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400';
  if (code < 400) return 'bg-sky-500/15 text-sky-700 dark:text-sky-400';
  if (code < 500) return 'bg-amber-500/15 text-amber-700 dark:text-amber-400';
  return 'bg-rose-500/15 text-rose-700 dark:text-rose-400';
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function formatMs(ms: number): string {
  if (ms < 1) return `${ms.toFixed(2)} ms`;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

export function timeAgo(ts: number): string {
  const s = Math.round((Date.now() - ts) / 1000);
  if (s < 5) return 'just now';
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return new Date(ts).toLocaleDateString();
}

export function clockTime(ts: number): string {
  const d = new Date(ts);
  return `${d.toLocaleTimeString([], { hour12: false })}.${String(d.getMilliseconds()).padStart(3, '0')}`;
}

/** Split text into literal and `{{ var }}` / `{% tag %}` segments for highlighting. */
export interface Segment {
  text: string;
  kind: 'text' | 'var' | 'tag';
  name?: string;
}

export function tokenizeTemplate(text: string): Segment[] {
  const out: Segment[] = [];
  const re = /\{\{\s*([^}]*?)\s*\}\}|\{%\s*([^%]*?)\s*%\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push({ text: text.slice(last, m.index), kind: 'text' });
    if (m[1] !== undefined) {
      const name = m[1].replace(/^_\./, '').split('|')[0].trim();
      out.push({ text: m[0], kind: 'var', name });
    } else {
      out.push({ text: m[0], kind: 'tag', name: (m[2] || '').split(/\s/)[0] });
    }
    last = re.lastIndex;
  }
  if (last < text.length) out.push({ text: text.slice(last), kind: 'text' });
  return out;
}

/**
 * A pasted shell command, not a URL. Accepts `curl` / `curl.exe` in any case, a leading
 * prompt (`$`, `%`, `❯`, `user@host $`, `#`), a markdown fence, and a BOM. Requires a
 * space or quote after the token so typing "curl" or a `curl:` YAML key is left alone.
 */
export function looksLikeCurl(text: string): boolean {
  let t = text.replace(/^\uFEFF/, '').trim();
  const fenced = t.match(/^```[^\n]*\r?\n([\s\S]*?)\r?\n?```\s*$/);
  if (fenced) t = fenced[1].trim();
  return curlTokenAtStart(stripCurlPrompt(t));
}

function stripCurlPrompt(t: string): string {
  if (curlTokenAtStart(t)) return t;
  const lineEnd = t.search(/\r?\n/);
  const first = lineEnd < 0 ? t : t.slice(0, lineEnd);
  const idx = findCurlToken(first);
  if (idx >= 0) {
    const prefix = first.slice(0, idx);
    const hashed = /^#\s*$/.test(prefix);
    const prompted =
      prefix.trim().length > 0 &&
      prefix.trim().length <= 120 &&
      !prefix.includes('://') &&
      /[$%>❯➜λ»]/.test(prefix);
    if (hashed || prompted) return t.slice(idx).trimStart();
  }
  return t;
}

function findCurlToken(s: string): number {
  const match = /(^|[^A-Za-z0-9_./-])(curl(?:\.exe)?(?=[\s'"]))/i.exec(s);
  if (!match) return -1;
  return match.index + match[1].length;
}

function curlTokenAtStart(s: string): boolean {
  return /^\s*curl(?:\.exe)?(?=[\s'"])/i.test(s);
}

/** Path params (`/:id`) present in a URL, in order, unique. */
export function pathParamsInUrl(url: string): string[] {
  const out: string[] = [];
  const path = url.split(/[?#]/, 1)[0];
  for (const m of path.matchAll(/\/:([^/?#:]+)/g)) {
    if (!out.includes(m[1])) out.push(m[1]);
  }
  return out;
}

/** Split `?a=1&b=2` off a URL into params (for paste-a-full-URL UX). */
export function splitQuery(url: string): { base: string; params: { name: string; value: string }[] } {
  // The fragment is not part of the query; it stays on the base URL.
  const h = url.indexOf('#');
  const hash = h < 0 ? '' : url.slice(h);
  const noHash = h < 0 ? url : url.slice(0, h);
  const i = noHash.indexOf('?');
  if (i < 0 || noHash.includes('{{', i)) return { base: url, params: [] };
  const base = noHash.slice(0, i) + hash;
  const params = noHash
    .slice(i + 1)
    .split('&')
    .filter(Boolean)
    .map(p => {
      const [k, ...rest] = p.split('=');
      const dec = (s: string) => {
        try {
          return decodeURIComponent(s.replace(/\+/g, ' '));
        } catch {
          return s;
        }
      };
      return { name: dec(k), value: dec(rest.join('=')) };
    });
  return { base, params };
}

export function prettyJson(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

export function isMac(): boolean {
  return typeof navigator !== 'undefined' && /Mac/.test(navigator.platform || navigator.userAgent);
}

export function modKey(): string {
  return isMac() ? '⌘' : 'Ctrl';
}

// ---- bulk edit: key/value rows and auth settings as `name: value` lines

/** Split `name: value` (or `name=value`) on whichever separator comes first. */
function splitPair(line: string): [string, string] {
  const colon = line.indexOf(':');
  const eq = line.indexOf('=');
  const i = colon < 0 ? eq : eq < 0 ? colon : Math.min(colon, eq);
  return i < 0 ? [line.trim(), ''] : [line.slice(0, i).trim(), line.slice(i + 1).trim()];
}

/** Rows as `Name: value` lines; disabled rows start with `//` (Postman's bulk-edit format). */
export function kvToText(items: KeyValue[]): string {
  return items
    .filter(r => r.name || r.value)
    .map(r => `${r.disabled ? '// ' : ''}${r.name}: ${r.value}`)
    .join('\n');
}

/** Parse bulk-edit text back into rows, keeping ids and descriptions of rows with the same name. */
export function kvFromText(text: string, previous: KeyValue[] = []): KeyValue[] {
  const unused = [...previous];
  const rows: KeyValue[] = [];
  for (const raw of text.split(/\r?\n/)) {
    let line = raw.trim();
    if (!line) continue;
    const disabled = line.startsWith('//');
    if (disabled) line = line.slice(2).trim();
    const [name, value] = splitPair(line);
    if (!name && !value) continue;
    const i = unused.findIndex(p => p.name === name);
    const prev = i >= 0 ? unused.splice(i, 1)[0] : undefined;
    rows.push({
      ...(prev?.id ? { id: prev.id } : {}),
      ...(prev?.description ? { description: prev.description } : {}),
      name,
      value,
      ...(disabled ? { disabled: true } : {}),
    });
  }
  return rows;
}

/** Auth settings as `field: value` lines, `type` first. */
export function authToText(auth: Auth): string {
  const { type, ...rest } = auth as Auth & Record<string, unknown>;
  const lines = [`type: ${type}`];
  for (const [k, v] of Object.entries(rest)) lines.push(`${k}: ${v ?? ''}`);
  return lines.join('\n');
}

/**
 * Apply `field: value` lines to `current`. A `type` line switches to that type's defaults
 * (from `defaultsFor`) first. Booleans parse from `true`/`false`; unknown fields are returned
 * in `ignored` rather than stored.
 */
export function authFromText(
  text: string,
  current: Auth,
  defaultsFor: (t: Auth['type']) => Auth,
  types: readonly Auth['type'][],
): { auth: Auth; ignored: string[] } {
  const pairs = text
    .split(/\r?\n/)
    .map(l => l.trim())
    .filter(l => l && !l.startsWith('//'))
    .map(l => {
      const i = l.indexOf(':');
      return i < 0 ? [l, ''] : [l.slice(0, i).trim(), l.slice(i + 1).trim()];
    });
  const typeLine = pairs.find(([k]) => k === 'type')?.[1] as Auth['type'] | undefined;
  const base = (typeLine && types.includes(typeLine) && typeLine !== current.type ? defaultsFor(typeLine) : current) as Auth &
    Record<string, unknown>;
  const next: Record<string, unknown> = { ...base };
  const ignored: string[] = [];
  for (const [k, v] of pairs) {
    if (k === 'type') {
      if (!typeLine || !types.includes(typeLine)) ignored.push(k);
      continue;
    }
    const known = k in base || (k === 'disabled' && base.type !== 'inherit' && base.type !== 'none');
    if (!known) {
      ignored.push(k);
      continue;
    }
    if (typeof base[k] === 'boolean' || k === 'disabled') next[k] = v.toLowerCase() === 'true';
    else if (base[k] === null || (k === 'prefix' && base.type === 'bearer')) next[k] = v || null;
    else next[k] = v;
  }
  return { auth: next as Auth, ignored };
}
