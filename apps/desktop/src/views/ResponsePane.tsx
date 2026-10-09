import { AlertTriangle, CheckCircle2, CircleSlash, History, Send, Trash2, XCircle } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';

import { CodeEditor, JsonTree } from '../components/editors';
import { CopyButton, Empty, IconButton, Kbd, Select, Tabs, useToast } from '../components/ui';
import { api, type ResponseSummary, type ResponseView } from '../lib/api';
import { cn, formatBytes, formatMs, modKey, prettyJson, statusTone, timeAgo } from '../lib/utils';

export function ResponsePane({
  requestId,
  response,
  sending,
  onSelect,
}: {
  requestId: string;
  response: ResponseView | null;
  sending: boolean;
  onSelect: (r: ResponseView | null) => void;
}) {
  const toast = useToast();
  const [tab, setTab] = useState('preview');
  const [history, setHistory] = useState<ResponseSummary[]>([]);

  useEffect(() => {
    api.responseList(requestId).then(setHistory).catch(() => setHistory([]));
  }, [requestId, response?.id]);

  const json = useMemo(() => {
    if (!response?.bodyText) return undefined;
    if (!/json|\+json/.test(response.contentType) && !/^\s*[[{]/.test(response.bodyText)) return undefined;
    try {
      return JSON.parse(response.bodyText) as unknown;
    } catch {
      return undefined;
    }
  }, [response]);

  if (sending && !response) {
    return (
      <div className="flex h-full items-center justify-center gap-2 text-muted">
        <span className="size-2 animate-ping rounded-full bg-accent" /> Sending…
      </div>
    );
  }
  if (!response) {
    return (
      <Empty icon={<Send className="size-8" />} title="No response yet">
        Press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd> or click Send. Responses are kept in history for this request.
      </Empty>
    );
  }

  const headerCount = response.headers.length;
  const tests = response.testResults ?? [];
  const failedTests = tests.filter(t => !t.passed && !t.skipped).length;
  const consoleLines = response.console ?? [];
  const cookies = response.headers.filter(h => h.name.toLowerCase() === 'set-cookie');

  return (
    <div className={cn('flex h-full min-h-0 flex-col', sending && 'opacity-60')}>
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-app px-3">
        {response.error ? (
          <span className={cn('shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-semibold', statusTone(0))}>Error</span>
        ) : (
          <span className={cn('shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-semibold', statusTone(response.statusCode))}>
            {response.statusCode} {response.statusMessage}
          </span>
        )}
        {!response.error && (
          <>
            <span className="shrink-0 whitespace-nowrap text-[12px] text-muted" title="Total time">
              {formatMs(response.timings.totalMs)}
            </span>
            <span className="shrink-0 whitespace-nowrap text-[12px] text-muted">{formatBytes(response.bytes)}</span>
          </>
        )}
        {tests.length > 0 && (
          <button
            onClick={() => setTab('tests')}
            className={cn(
              'shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-medium',
              failedTests ? 'bg-rose-500/15 text-rose-700 dark:text-rose-400' : 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400',
            )}
          >
            {failedTests ? `${failedTests}/${tests.length} tests failed` : `${tests.length} tests passed`}
          </button>
        )}
        <div className="flex-1" />
        <History className="size-3.5 shrink-0 text-muted" />
        <Select
          aria-label="Response history"
          className="h-7 min-w-0 max-w-56 text-[12px]"
          value={response.id}
          onChange={e => api.responseGet(e.target.value).then(onSelect)}
        >
          {history.map(h => (
            <option key={h.id} value={h.id}>
              {h.error ? 'Error' : h.statusCode} · {formatMs(h.totalMs)} · {timeAgo(h.created)}
            </option>
          ))}
        </Select>
        <IconButton
          label="Clear history"
          onClick={async () => {
            await api.responseClear(requestId);
            onSelect(null);
            toast('History cleared');
          }}
        >
          <Trash2 className="size-3.5" />
        </IconButton>
      </div>

      {response.error ? (
        <div className="m-4 rounded-lg border border-rose-500/30 bg-rose-500/5 p-4">
          <div className="mb-1 flex items-center gap-2 font-semibold text-rose-700 dark:text-rose-400">
            <AlertTriangle className="size-4" /> Request failed
          </div>
          <div className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{response.error}</div>
          <div className="mt-3 text-[12.5px] text-muted">
            {response.error.includes('unresolved variable')
              ? 'Define the variable in an environment (⌘E) or a parent folder, or fix the name.'
              : response.error.includes('timed out')
                ? 'Increase the timeout in Settings or check that the server is reachable.'
                : 'Check the URL, network connection and TLS settings. The Timeline tab shows what was sent.'}
          </div>
        </div>
      ) : null}

      {response.scriptError && !response.error && (
        <div className="mx-3 mt-3 flex items-start gap-2 rounded-lg border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-[12.5px]">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div>
            <div className="font-semibold">After-response script failed</div>
            <div className="selectable font-mono">{response.scriptError}</div>
          </div>
        </div>
      )}
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'preview', label: 'Preview' },
          { id: 'raw', label: 'Raw' },
          { id: 'headers', label: 'Headers', count: headerCount },
          { id: 'cookies', label: 'Cookies', count: cookies.length },
          { id: 'tests', label: 'Tests', count: tests.length },
          { id: 'console', label: 'Console', count: consoleLines.length },
          { id: 'timeline', label: 'Timeline' },
          { id: 'timing', label: 'Timing' },
        ]}
        right={tab === 'raw' || tab === 'preview' ? <CopyButton text={response.bodyText} label="Copy body" /> : undefined}
      />
      <div className="min-h-0 flex-1 overflow-auto">
        {tab === 'preview' &&
          (response.bodyImage ? (
            <div className="p-4">
              <img alt="Response" className="max-w-full" src={`data:${response.contentType};base64,${response.bodyImage}`} />
            </div>
          ) : json !== undefined ? (
            <div className="p-3">
              <JsonTree
                value={json}
                onPath={p => {
                  navigator.clipboard?.writeText(p);
                  toast(`Copied ${p}`);
                }}
              />
            </div>
          ) : response.bodyText ? (
            <pre className="selectable p-3 font-mono text-[12.5px] whitespace-pre-wrap">{response.bodyText}</pre>
          ) : (
            <Empty title="Empty body" />
          ))}
        {tab === 'raw' && (
          <div className="flex h-full flex-col">
            <CodeEditor
              value={(json !== undefined && prettyJson(response.bodyText)) || response.bodyText}
              language={json !== undefined ? 'json' : 'text'}
              readOnly
            />
            {response.bodyTruncated && <div className="border-t border-app p-2 text-[12px] text-muted">Body truncated to 5 MB for display.</div>}
          </div>
        )}
        {tab === 'headers' && (
          <table className="selectable w-full text-[12.5px]">
            <tbody>
              {response.headers.map((h, i) => (
                <tr key={i} className="border-b border-app align-top">
                  <td className="w-1/3 px-3 py-1.5 font-mono font-medium text-violet-700 dark:text-violet-300">{h.name}</td>
                  <td className="px-3 py-1.5 font-mono break-all">{h.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {tab === 'cookies' &&
          (cookies.length ? (
            <div className="selectable divide-y divide-[var(--border)] font-mono text-[12.5px]">
              {cookies.map((c, i) => (
                <div key={i} className="px-3 py-2 break-all">
                  {c.value}
                </div>
              ))}
            </div>
          ) : (
            <Empty title="No cookies set by this response" />
          ))}
        {tab === 'timeline' && (
          <div className="selectable p-3 font-mono text-[12px] leading-[1.6]">
            {response.timeline.map((t, i) => (
              <div
                key={i}
                className={cn(
                  'break-all whitespace-pre-wrap',
                  t.kind === 'header-out' && 'text-sky-700 dark:text-sky-400',
                  t.kind === 'header-in' && 'text-emerald-700 dark:text-emerald-400',
                  t.kind === 'info' && 'text-muted',
                  t.kind === 'data-out' && 'border-l-2 border-app pl-2 text-muted',
                )}
              >
                {t.kind === 'header-out' ? '> ' : t.kind === 'header-in' ? '< ' : t.kind === 'info' ? '* ' : ''}
                {t.text}
              </div>
            ))}
          </div>
        )}
        {tab === 'timing' && <TimingChart response={response} />}
        {tab === 'tests' && <TestsView response={response} />}
        {tab === 'console' && <ConsoleView response={response} />}
      </div>
    </div>
  );
}

function TimingChart({ response }: { response: ResponseView }) {
  const t = response.timings;
  const total = Math.max(t.totalMs, 0.001);
  const wait = Math.max(0, t.totalMs - t.ttfbMs - t.downloadMs);
  const rows = [
    { label: 'Client preparation', ms: wait, color: 'bg-zinc-400' },
    { label: 'Connect, TLS & server wait (TTFB)', ms: t.ttfbMs, color: 'bg-accent' },
    { label: 'Content download', ms: t.downloadMs, color: 'bg-emerald-500' },
  ];
  let offset = 0;
  return (
    <div className="flex max-w-3xl flex-col gap-3 p-4">
      {rows.map(r => {
        const left = (offset / total) * 100;
        offset += r.ms;
        return (
          <div key={r.label} className="grid grid-cols-[220px_1fr_80px] items-center gap-3 text-[12.5px]">
            <span className="text-muted">{r.label}</span>
            <div className="relative h-3 rounded bg-muted">
              <div className={cn('absolute h-3 rounded', r.color)} style={{ left: `${left}%`, width: `${Math.max(0.5, (r.ms / total) * 100)}%` }} />
            </div>
            <span className="text-right font-mono">{formatMs(r.ms)}</span>
          </div>
        );
      })}
      <div className="grid grid-cols-[220px_1fr_80px] gap-3 border-t border-app pt-3 text-[12.5px] font-semibold">
        <span>Total</span>
        <span />
        <span className="text-right font-mono">{formatMs(t.totalMs)}</span>
      </div>
      <p className="text-[12px] text-muted">
        Each send opens a fresh connection, so TTFB includes DNS, TCP and TLS setup (like curl). {response.httpVersion} · {response.method}{' '}
        <span className="selectable font-mono">{response.url}</span>
      </p>
    </div>
  );
}

function TestsView({ response }: { response: ResponseView }) {
  const tests = response.testResults ?? [];
  if (!tests.length)
    return (
      <Empty title="No tests">
        Add tests in the request's Scripts tab, e.g. <code className="font-mono">{"ls.test('ok', () => ls.response.to.have.status(200))"}</code>
      </Empty>
    );
  const passed = tests.filter(t => t.passed).length;
  const skipped = tests.filter(t => t.skipped).length;
  return (
    <div className="flex flex-col">
      <div className="flex gap-3 border-b border-app px-3 py-2 text-[12.5px]">
        <span className="text-emerald-700 dark:text-emerald-400">{passed} passed</span>
        <span className="text-rose-700 dark:text-rose-400">{tests.length - passed - skipped} failed</span>
        {skipped > 0 && <span className="text-muted">{skipped} skipped</span>}
      </div>
      {tests.map((t, i) => (
        <div key={i} className="flex items-start gap-2 border-b border-app px-3 py-2">
          {t.skipped ? (
            <CircleSlash className="mt-0.5 size-4 shrink-0 text-muted" />
          ) : t.passed ? (
            <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-500" />
          ) : (
            <XCircle className="mt-0.5 size-4 shrink-0 text-rose-500" />
          )}
          <div className="min-w-0 flex-1">
            <div className={cn(t.skipped && 'text-muted')}>{t.name}</div>
            {t.error && !t.passed && <div className="selectable mt-0.5 font-mono text-[12px] text-rose-700 dark:text-rose-400">{t.error}</div>}
          </div>
          <span className="shrink-0 text-[11px] text-muted">{t.category}</span>
          <span className="w-14 shrink-0 text-right text-[11px] text-muted">{formatMs(t.durationMs)}</span>
        </div>
      ))}
    </div>
  );
}

function ConsoleView({ response }: { response: ResponseView }) {
  const [level, setLevel] = useState('all');
  const lines = (response.console ?? []).filter(l => level === 'all' || l.level === level);
  if (!(response.console ?? []).length) return <Empty title="Console is empty">Output from console.log() in scripts appears here.</Empty>;
  const tone: Record<string, string> = {
    error: 'text-rose-700 dark:text-rose-400',
    warn: 'text-amber-700 dark:text-amber-400',
    info: 'text-sky-700 dark:text-sky-400',
    debug: 'text-muted',
  };
  return (
    <div className="flex flex-col">
      <div className="flex gap-1.5 border-b border-app px-3 py-1.5">
        {['all', 'log', 'info', 'warn', 'error', 'debug'].map(l => (
          <button
            key={l}
            onClick={() => setLevel(l)}
            className={cn('rounded-full px-2.5 py-0.5 text-[12px] capitalize', level === l ? 'bg-accent text-on-accent' : 'bg-muted text-muted hover:text-app')}
          >
            {l}
          </button>
        ))}
      </div>
      <div className="selectable font-mono text-[12px]">
        {lines.map((l, i) => (
          <div key={i} className="flex gap-3 border-b border-app px-3 py-1">
            <span className="shrink-0 text-muted">{new Date(l.timestampMs).toLocaleTimeString([], { hour12: false })}</span>
            <span className={cn('w-10 shrink-0 uppercase', tone[l.level])}>{l.level}</span>
            <span className={cn('min-w-0 flex-1 whitespace-pre-wrap break-all', tone[l.level])}>{l.text}</span>
            <span className="shrink-0 text-[11px] text-muted">{l.source}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
