import { AlertTriangle, Braces, CheckCircle2, FileJson, Play, Search, ShieldAlert, Wrench, XCircle } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { CodeEditor, JsonTree, Markdown } from '../components/editors';
import { SchemaForm } from '../components/SchemaForm';
import { Badge, Button, CopyButton, Empty, Kbd, Modal, Split, Tabs, useToast } from '../components/ui';
import { api, type CallToolResult, errorText, type Hints, type ParamRow, type ToolView } from '../lib/api';
import { cn, formatMs, modKey } from '../lib/utils';

/** What each behavior label means, shared by the badges' tooltips and the legend. */
const HINTS = {
  'read-only': { tone: 'green', meaning: 'Only reads. It does not change anything.' },
  writes: { tone: 'amber', meaning: 'Changes data, but only by adding. It does not delete or overwrite.' },
  destructive: { tone: 'red', meaning: 'May delete or overwrite data. Logic Socket asks before running it.' },
  idempotent: { tone: 'blue', meaning: 'Safe to repeat: calling it again with the same arguments has no extra effect.' },
  'open-world': { tone: 'violet', meaning: 'Reaches outside systems, such as the web or a third-party API.' },
  'closed-world': { tone: 'neutral', meaning: 'Stays within its own data or service.' },
  unannotated: {
    tone: 'neutral',
    meaning:
      'The server did not describe this tool. The MCP spec says to assume it may change or delete data and reach outside systems, so Logic Socket asks before running it.',
  },
} as const;
type HintLabel = keyof typeof HINTS;

export function HintBadges({ hints, compact }: { hints: Hints; compact?: boolean }) {
  const labels: HintLabel[] = [];
  if (!hints.declared) labels.push('unannotated');
  else {
    labels.push(hints.readOnly ? 'read-only' : hints.destructive ? 'destructive' : 'writes');
    if (hints.idempotent) labels.push('idempotent');
    if (!compact || !hints.openWorld) labels.push(hints.openWorld ? 'open-world' : 'closed-world');
  }
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      {labels.map(l => (
        <Badge key={l} tone={HINTS[l].tone} title={HINTS[l].meaning}>
          {l}
        </Badge>
      ))}
    </span>
  );
}

/** "What do the labels mean?" link and the legend it opens. */
export function HintLegend() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button className="text-[11.5px] text-muted underline-offset-2 hover:text-app hover:underline" onClick={() => setOpen(true)}>
        What do the labels mean?
      </button>
      <Modal open={open} onClose={() => setOpen(false)} title="Tool labels" width="max-w-lg">
        <div className="flex flex-col gap-2.5 p-5">
          <p className="mb-1 text-[12.5px] text-muted">MCP servers can describe how each tool behaves. These are hints from the server, not guarantees.</p>
          {(Object.keys(HINTS) as HintLabel[]).map(l => (
            <div key={l} className="grid grid-cols-[104px_1fr] items-start gap-3">
              <span>
                <Badge tone={HINTS[l].tone}>{l}</Badge>
              </span>
              <span className="text-[12.5px]">{HINTS[l].meaning}</span>
            </div>
          ))}
        </div>
      </Modal>
    </>
  );
}

function fmt(v: unknown): string {
  return typeof v === 'string' ? `"${v}"` : JSON.stringify(v);
}

