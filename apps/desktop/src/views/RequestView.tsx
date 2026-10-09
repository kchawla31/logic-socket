import { Braces, FileCode2, Send } from 'lucide-react';
import { type ClipboardEvent, useCallback, useEffect, useRef, useState } from 'react';

import { CodeEditor, KeyValueEditor } from '../components/editors';
import { GraphQLEditor } from '../components/GraphQLEditor';
import { ScriptEditor } from '../components/ScriptEditor';
import { Button, Empty, IconButton, Input, Kbd, Select, Split, Tabs, Toggle, useToast } from '../components/ui';
import { VarInput } from '../components/VarInput';
import { api, type Body, errorText, type KeyValue, type Preview, type Request, type ResponseView } from '../lib/api';
import { cn, looksLikeCurl, METHODS, methodColor, modKey, pathParamsInUrl, prettyJson, splitQuery } from '../lib/utils';
import { AuthEditor } from './AuthEditor';
import { CodeModal } from './ImportExport';
import { ResponsePane } from './ResponsePane';

const BODY_TYPES: { id: string; label: string }[] = [
  { id: '', label: 'No body' },
  { id: 'application/json', label: 'JSON' },
  { id: 'application/graphql', label: 'GraphQL' },
  { id: 'application/x-www-form-urlencoded', label: 'Form URL-encoded' },
  { id: 'multipart/form-data', label: 'Multipart form' },
  { id: 'text/plain', label: 'Plain text' },
  { id: 'application/xml', label: 'XML' },
  { id: 'application/octet-stream', label: 'Binary file' },
];

