import { CheckCircle2, ChevronDown, ChevronRight, CircleSlash, Download, FileUp, Play, Square, XCircle } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { Badge, Button, Empty, Input, Kbd, Split, Toggle, useToast } from '../components/ui';
import { api, errorText, onRunnerEvent, type RunResult, type RunSummary, runFailed, type TreeNode } from '../lib/api';
import { cn, formatMs, methodColor, methodShort, modKey } from '../lib/utils';

function requestsUnder(nodes: TreeNode[]): TreeNode[] {
  return nodes.flatMap(n => (n.kind === 'request' ? [n] : n.kind === 'folder' ? requestsUnder(n.children) : []));
}

function findNode(nodes: TreeNode[], id: string): TreeNode | null {
  for (const n of nodes) {
    if (n.id === id) return n;
    const f = findNode(n.children, id);
    if (f) return f;
  }
  return null;
}

function countRows(text: string): number {
  const t = text.trim();
  if (!t) return 0;
  if (t.startsWith('[')) {
    try {
      return (JSON.parse(t) as unknown[]).length;
    } catch {
      return 0;
    }
  }
  return Math.max(0, t.split(/\r?\n/).filter(Boolean).length - 1);
}

export function RunnerView({ targetId, workspaceId, workspaceName, tree }: { targetId: string; workspaceId: string; workspaceName: string; tree: TreeNode[] }) {
  const toast = useToast();
  const target = targetId === workspaceId ? null : findNode(tree, targetId);
  const requests = useMemo(() => requestsUnder(target ? target.children : tree), [target, tree]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(requests.map(r => r.id)));
  const [iterations, setIterations] = useState(1);
  const [delay, setDelay] = useState(0);
  const [bail, setBail] = useState(false);
  const [dataText, setDataText] = useState('');
  const [dataName, setDataName] = useState('');
  const [runId, setRunId] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<RunResult[]>([]);
  const [current, setCurrent] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [summary, setSummary] = useState<RunSummary | null>(null);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const fileInput = useRef<HTMLInputElement>(null);
  const runIdRef = useRef<string | null>(null);

  // keep new requests selected by default
  useEffect(() => {
    setSelected(prev => {
      const next = new Set(prev);
      for (const r of requests) if (!prev.has(r.id) && prev.size === 0) next.add(r.id);
      return next;
    });
  }, [requests]);

  useEffect(() => {
    const un = onRunnerEvent((id, ev) => {
      if (id !== runIdRef.current) return;
      if (ev.type === 'requestStart') setCurrent(`${ev.method} ${ev.name} (iteration ${ev.iteration})`);
      if (ev.type === 'requestEnd') setResults(r => [...r, ev.result]);
      if (ev.type === 'warning') setWarnings(w => [...w, `Iteration ${ev.iteration}: ${ev.message}`]);
      if (ev.type === 'done') {
        setSummary(ev.summary);
        setRunning(false);
        setCurrent(null);
      }
    });
    return () => {
      un.then(f => f());
    };
  }, []);

  const rows = countRows(dataText);
  const effectiveIterations = rows && iterations === 1 ? rows : iterations;
  const ordered = requests.filter(r => selected.has(r.id));
  const expected = ordered.length * effectiveIterations;

  const start = async () => {
    if (running) return;
    setResults([]);
    setSummary(null);
    setWarnings([]);
    setOpen(new Set());
    try {
      setRunning(true);
      // Choose the id up front: the runner may emit events before the start call returns.
      const id = `run_${crypto.randomUUID().replace(/-/g, '')}`;
      runIdRef.current = id;
      setRunId(id);
      await api.runnerStart({ runId: id, requestIds: ordered.map(r => r.id), iterations: effectiveIterations, delayMs: delay, bail, dataText: dataText || null });
    } catch (e) {
      setRunning(false);
      toast(errorText(e), 'error');
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        start();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const exportReport = async (kind: 'junit' | 'json') => {
    if (!runId) return;
    try {
      const path = await api.runnerExport(runId, kind);
      toast(`Saved ${path}`, 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  const passedReq = results.filter(r => !runFailed(r) && !r.skipped).length;
  const failedReq = results.filter(runFailed).length;
  const testsPassed = results.reduce((n, r) => n + r.tests.filter(t => t.passed).length, 0);
  const testsFailed = results.reduce((n, r) => n + r.tests.filter(t => !t.passed && !t.skipped).length, 0);
  const progress = expected ? Math.min(100, (results.length / expected) * 100) : 0;
  let lastIteration = 0;

  return (
    <Split direction="row" initial={360} min={300} storageKey="lsock-split-runner">
      <div className="flex h-full min-h-0 flex-col bg-subtle">
        <div className="shrink-0 border-b border-app px-4 py-3">
          <div className="text-[11px] font-semibold tracking-wide text-muted uppercase">Collection runner</div>
          <div className="mt-0.5 text-[15px] font-semibold">{target ? target.name : workspaceName}</div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[12px] font-semibold text-muted">Requests ({selected.size}/{requests.length})</span>
            <span className="flex gap-2 text-[12px]">
              <button className="text-accent" onClick={() => setSelected(new Set(requests.map(r => r.id)))}>
                All
              </button>
              <button className="text-accent" onClick={() => setSelected(new Set())}>
                None
              </button>
            </span>
          </div>
          <div className="mb-4 rounded-lg border border-app bg-app">
            {requests.map(r => (
              <label key={r.id} className="flex cursor-pointer items-center gap-2 border-b border-app px-2.5 py-1.5 last:border-b-0 hover:bg-muted">
                <input
                  type="checkbox"
                  className="accent-[var(--color-accent)]"
                  checked={selected.has(r.id)}
                  onChange={e => {
                    const next = new Set(selected);
                    if (e.target.checked) next.add(r.id);
                    else next.delete(r.id);
                    setSelected(next);
                  }}
                />
                <span className={cn('w-9 font-mono text-[10px] font-bold', methodColor(r.method))}>{methodShort(r.method)}</span>
                <span className="truncate">{r.name}</span>
              </label>
            ))}
            {requests.length === 0 && <div className="p-3 text-[12.5px] text-muted">No requests here.</div>}
          </div>
          <div className="flex flex-col gap-3">
            <label className="flex items-center justify-between gap-3">
              <span>Iterations</span>
              <Input type="number" min={1} className="w-24" value={iterations} onChange={e => setIterations(Math.max(1, Number(e.target.value) || 1))} />
            </label>
            <label className="flex items-center justify-between gap-3">
              <span>Delay between requests (ms)</span>
              <Input type="number" min={0} className="w-24" value={delay} onChange={e => setDelay(Math.max(0, Number(e.target.value) || 0))} />
            </label>
            <div className="flex items-center justify-between gap-3">
              <span>Stop on first failure</span>
              <Toggle checked={bail} onChange={setBail} />
            </div>
            <div className="flex flex-col gap-1.5">
              <div className="flex items-center justify-between">
                <span>Data file</span>
                <span className="flex gap-1">
                  <Button size="sm" variant="ghost" onClick={() => fileInput.current?.click()}>
                    <FileUp className="size-3.5" /> Load CSV/JSON
                  </Button>
                  {dataText && (
                    <Button size="sm" variant="ghost" onClick={() => (setDataText(''), setDataName(''))}>
                      Clear
                    </Button>
                  )}
                </span>
              </div>
              <input
                ref={fileInput}
                type="file"
                accept=".csv,.json,text/csv,application/json"
                className="hidden"
                onChange={async e => {
                  const f = e.target.files?.[0];
                  if (f) {
                    setDataText(await f.text());
                    setDataName(f.name);
                  }
                  e.target.value = '';
                }}
              />
              <textarea
                value={dataText}
                onChange={e => (setDataText(e.target.value), setDataName(''))}
                placeholder={'email,password\nada@example.com,secret\n…or paste a JSON array'}
                className="h-24 resize-y rounded-md border border-app bg-app p-2 font-mono text-[12px] outline-none focus:border-accent"
              />
              {dataText && (
                <span className="text-[12px] text-muted">
                  {dataName && `${dataName} · `}
                  {rows} row{rows === 1 ? '' : 's'} — each iteration uses the next row via <code className="font-mono">{'{{ column }}'}</code> or{' '}
                  <code className="font-mono">insomnia.iterationData</code>
                  {rows > 0 && iterations === 1 && ` (${rows} iterations)`}
                </span>
              )}
            </div>
          </div>
        </div>
        <div className="flex shrink-0 gap-2 border-t border-app p-3">
          {running ? (
            <Button className="flex-1" onClick={() => runId && api.runnerCancel(runId)}>
              <Square className="size-3.5" /> Stop
            </Button>
          ) : (
            <Button className="flex-1" variant="primary" onClick={start} disabled={!selected.size} title={`Run (${modKey()}+Enter)`}>
              <Play className="size-3.5" /> Run {selected.size} request{selected.size === 1 ? '' : 's'}
              {effectiveIterations > 1 ? ` × ${effectiveIterations}` : ''}
            </Button>
          )}
        </div>
      </div>

      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 border-b border-app px-4 py-3">
          <div className="flex items-center gap-2">
            <Badge tone="green">{passedReq} passed</Badge>
            <Badge tone="red">{failedReq} failed</Badge>
            <Badge>
              tests {testsPassed}/{testsPassed + testsFailed}
            </Badge>
            {summary && <span className="text-[12px] text-muted">{formatMs(summary.durationMs)}</span>}
            {summary?.bailed && <Badge tone="amber">stopped on failure</Badge>}
            {summary?.cancelled && <Badge tone="amber">cancelled</Badge>}
            <div className="flex-1" />
            {summary && (
              <>
                <Button size="sm" variant="ghost" onClick={() => exportReport('junit')}>
                  <Download className="size-3.5" /> JUnit
                </Button>
                <Button size="sm" variant="ghost" onClick={() => exportReport('json')}>
                  <Download className="size-3.5" /> JSON
                </Button>
              </>
            )}
          </div>
          {(running || summary) && (
            <div className="mt-2.5 h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                className={cn('h-full rounded-full transition-all', failedReq ? 'bg-rose-500' : 'bg-emerald-500')}
                style={{ width: `${summary ? 100 : progress}%` }}
              />
            </div>
          )}
          {current && <div className="mt-1.5 truncate text-[12px] text-muted">Running {current}…</div>}
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {warnings.map((w, i) => (
            <div key={i} className="border-b border-app bg-amber-500/5 px-4 py-1.5 text-[12.5px] text-amber-700 dark:text-amber-400">
              ⚠ {w}
            </div>
          ))}
          {results.length === 0 && !running && (
            <Empty icon={<Play className="size-8" />} title="Run requests in sequence">
              Scripts run before and after each request, tests are collected, and data-file rows drive iterations. Press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd> to start.
            </Empty>
          )}
          {results.map((r, i) => {
            const header = r.iteration !== lastIteration && (effectiveIterations > 1 || (summary?.iterations ?? 1) > 1);
            lastIteration = r.iteration;
            const failed = runFailed(r);
            const expanded = open.has(i);
            return (
              <div key={i}>
                {header && <div className="bg-subtle px-4 py-1 text-[11px] font-semibold tracking-wide text-muted uppercase">Iteration {r.iteration}</div>}
                <button
                  className="flex w-full items-center gap-2 border-b border-app px-4 py-2 text-left hover:bg-muted"
                  onClick={() => {
                    const n = new Set(open);
                    if (expanded) n.delete(i);
                    else n.add(i);
                    setOpen(n);
                  }}
                >
                  {expanded ? <ChevronDown className="size-3.5 text-muted" /> : <ChevronRight className="size-3.5 text-muted" />}
                  {r.skipped ? (
                    <CircleSlash className="size-4 text-muted" />
                  ) : failed ? (
                    <XCircle className="size-4 text-rose-500" />
                  ) : (
                    <CheckCircle2 className="size-4 text-emerald-500" />
                  )}
                  <span className={cn('w-10 font-mono text-[10.5px] font-bold', methodColor(r.method))}>{methodShort(r.method)}</span>
                  <span className="min-w-0 flex-1 truncate">{r.name}</span>
                  {r.tests.length > 0 && (
                    <span className={cn('text-[12px]', r.tests.some(t => !t.passed && !t.skipped) ? 'text-rose-600' : 'text-muted')}>
                      {r.tests.filter(t => t.passed).length}/{r.tests.length} tests
                    </span>
                  )}
                  <span className="w-28 truncate text-right text-[12px]">
                    {r.skipped ? 'skipped' : r.error ? <span className="text-rose-600">error</span> : `${r.status} ${r.statusMessage}`}
                  </span>
                  <span className="w-16 text-right text-[12px] text-muted">{formatMs(r.durationMs)}</span>
                </button>
                {expanded && (
                  <div className="border-b border-app bg-subtle px-10 py-2 text-[12.5px]">
                    <div className="selectable mb-1 font-mono text-[12px] text-muted">{r.url}</div>
                    {r.error && <div className="selectable mb-1 font-mono text-rose-700 dark:text-rose-400">{r.error}</div>}
                    {r.scriptError && <div className="selectable mb-1 font-mono text-amber-700 dark:text-amber-400">script: {r.scriptError}</div>}
                    {r.tests.map((t, j) => (
                      <div key={j} className="flex items-start gap-2 py-0.5">
                        {t.skipped ? (
                          <CircleSlash className="mt-0.5 size-3.5 text-muted" />
                        ) : t.passed ? (
                          <CheckCircle2 className="mt-0.5 size-3.5 text-emerald-500" />
                        ) : (
                          <XCircle className="mt-0.5 size-3.5 text-rose-500" />
                        )}
                        <span>{t.name}</span>
                        {!t.passed && t.error && <span className="font-mono text-[12px] text-rose-700 dark:text-rose-400">— {t.error}</span>}
                      </div>
                    ))}
                    {r.console.length > 0 && (
                      <div className="selectable mt-1.5 rounded border border-app bg-app p-2 font-mono text-[11.5px]">
                        {r.console.map((c, j) => (
                          <div key={j} className={cn(c.level === 'error' && 'text-rose-600', c.level === 'warn' && 'text-amber-600')}>
                            {c.text}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </Split>
  );
}