export function ParamTable({ rows, empty = 'No parameters' }: { rows: ParamRow[]; empty?: string }) {
  if (!rows.length) return <div className="px-1 py-2 text-[12.5px] text-muted">{empty}</div>;
  return (
    <div className="overflow-hidden rounded-lg border border-app">
      <table className="selectable w-full text-[12.5px]">
        <thead className="bg-subtle text-left text-[11px] tracking-wide text-muted uppercase">
          <tr>
            <th className="px-3 py-1.5 font-semibold">Parameter</th>
            <th className="px-3 py-1.5 font-semibold">Type</th>
            <th className="px-3 py-1.5 font-semibold">Default</th>
            <th className="w-1/2 px-3 py-1.5 font-semibold">Details</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(r => (
            <tr key={r.path} className="border-t border-app align-top">
              <td className="px-3 py-2 whitespace-nowrap" style={{ paddingLeft: 12 + r.depth * 16 }}>
                {r.depth > 0 && <span className="mr-1 text-muted">└</span>}
                <span className={cn('font-mono', r.required && 'font-semibold')}>{r.name}</span>
                {r.required && (
                  <span className="ml-1 text-amber-600 dark:text-amber-400" title="Required">
                    *
                  </span>
                )}
              </td>
              <td className="px-3 py-2">
                <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11.5px] whitespace-nowrap">{r.typeLabel}</code>
              </td>
              <td className="px-3 py-2 font-mono text-[11.5px] text-muted">{r.default !== undefined && r.default !== null ? fmt(r.default) : ''}</td>
              <td className="px-3 py-2">
                {r.description && <div className="mb-1">{r.description}</div>}
                <div className="flex flex-wrap gap-1">
                  {r.constraints.map(c => (
                    <Badge key={c}>{c}</Badge>
                  ))}
                  {r.enumValues.length > 0 && (
                    <span className="text-[11.5px] text-muted">
                      one of{' '}
                      {r.enumValues.map((v, i) => (
                        <code key={i} className="mx-0.5 rounded bg-accent-soft px-1 font-mono text-accent">
                          {typeof v === 'string' ? v : JSON.stringify(v)}
                        </code>
                      ))}
                    </span>
                  )}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ContentBlock({ block }: { block: Record<string, unknown> }) {
  const toast = useToast();
  const type = block.type as string;
  if (type === 'text') {
    const text = String(block.text ?? '');
    try {
      const parsed = JSON.parse(text);
      if (parsed && typeof parsed === 'object')
        return <JsonTree value={parsed} onPath={p => (navigator.clipboard?.writeText(p), toast(`Copied ${p}`))} />;
    } catch {
      /* plain text */
    }
    return <pre className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{text}</pre>;
  }
  if (type === 'image') return <img alt="Tool output" className="max-w-full rounded" src={`data:${block.mimeType};base64,${block.data}`} />;
  if (type === 'audio') return <audio controls src={`data:${block.mimeType};base64,${block.data}`} />;
  if (type === 'resource_link' || type === 'resource') {
    const res = (block.resource as Record<string, unknown>) ?? block;
    return (
      <div className="rounded-md border border-app p-2">
        <div className="font-mono text-[12px] text-accent">{String(res.uri ?? '')}</div>
        {res.text ? <pre className="selectable mt-1 font-mono text-[12px] whitespace-pre-wrap">{String(res.text)}</pre> : null}
      </div>
    );
  }
  return <JsonTree value={block} onPath={() => {}} />;
}

function Result({ result, latency, error }: { result: CallToolResult | null; latency: number; error: string | null }) {
  const [raw, setRaw] = useState(false);
  if (error)
    return (
      <div className="rounded-lg border border-rose-500/30 bg-rose-500/5 p-3">
        <div className="mb-1 flex items-center gap-1.5 font-semibold text-rose-700 dark:text-rose-400">
          <XCircle className="size-4" /> Protocol error
        </div>
        <div className="selectable font-mono text-[12.5px] whitespace-pre-wrap">{error}</div>
      </div>
    );
  if (!result) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {result.isError ? (
          <span className="flex items-center gap-1.5 font-semibold text-rose-700 dark:text-rose-400">
            <AlertTriangle className="size-4" /> Tool returned an error
          </span>
        ) : (
          <span className="flex items-center gap-1.5 font-semibold text-emerald-700 dark:text-emerald-400">
            <CheckCircle2 className="size-4" /> Success
          </span>
        )}
        <span className="text-[12px] text-muted">{formatMs(latency)}</span>
        <span className="text-[12px] text-muted">
          · {result.content.length} content block{result.content.length === 1 ? '' : 's'}
        </span>
        <div className="flex-1" />
        <Button size="sm" variant="ghost" onClick={() => setRaw(!raw)}>
          <FileJson className="size-3.5" /> {raw ? 'Readable' : 'Raw JSON'}
        </Button>
        <CopyButton text={JSON.stringify(result, null, 2)} />
      </div>
      {raw ? (
        <div className="h-72 overflow-hidden rounded-md border border-app">
          <CodeEditor value={JSON.stringify(result, null, 2)} readOnly />
        </div>
      ) : (
        <>
          {result.content.map((b, i) => (
            <div key={i} className="rounded-md border border-app bg-subtle p-3">
              <div className="mb-1.5 text-[10.5px] font-semibold tracking-wide text-muted uppercase">{String(b.type)}</div>
              <ContentBlock block={b} />
            </div>
          ))}
          {result.structuredContent !== undefined && result.structuredContent !== null && (
            <div className="rounded-md border border-app bg-subtle p-3">
              <div className="mb-1.5 text-[10.5px] font-semibold tracking-wide text-muted uppercase">Structured content</div>
              <JsonTree value={result.structuredContent} onPath={() => {}} defaultOpen={3} />
            </div>
          )}
        </>
      )}
    </div>
  );
}

function ToolDetail({ serverId, tool }: { serverId: string; tool: ToolView }) {
  const [view, setView] = useState('overview');
  const [mode, setMode] = useState<'form' | 'json'>('form');
  const [args, setArgs] = useState<unknown>(tool.example ?? {});
  const [jsonText, setJsonText] = useState(JSON.stringify(tool.example ?? {}, null, 2));
  const [errors, setErrors] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CallToolResult | null>(null);
  const [callError, setCallError] = useState<string | null>(null);
  const [latency, setLatency] = useState(0);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setArgs(tool.example ?? {});
    setJsonText(JSON.stringify(tool.example ?? {}, null, 2));
    setResult(null);
    setCallError(null);
    setView('overview');
  }, [tool.name, tool.example]);

  useEffect(() => {
    const t = setTimeout(() => api.mcpValidate(tool.inputSchema, args).then(setErrors).catch(() => {}), 150);
    return () => clearTimeout(t);
  }, [args, tool.inputSchema]);

  const run = useCallback(
    async (confirmed = false) => {
      if (tool.hints.destructive && !confirmed) {
        setConfirm(true);
        return;
      }
      setConfirm(false);
      setRunning(true);
      setCallError(null);
      setView('try');
      try {
        const r = await api.mcpCallTool(serverId, tool.name, args);
        setResult(r.result);
        setLatency(r.latencyMs);
      } catch (e) {
        setResult(null);
        setCallError(errorText(e));
      } finally {
        setRunning(false);
      }
    },
    [serverId, tool, args],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && view === 'try') {
        e.preventDefault();
        run();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [run, view]);

  const definition = useMemo(() => {
    const { displayName: _d, hints: _h, params: _p, outputParams: _o, example: _e, ...rest } = tool;
    return JSON.stringify(rest, null, 2);
  }, [tool]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-app px-5 pt-4 pb-3">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <h2 className="text-[17px] leading-tight font-semibold">{tool.displayName}</h2>
            {tool.displayName !== tool.name && <div className="selectable mt-0.5 font-mono text-[12px] text-muted">{tool.name}</div>}
          </div>
          <Button variant="primary" onClick={() => setView('try')}>
            <Play className="size-3.5" /> Try it
          </Button>
        </div>
        <div className="mt-2">
          <HintBadges hints={tool.hints} />
        </div>
      </div>
      <Tabs
        value={view}
        onChange={setView}
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'try', label: 'Try it', dot: !!result || !!callError },
          { id: 'json', label: 'Definition JSON' },
        ]}
      />
      <div className="min-h-0 flex-1 overflow-auto">
        {view === 'overview' && (
          <div className="flex max-w-4xl flex-col gap-5 p-5">
            {tool.description ? <Markdown>{tool.description}</Markdown> : <div className="text-muted italic">No description provided.</div>}
            <section>
              <h3 className="mb-2 text-[12px] font-semibold tracking-wide text-muted uppercase">
                Input parameters <span className="font-normal normal-case">· {tool.params.filter(p => p.depth === 0).length} top-level</span>
              </h3>
              <ParamTable rows={tool.params} empty="This tool takes no arguments." />
            </section>
            {tool.outputSchema && (
              <section>
                <h3 className="mb-2 text-[12px] font-semibold tracking-wide text-muted uppercase">Returns (structured output)</h3>
                <ParamTable rows={tool.outputParams} empty="Output schema has no named properties." />
              </section>
            )}
          </div>
        )}
        {view === 'try' && (
          <Split direction="col" initial={300} min={140} storageKey="lsock-split-try">
            <div className="flex h-full min-h-0 flex-col">
              <div className="flex shrink-0 items-center gap-2 border-b border-app px-4 py-2">
                <div className="flex rounded-md border border-app p-0.5">
                  {(['form', 'json'] as const).map(m => (
                    <button
                      key={m}
                      onClick={() => {
                        if (m === 'json') setJsonText(JSON.stringify(args ?? {}, null, 2));
                        setMode(m);
                      }}
                      className={cn('rounded px-2.5 py-0.5 text-[12px]', mode === m ? 'bg-accent text-on-accent' : 'text-muted hover:text-app')}
                    >
                      {m === 'form' ? 'Form' : 'JSON'}
                    </button>
                  ))}
                </div>
                <Button size="sm" variant="ghost" onClick={() => (setArgs(tool.example ?? {}), setJsonText(JSON.stringify(tool.example ?? {}, null, 2)))}>
                  <Braces className="size-3.5" /> Reset to example
                </Button>
                <div className="flex-1" />
                {errors.length > 0 ? (
                  <span className="text-[12px] text-amber-600 dark:text-amber-400">
                    {errors.length} issue{errors.length > 1 ? 's' : ''}
                  </span>
                ) : (
                  <span className="text-[12px] text-emerald-600 dark:text-emerald-400">Arguments valid</span>
                )}
                <Button variant="primary" onClick={() => run()} loading={running} title={`Call tool (${modKey()}+Enter)`}>
                  {!running && <Play className="size-3.5" />} Call tool
                </Button>
              </div>
              <div className="min-h-0 flex-1 overflow-auto">
                {mode === 'form' ? (
                  <div className="max-w-2xl p-4">
                    <SchemaForm root={tool.inputSchema} schema={tool.inputSchema} value={args} onChange={setArgs} />
                  </div>
                ) : (
                  <CodeEditor
                    value={jsonText}
                    onChange={t => {
                      setJsonText(t);
                      try {
                        setArgs(JSON.parse(t));
                      } catch {
                        /* keep last valid */
                      }
                    }}
                  />
                )}
              </div>
              {errors.length > 0 && (
                <ul className="shrink-0 border-t border-app bg-amber-500/5 px-4 py-2 text-[12px] text-amber-700 dark:text-amber-400">
                  {errors.map(e => (
                    <li key={e}>• {e}</li>
                  ))}
                </ul>
              )}
            </div>
            <div className="h-full overflow-auto p-4">
              {running ? (
                <div className="flex items-center gap-2 text-muted">
                  <span className="size-2 animate-ping rounded-full bg-accent" /> Calling {tool.name}…
                </div>
              ) : result || callError ? (
                <Result result={result} latency={latency} error={callError} />
              ) : (
                <div className="text-[12.5px] text-muted">
                  Fill in the arguments and press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd>. Every frame is recorded in the Protocol log tab.
                </div>
              )}
            </div>
          </Split>
        )}
        {view === 'json' && (
          <div className="flex h-full flex-col">
            <div className="flex shrink-0 justify-end border-b border-app px-2 py-1">
              <CopyButton text={definition} />
            </div>
            <CodeEditor value={definition} readOnly />
          </div>
        )}
      </div>
      <Modal
        open={confirm}
        onClose={() => setConfirm(false)}
        width="max-w-md"
        title={
          <span className="flex items-center gap-2">
            <ShieldAlert className="size-4 text-rose-500" /> Run a destructive tool?
          </span>
        }
        footer={
          <>
            <Button onClick={() => setConfirm(false)}>Cancel</Button>
            <Button variant="danger" onClick={() => run(true)}>
              Run {tool.name}
            </Button>
          </>
        }
      >
        <div className="p-4 text-[13px] leading-relaxed">
          <b className="font-mono">{tool.name}</b> is {tool.hints.declared ? 'marked as destructive by the server' : 'not annotated, so the spec treats it as potentially destructive'}. It may delete or overwrite data.
        </div>
      </Modal>
    </div>
  );
}