/** Debounced autosave of a document; returns [doc, setDoc]. */
function useAutosave<T extends { id: string }>(id: string, save: (d: T) => Promise<unknown>) {
  const [doc, setDocState] = useState<T | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<T | null>(null);
  const toast = useToast();

  const flush = useCallback(async () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const p = pending.current;
    pending.current = null;
    if (p) await save(p).catch(e => toast(`Save failed: ${errorText(e)}`, 'error'));
  }, [save, toast]);

  useEffect(() => {
    let live = true;
    api.docGet<T>(id).then(d => live && setDocState(d));
    return () => {
      live = false;
      flush();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const setDoc = useCallback(
    (next: T) => {
      setDocState(next);
      pending.current = next;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(flush, 350);
    },
    [flush],
  );
  return [doc, setDoc, flush] as const;
}

export { useAutosave };

/** Copy the HTTP call from a parsed curl command. Scripts, settings, and docs stay. */
function applyCurlImport(req: Request, parsed: Request): Request {
  const keepName = req.name.trim() !== '' && req.name !== 'New Request';
  const names = pathParamsInUrl(parsed.url);
  const pathParameters = names.map(n => req.pathParameters.find(p => p.name === n) ?? { name: n, value: '' });
  return {
    ...req,
    name: keepName ? req.name : parsed.name,
    method: parsed.method,
    url: parsed.url,
    parameters: parsed.parameters,
    pathParameters,
    headers: parsed.headers,
    body: parsed.body,
    authentication: parsed.authentication,
  };
}

function clipboardPlainText(cd: DataTransfer): string {
  for (const kind of ['text/plain', 'text', 'Text']) {
    try {
      const value = cd.getData(kind);
      if (value) return value;
    } catch {
      // WKWebView can throw when asked for a type it did not put on the clipboard.
    }
  }
  return '';
}

export function RequestView({ id, onRenamed }: { id: string; onRenamed: () => void }) {
  const toast = useToast();
  const [req, setReq, flush] = useAutosave<Request>(id, api.requestUpdate);
  const [tab, setTab] = useState('params');
  const [scriptPhase, setScriptPhase] = useState<'pre' | 'after'>('pre');
  const [sending, setSending] = useState(false);
  const [codeOpen, setCodeOpen] = useState(false);
  const [response, setResponse] = useState<ResponseView | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const urlRef = useRef<HTMLInputElement>(null);
  const curlImporting = useRef(false);

  // Latest stored response on open.
  useEffect(() => {
    setResponse(null);
    api.responseList(id).then(list => list[0] && api.responseGet(list[0].id).then(setResponse));
  }, [id]);

  // Live URL preview.
  useEffect(() => {
    if (!req) return;
    const t = setTimeout(() => {
      api.renderPreview(id, req.url).then(setPreview).catch(() => setPreview(null));
    }, 200);
    return () => clearTimeout(t);
  }, [id, req?.url, req]);

  const update = (patch: Partial<Request>) => req && setReq({ ...req, ...patch });

  const importCurlText = (text: string) => {
    if (!req || curlImporting.current) return;
    curlImporting.current = true;
    void (async () => {
      try {
        const parsed = await api.curlParse(text);
        const keepName = req.name.trim() !== '' && req.name !== 'New Request';
        setReq(applyCurlImport(req, parsed));
        if (!keepName) setTimeout(onRenamed, 500);
        toast('Imported from cURL', 'success');
      } catch (err) {
        toast(`Could not parse cURL: ${errorText(err)}`, 'error');
        // The controlled input snaps back if the command already landed in the field.
        setReq({ ...req });
      } finally {
        requestAnimationFrame(() => {
          curlImporting.current = false;
        });
      }
    })();
  };

  const setUrl = (url: string) => {
    if (!req || curlImporting.current) return;
    if (looksLikeCurl(url)) {
      importCurlText(url);
      return;
    }
    const names = pathParamsInUrl(url);
    const pathParameters = names.map(n => req.pathParameters.find(p => p.name === n) ?? { name: n, value: '' });
    setReq({ ...req, url, pathParameters });
  };

  const send = useCallback(async () => {
    if (!req || sending) return;
    setSending(true);
    try {
      await flush();
      const r = await api.requestSend(req.id);
      setResponse(r);
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setSending(false);
    }
  }, [req, sending, flush, toast]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        send();
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'l') {
        e.preventDefault();
        urlRef.current?.focus();
        urlRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [send]);

  const onUrlPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    if (!req) return;
    const text = clipboardPlainText(e.clipboardData);
    if (text && looksLikeCurl(text)) {
      e.preventDefault();
      importCurlText(text);
      return;
    }
    // WKWebView sometimes inserts the paste while getData returns nothing.
    // A single-line command still round-trips through the input; multiline needs the clipboard text above.
    if (!text) {
      const input = e.currentTarget;
      requestAnimationFrame(() => {
        if (looksLikeCurl(input.value)) importCurlText(input.value);
      });
      return;
    }
    const { base, params } = splitQuery(text.trim());
    if (params.length && !req.url) {
      e.preventDefault();
      setReq({ ...req, url: base, parameters: [...req.parameters, ...params] });
    }
  };

  if (!req) return <Empty title="Loading…" />;

  const enabledCount = (l: KeyValue[]) => l.filter(x => !x.disabled && x.name).length;
  const mime = req.body.mimeType ?? '';
  const bodyParams: KeyValue[] = req.body.params.map(p => ({ ...p, value: p.value ?? '' }));
  const setBody = (b: Partial<Body>) => update({ body: { ...req.body, ...b } });

  return (
    <Split direction="row" initial={640} min={360} storageKey="lsock-split-request">
      <div className="flex h-full min-h-0 flex-col">
        <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
          <Select
            aria-label="Method"
            value={req.method}
            onChange={e => update({ method: e.target.value })}
            className={cn('w-[92px] font-mono text-[12.5px] font-bold', methodColor(req.method))}
          >
            {METHODS.map(m => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
            {!METHODS.includes(req.method as (typeof METHODS)[number]) && <option value={req.method}>{req.method}</option>}
          </Select>
          <VarInput
            inputRef={urlRef}
            className="flex-1"
            value={req.url}
            contextId={id}
            onChange={setUrl}
            onEnter={send}
            onPaste={onUrlPaste}
            placeholder="https://api.example.com/users/:id — or paste a cURL command"
          />
          <Button variant="primary" onClick={send} loading={sending} title={`Send (${modKey()}+Enter)`}>
            {!sending && <Send className="size-3.5" />} Send
          </Button>
          <IconButton label="Generate code (curl, JavaScript, Python, Go, Rust…)" onClick={() => setCodeOpen(true)}>
            <FileCode2 className="size-4" />
          </IconButton>
          <CodeModal open={codeOpen} onClose={() => setCodeOpen(false)} requestId={id} requestName={req.name} />
        </div>
        {preview && req.url.includes('{') && (
          <div
            className={cn(
              'selectable shrink-0 truncate border-b border-app px-3 py-1 font-mono text-[11.5px]',
              preview.error ? 'text-rose-600 dark:text-rose-400' : 'text-muted',
            )}
            title={preview.rendered ?? preview.error ?? ''}
          >
            {preview.error ? `⚠ ${preview.error}` : `→ ${preview.rendered}`}
          </div>
        )}
        <Tabs
          value={tab}
          onChange={setTab}
          tabs={[
            { id: 'params', label: 'Params', count: enabledCount(req.parameters) + req.pathParameters.length },
            { id: 'body', label: 'Body', dot: !!mime },
            { id: 'auth', label: 'Auth', dot: req.authentication.type !== 'inherit' && req.authentication.type !== 'none' },
            { id: 'headers', label: 'Headers', count: enabledCount(req.headers) },
            { id: 'scripts', label: 'Scripts', dot: !!(req.preRequestScript?.trim() || req.afterResponseScript?.trim()) },
            { id: 'settings', label: 'Settings' },
            { id: 'docs', label: 'Docs', dot: !!req.description },
          ]}
        />
        <div className="min-h-0 flex-1 overflow-auto">
          {tab === 'params' && (
            <div>
              <SectionTitle>Query parameters</SectionTitle>
              <KeyValueEditor items={req.parameters} onChange={parameters => update({ parameters })} contextId={id} />
              {req.pathParameters.length > 0 && (
                <>
                  <SectionTitle>Path parameters</SectionTitle>
                  <KeyValueEditor
                    items={req.pathParameters}
                    onChange={pathParameters => update({ pathParameters })}
                    contextId={id}
                    readOnlyNames
                  />
                </>
              )}
              <p className="px-3 py-3 text-[12px] text-muted">
                Tip: use <code className="font-mono">/:name</code> in the URL for path parameters and type{' '}
                <code className="font-mono">{'{{'}</code> to insert a variable.
              </p>
            </div>
          )}
          {tab === 'body' && (
            <div className="flex h-full min-h-0 flex-col">
              <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5">
                <Select
                  aria-label="Body type"
                  value={BODY_TYPES.some(b => b.id === mime) ? mime : 'text/plain'}
                  onChange={e => {
                    const mimeType = e.target.value || null;
                    if (mimeType === 'application/graphql' && req.method === 'GET') update({ method: 'POST', body: { ...req.body, mimeType } });
                    else setBody({ mimeType });
                  }}
                  className="h-7 text-[12.5px]"
                >
                  {BODY_TYPES.map(b => (
                    <option key={b.id} value={b.id}>
                      {b.label}
                    </option>
                  ))}
                </Select>
                <div className="flex-1" />
                {mime === 'application/json' && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      const p = prettyJson(req.body.text ?? '');
                      if (p) setBody({ text: p });
                      else toast('Body is not valid JSON (templates are fine, but cannot be prettified)', 'error');
                    }}
                  >
                    <Braces className="size-3.5" /> Prettify
                  </Button>
                )}
              </div>
              {!mime && <Empty title="This request has no body">Choose a body type above.</Empty>}
              {mime === 'application/graphql' && (
                <GraphQLEditor key={id} requestId={id} text={req.body.text ?? ''} onChange={text => setBody({ text })} />
              )}
              {['application/json', 'text/plain', 'application/xml'].includes(mime) && (
                <CodeEditor
                  value={req.body.text ?? ''}
                  onChange={text => setBody({ text })}
                  language={mime === 'application/json' ? 'json' : 'text'}
                  placeholder={mime === 'application/json' ? '{\n  "name": "{{ _.user }}"\n}' : ''}
                />
              )}
              {mime === 'application/x-www-form-urlencoded' && (
                <KeyValueEditor
                  items={bodyParams}
                  onChange={params => setBody({ params: params.map(p => ({ ...p, value: p.value })) })}
                  contextId={id}
                />
              )}
              {mime === 'multipart/form-data' && <MultipartEditor body={req.body} onChange={setBody} contextId={id} />}
              {mime === 'application/octet-stream' && (
                <div className="flex flex-col gap-2 p-4">
                  <label className="text-muted">File path</label>
                  <VarInput value={req.body.fileName ?? ''} onChange={fileName => setBody({ fileName })} contextId={id} placeholder="/path/to/file.bin" />
                </div>
              )}
            </div>
          )}
          {tab === 'auth' && <AuthEditor auth={req.authentication} onChange={authentication => update({ authentication })} contextId={id} />}
          {tab === 'headers' && (
            <div>
              <KeyValueEditor items={req.headers} onChange={headers => update({ headers })} contextId={id} namePlaceholder="Header" />
              <p className="px-3 py-3 text-[12px] text-muted">Headers set on parent folders are added automatically; a header here with the same name wins.</p>
            </div>
          )}
          {tab === 'scripts' && (
            <div className="flex h-full min-h-0 flex-col">
              <div className="flex shrink-0 gap-1 border-b border-app px-2 py-1.5">
                {(['pre', 'after'] as const).map(p => {
                  const has = !!(p === 'pre' ? req.preRequestScript : req.afterResponseScript)?.trim();
                  return (
                    <button
                      key={p}
                      onClick={() => setScriptPhase(p)}
                      className={cn(
                        'flex items-center gap-1.5 rounded-md px-2.5 py-1 text-[12.5px]',
                        scriptPhase === p ? 'bg-accent-soft text-app font-medium' : 'text-muted hover:text-app',
                      )}
                    >
                      {p === 'pre' ? 'Pre-request' : 'After-response'}
                      {has && <span className="size-1.5 rounded-full bg-accent" />}
                    </button>
                  );
                })}
              </div>
              <ScriptEditor
                key={`${id}-${scriptPhase}`}
                phase={scriptPhase}
                value={(scriptPhase === 'pre' ? req.preRequestScript : req.afterResponseScript) ?? ''}
                onChange={v => update(scriptPhase === 'pre' ? { preRequestScript: v } : { afterResponseScript: v })}
              />
            </div>
          )}
          {tab === 'settings' && (
            <div className="flex max-w-xl flex-col gap-3 p-4">
              <Toggle label="Send cookies from the cookie jar" checked={req.settings.sendCookies} onChange={v => update({ settings: { ...req.settings, sendCookies: v } })} />
              <Toggle label="Store cookies from responses" checked={req.settings.storeCookies} onChange={v => update({ settings: { ...req.settings, storeCookies: v } })} />
              <Toggle label="URL-encode query parameters" checked={req.settings.encodeUrl} onChange={v => update({ settings: { ...req.settings, encodeUrl: v } })} />
              <Toggle
                label="Render variables in body"
                checked={!req.settings.disableRenderBody}
                onChange={v => update({ settings: { ...req.settings, disableRenderBody: !v } })}
              />
              <Toggle
                label="Send User-Agent header"
                checked={!req.settings.disableUserAgent}
                onChange={v => update({ settings: { ...req.settings, disableUserAgent: !v } })}
              />
              <label className="flex items-center gap-3">
                <span>Follow redirects</span>
                <Select
                  value={req.settings.followRedirects}
                  onChange={e => update({ settings: { ...req.settings, followRedirects: e.target.value as Request['settings']['followRedirects'] } })}
                >
                  <option value="global">Use global setting</option>
                  <option value="on">Always</option>
                  <option value="off">Never</option>
                </Select>
              </label>
            </div>
          )}
          {tab === 'docs' && (
            <div className="flex h-full flex-col gap-2 p-3">
              <Input value={req.name} onChange={e => update({ name: e.target.value })} onBlur={onRenamed} aria-label="Request name" />
              <textarea
                value={req.description}
                onChange={e => update({ description: e.target.value })}
                placeholder="Describe this request (Markdown)…"
                className="min-h-40 flex-1 resize-none rounded-md border border-app bg-app p-2.5 outline-none focus:border-accent"
              />
            </div>
          )}
        </div>
        <div className="flex h-7 shrink-0 items-center gap-3 border-t border-app px-3 text-[11px] text-muted">
          <span>
            <Kbd>{modKey()}</Kbd> <Kbd>↵</Kbd> send
          </span>
          <span>
            <Kbd>{modKey()}</Kbd> <Kbd>L</Kbd> focus URL
          </span>
          <span>
            <Kbd>{'{{'}</Kbd> variables
          </span>
        </div>
      </div>
      <ResponsePane requestId={id} response={response} sending={sending} onSelect={setResponse} />
    </Split>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return <div className="bg-subtle px-3 py-1.5 text-[11px] font-semibold tracking-wide text-muted uppercase">{children}</div>;
}

function MultipartEditor({ body, onChange, contextId }: { body: Body; onChange: (b: Partial<Body>) => void; contextId: string }) {
  const rows = [...body.params, { name: '', value: '', type: 'text' }];
  const set = (i: number, patch: Record<string, unknown>) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r)).filter((r, j) => j < rows.length - 1 || r.name || r.value || r.fileName);
    onChange({ params: next });
  };
  return (
    <div>
      {rows.map((r, i) => (
        <div key={i} className="flex items-center gap-1.5 border-b border-app px-2 py-1">
          <VarInput className="w-1/3" bare value={r.name} placeholder="field" onChange={v => set(i, { name: v })} contextId={contextId} />
          <Select className="h-7 w-20 text-[12px]" value={r.type === 'file' ? 'file' : 'text'} onChange={e => set(i, { type: e.target.value })}>
            <option value="text">Text</option>
            <option value="file">File</option>
          </Select>
          {r.type === 'file' ? (
            <VarInput className="flex-1" bare value={r.fileName ?? ''} placeholder="/path/to/file" onChange={v => set(i, { fileName: v })} contextId={contextId} />
          ) : (
            <VarInput className="flex-1" bare value={r.value} placeholder="value" onChange={v => set(i, { value: v })} contextId={contextId} />
          )}
        </div>
      ))}
    </div>
  );
}
