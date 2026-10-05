import { ArrowDownLeft, ArrowUpRight, Braces, FileCode, Info, Play, Plus, RefreshCw, Send, Square, TriangleAlert, Upload } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';

import { CodeEditor, JsonTree, KeyValueEditor } from '../components/editors';
import { Badge, Button, Empty, Input, Kbd, Modal, Select, Split, Tabs, useDialog, useToast } from '../components/ui';
import { VarInput } from '../components/VarInput';
import { api, errorText, type GrpcMethod, type GrpcRequest, type GrpcService, type GrpcUnaryResult, onGrpcEvent, type ProtoFile, type RtEvent } from '../lib/api';
import { clockTime, cn, formatMs, modKey } from '../lib/utils';
import { useAutosave } from './RequestView';

function kindLabel(m: GrpcMethod) {
  if (m.clientStreaming && m.serverStreaming) return { label: 'bidi', tone: 'violet' };
  if (m.clientStreaming) return { label: 'client stream', tone: 'blue' };
  if (m.serverStreaming) return { label: 'server stream', tone: 'green' };
  return { label: 'unary', tone: 'neutral' };
}

export function ProtoFilesModal({ open, onClose, workspaceId, onChanged }: { open: boolean; onClose: () => void; workspaceId: string; onChanged: () => void }) {
  const toast = useToast();
  const dialog = useDialog();
  const [files, setFiles] = useState<ProtoFile[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const reload = () => api.protoFileList(workspaceId).then(setFiles);
  useEffect(() => {
    if (open) reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, workspaceId]);
  const current = files.find(f => f.id === sel) ?? files[0];
  const save = async (doc: ProtoFile) => {
    setFiles(fs => fs.map(f => (f.id === doc.id ? doc : f)));
    await api.protoFileUpdate(doc).catch(e => toast(errorText(e), 'error'));
    onChanged();
  };
  return (
    <Modal open={open} onClose={onClose} title={<span className="flex items-center gap-2"><FileCode className="size-4" /> Proto files</span>} width="max-w-5xl">
      <div className="flex h-[65vh]">
        <div className="flex w-60 shrink-0 flex-col border-r border-app bg-subtle p-2">
          {files.map(f => (
            <button key={f.id} onClick={() => setSel(f.id)} className={cn('mb-0.5 truncate rounded-md px-2.5 py-1.5 text-left font-mono text-[12.5px]', current?.id === f.id ? 'bg-accent-soft' : 'hover:bg-muted')}>
              {f.name}
            </button>
          ))}
          <Button
            size="sm"
            variant="ghost"
            className="mt-1 justify-start"
            onClick={async () => {
              const name = await dialog.prompt('New proto file (path used by imports)', 'service.proto');
              if (!name) return;
              const f = await api.protoFileCreate(workspaceId, name, 'syntax = "proto3";\n\npackage example;\n\nservice Example {\n  rpc Ping (PingRequest) returns (PingReply);\n}\n\nmessage PingRequest { string text = 1; }\nmessage PingReply { string text = 1; }\n');
              await reload();
              setSel(f.id);
              onChanged();
            }}
          >
            <Plus className="size-3.5" /> New file
          </Button>
          <label className="flex cursor-pointer items-center gap-1.5 rounded-md px-2.5 py-1.5 text-[12.5px] hover:bg-muted">
            <Upload className="size-3.5" /> Import .proto files
            <input
              type="file"
              multiple
              accept=".proto"
              className="hidden"
              onChange={async e => {
                for (const file of Array.from(e.target.files ?? [])) await api.protoFileCreate(workspaceId, file.name, await file.text());
                e.target.value = '';
                await reload();
                onChanged();
              }}
            />
          </label>
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          {current ? (
            <>
              <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
                <Input className="max-w-sm font-mono" value={current.name} onChange={e => setFiles(fs => fs.map(f => (f.id === current.id ? { ...f, name: e.target.value } : f)))} onBlur={() => save(current)} />
                <div className="flex-1" />
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={async () => {
                    if (!(await dialog.confirm(`Delete ${current.name}?`, { danger: true, confirmLabel: 'Delete' }))) return;
                    await api.itemDelete(current.id);
                    setSel(null);
                    reload();
                    onChanged();
                  }}
                >
                  Delete
                </Button>
              </div>
              <CodeEditor value={current.contents} language="text" onChange={contents => save({ ...current, contents })} />
            </>
          ) : (
            <Empty icon={<FileCode className="size-8" />} title="No proto files">Add or import .proto files. Imports resolve between files by name; Google well-known types are built in.</Empty>
          )}
        </div>
      </div>
    </Modal>
  );
}

export function GrpcView({ id, workspaceId }: { id: string; workspaceId: string }) {
  const toast = useToast();
  const [req, setReq, flush] = useAutosave<GrpcRequest>(id, api.grpcUpdate);
  const [services, setServices] = useState<GrpcService[]>([]);
  const [loading, setLoading] = useState(false);
  const [schemaError, setSchemaError] = useState<string | null>(null);
  const [tab, setTab] = useState('message');
  const [result, setResult] = useState<GrpcUnaryResult | null>(null);
  const [events, setEvents] = useState<RtEvent[]>([]);
  const [running, setRunning] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [protosOpen, setProtosOpen] = useState(false);
  const [resultTab, setResultTab] = useState('response');

  const load = async (refresh: boolean) => {
    setLoading(true);
    setSchemaError(null);
    try {
      await flush();
      setServices(await api.grpcMethods(id, refresh));
    } catch (e) {
      setServices([]);
      setSchemaError(errorText(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load(false);
    api.grpcLog(id).then(setEvents);
    const un = onGrpcEvent((gid, ev) => {
      if (gid !== id) return;
      setEvents(prev => (prev.some(x => x.seq === ev.seq) ? prev : [...prev, ev]));
      if (ev.kind === 'close' || ev.direction === 'error') setStreaming(false);
    });
    return () => {
      un.then(f => f());
    };
  }, [id]); // eslint-disable-line react-hooks/exhaustive-deps

  const method = useMemo(() => services.flatMap(s => s.methods).find(m => m.path === req?.method), [services, req?.method]);
  const isStream = !!method && (method.clientStreaming || method.serverStreaming);

  const run = async () => {
    if (!req || !method) return;
    await flush();
    if (!isStream) {
      setRunning(true);
      try {
        setResult(await api.grpcInvoke(id));
      } catch (e) {
        toast(errorText(e), 'error');
      } finally {
        setRunning(false);
      }
      return;
    }
    try {
      setEvents([]);
      await api.grpcStreamStart(id);
      setStreaming(true);
      setTab('message');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        if (streaming && method?.clientStreaming) api.grpcSend(id).catch(err => toast(errorText(err), 'error'));
        else run();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  if (!req) return <Empty title="Loading…" />;
  const update = (p: Partial<GrpcRequest>) => setReq({ ...req, ...p });
  const statusTone = result ? (result.status.code === 0 ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400' : 'bg-rose-500/15 text-rose-700 dark:text-rose-400') : '';

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
        <span className="rounded bg-muted px-2 py-1 font-mono text-[11px] font-bold text-teal-600 dark:text-teal-400">gRPC</span>
        <VarInput className="w-72" value={req.url} contextId={id} onChange={url => update({ url })} placeholder="grpc://localhost:50051" />
        <Select
          aria-label="Method"
          className="min-w-0 flex-1 font-mono text-[12.5px]"
          value={req.method}
          onChange={e => {
            const m = services.flatMap(s => s.methods).find(x => x.path === e.target.value);
            const emptyBody = !req.message.trim() || req.message.trim() === '{}';
            update({ method: e.target.value, ...(m && emptyBody ? { message: JSON.stringify(m.example, null, 2) } : {}) });
          }}
        >
          <option value="">{loading ? 'Loading methods…' : services.length ? 'Select a method…' : 'No methods — load the schema'}</option>
          {services.map(s => (
            <optgroup key={s.name} label={s.name}>
              {s.methods.map(m => (
                <option key={m.path} value={m.path}>
                  {m.name} {m.clientStreaming || m.serverStreaming ? `(${kindLabel(m).label})` : ''}
                </option>
              ))}
            </optgroup>
          ))}
        </Select>
        {streaming ? (
          <Button onClick={() => api.grpcCancel(id).then(() => setStreaming(false))}>
            <Square className="size-3.5" /> Cancel
          </Button>
        ) : (
          <Button variant="primary" onClick={run} loading={running} disabled={!method}>
            {!running && <Play className="size-3.5" />} {isStream ? 'Start' : 'Invoke'}
          </Button>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2 border-b border-app bg-subtle px-3 py-1.5 text-[12px]">
        <span className="text-muted">Schema</span>
        <div className="flex rounded-md border border-app p-0.5">
          {(['reflection', 'protos'] as const).map(src => (
            <button
              key={src}
              onClick={() => {
                update({ schemaSource: src });
                setTimeout(() => load(true), 400);
              }}
              className={cn('rounded px-2 py-0.5', req.schemaSource === src ? 'bg-accent text-white' : 'text-muted hover:text-app')}
            >
              {src === 'reflection' ? 'Server reflection' : 'Proto files'}
            </button>
          ))}
        </div>
        {req.schemaSource === 'protos' && (
          <Button size="sm" variant="ghost" onClick={() => setProtosOpen(true)}>
            <FileCode className="size-3.5" /> Manage proto files
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => load(true)} loading={loading}>
          {!loading && <RefreshCw className="size-3.5" />} Reload
        </Button>
        {method && (
          <span className="ml-auto flex items-center gap-2">
            <Badge tone={kindLabel(method).tone}>{kindLabel(method).label}</Badge>
            <span className="font-mono text-muted">
              {method.inputType} → {method.outputType}
            </span>
          </span>
        )}
      </div>
      {schemaError && <div className="shrink-0 border-b border-rose-500/30 bg-rose-500/5 px-3 py-1.5 font-mono text-[12px] text-rose-700 dark:text-rose-400">{schemaError}</div>}

      <Split direction="row" initial={480} min={320} storageKey="irs-split-grpc">
        <div className="flex h-full min-h-0 flex-col">
          <Tabs
            value={tab}
            onChange={setTab}
            tabs={[
              { id: 'message', label: 'Message' },
              { id: 'metadata', label: 'Metadata', count: req.metadata.filter(m => !m.disabled && m.name).length },
              { id: 'settings', label: 'Settings' },
            ]}
          />
          {tab === 'message' && (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1">
                <span className="text-[12px] text-muted">{method ? method.inputType : 'JSON message'} — {'{{ variables }}'} work here</span>
                <div className="flex-1" />
                {method && (
                  <Button size="sm" variant="ghost" onClick={() => update({ message: JSON.stringify(method.example, null, 2) })}>
                    <Braces className="size-3.5" /> Example
                  </Button>
                )}
              </div>
              <CodeEditor value={req.message} onChange={message => update({ message })} />
              {streaming && method?.clientStreaming && (
                <div className="flex shrink-0 items-center gap-2 border-t border-app px-3 py-2">
                  <span className="text-[11.5px] text-muted">
                    <Kbd>{modKey()}</Kbd> <Kbd>↵</Kbd> send
                  </span>
                  <div className="flex-1" />
                  <Button onClick={() => api.grpcCommit(id).catch(e => toast(errorText(e), 'error'))}>Commit</Button>
                  <Button variant="primary" onClick={() => api.grpcSend(id).catch(e => toast(errorText(e), 'error'))}>
                    <Send className="size-3.5" /> Send
                  </Button>
                </div>
              )}
            </div>
          )}
          {tab === 'metadata' && <KeyValueEditor items={req.metadata} onChange={metadata => update({ metadata })} contextId={id} namePlaceholder="authorization" />}
          {tab === 'settings' && (
            <div className="flex max-w-md flex-col gap-3 p-4">
              <label className="flex items-center justify-between gap-3">
                <span>Deadline (ms)</span>
                <Input type="number" className="w-32" value={req.timeoutMs} onChange={e => update({ timeoutMs: Number(e.target.value) || 30000 })} />
              </label>
              <p className="text-[12px] text-muted">Use grpc:// for plaintext (HTTP/2 cleartext) and grpcs:// for TLS.</p>
            </div>
          )}
        </div>

        <div className="flex h-full min-h-0 flex-col">
          {isStream || (!result && events.length) ? (
            <div className="min-h-0 flex-1 overflow-auto font-mono text-[12px]">
              {events.map(e => (
                <div key={e.seq} className="grid grid-cols-[18px_88px_1fr] items-start gap-2 border-b border-app px-3 py-1.5">
                  {e.direction === 'out' ? (
                    <ArrowUpRight className="mt-0.5 size-3.5 text-sky-500" />
                  ) : e.direction === 'in' ? (
                    <ArrowDownLeft className="mt-0.5 size-3.5 text-emerald-500" />
                  ) : e.direction === 'error' ? (
                    <TriangleAlert className="mt-0.5 size-3.5 text-rose-500" />
                  ) : (
                    <Info className="mt-0.5 size-3.5 text-muted" />
                  )}
                  <span className="text-muted">{clockTime(e.timestampMs)}</span>
                  {e.direction === 'in' || e.direction === 'out' ? (
                    <JsonTree value={JSON.parse(e.data)} onPath={() => {}} defaultOpen={3} />
                  ) : (
                    <span className={cn(e.direction === 'error' ? 'text-rose-600' : 'text-muted')}>{e.data}</span>
                  )}
                </div>
              ))}
              {events.length === 0 && (
                <div className="h-full font-sans">
                  <Empty title="No messages yet">Press Start. Server messages stream in here; for client/bidi streams, edit the message and press Send, then Commit.</Empty>
                </div>
              )}
            </div>
          ) : result ? (
            <>
              <div className="flex h-11 shrink-0 items-center gap-2 border-b border-app px-3">
                <span className={cn('rounded px-2 py-0.5 text-[12px] font-semibold', statusTone)}>
                  {result.status.code} {result.status.codeName}
                </span>
                <span className="text-[12px] text-muted">{formatMs(result.latencyMs)}</span>
                {result.status.message && <span className="truncate text-[12.5px] text-rose-600">{result.status.message}</span>}
              </div>
              <Tabs value={resultTab} onChange={setResultTab} tabs={[{ id: 'response', label: 'Response' }, { id: 'headers', label: 'Headers', count: result.headers.length }]} />
              <div className="min-h-0 flex-1 overflow-auto p-3">
                {resultTab === 'response' ? (
                  result.response !== undefined && result.response !== null ? (
                    <JsonTree value={result.response} onPath={p => (navigator.clipboard?.writeText(p), toast(`Copied ${p}`))} defaultOpen={4} />
                  ) : (
                    <Empty title="No response message">The call ended with a non-OK status.</Empty>
                  )
                ) : (
                  <table className="w-full font-mono text-[12.5px]">
                    <tbody>
                      {result.headers.map(([k, v]) => (
                        <tr key={k} className="border-b border-app">
                          <td className="py-1 pr-3 text-violet-700 dark:text-violet-300">{k}</td>
                          <td className="py-1 break-all">{v}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </>
          ) : (
            <Empty icon={<Play className="size-8" />} title="Invoke a method">
              Load the schema (server reflection or proto files), pick a method — an example message is filled in — and press <Kbd>{modKey()}</Kbd> <Kbd>Enter</Kbd>.
            </Empty>
          )}
        </div>
      </Split>
      <ProtoFilesModal open={protosOpen} onClose={() => setProtosOpen(false)} workspaceId={workspaceId} onChanged={() => load(true)} />
    </div>
  );
}
