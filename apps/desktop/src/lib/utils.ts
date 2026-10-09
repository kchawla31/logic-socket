import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

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