export function ToolsPanel({ serverId, tools }: { serverId: string; tools: ToolView[] }) {
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<string | null>(tools[0]?.name ?? null);
  const filtered = useMemo(() => {
    const ql = q.toLowerCase();
    return tools.filter(
      t => !ql || t.name.toLowerCase().includes(ql) || t.displayName.toLowerCase().includes(ql) || (t.description ?? '').toLowerCase().includes(ql),
    );
  }, [tools, q]);
  const tool = tools.find(t => t.name === selected) ?? filtered[0];
  if (!tools.length) return <Empty icon={<Wrench className="size-8" />} title="This server exposes no tools" />;
  return (
    <Split direction="row" initial={300} min={220} storageKey="lsock-split-tools">
      <div className="flex h-full min-h-0 flex-col bg-subtle">
        <div className="relative shrink-0 p-2">
          <Search className="pointer-events-none absolute top-4 left-4 size-3.5 text-muted" />
          <input
            value={q}
            onChange={e => setQ(e.target.value)}
            placeholder={`Search ${tools.length} tools`}
            aria-label="Search tools"
            className="h-8 w-full rounded-md border border-app bg-app pr-2 pl-7 text-[12.5px] outline-none focus:border-accent"
          />
          <div className="mt-1 px-0.5">
            <HintLegend />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto px-1.5 pb-3">
          {filtered.map(t => (
            <button
              key={t.name}
              onClick={() => setSelected(t.name)}
              className={cn(
                'mb-0.5 flex w-full flex-col gap-1 rounded-md px-2.5 py-2 text-left',
                tool?.name === t.name ? 'bg-accent-soft' : 'hover:bg-muted',
              )}
            >
              <div className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] font-medium">{t.name}</span>
                <span className="text-[11px] text-muted">{t.params.filter(p => p.depth === 0).length} params</span>
              </div>
              {t.description && <span className="line-clamp-2 text-[12px] text-muted">{t.description.replace(/[*_`#]/g, '')}</span>}
              <HintBadges hints={t.hints} compact />
            </button>
          ))}
          {filtered.length === 0 && <div className="p-4 text-center text-[12.5px] text-muted">No tools match “{q}”</div>}
        </div>
      </div>
      {tool ? <ToolDetail key={tool.name} serverId={serverId} tool={tool} /> : <Empty title="Select a tool" />}
    </Split>
  );
}
