import { AlertTriangle, Download, FileUp, FolderOpen, Link2, ClipboardPaste, FileCode2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { CodeEditor } from '../components/editors';
import { Badge, Button, CopyButton, Input, Modal, Tabs, Toggle, useToast } from '../components/ui';
import { api, type CodeTargetId, errorText, type ExportFormat, type ImportPreview } from '../lib/api';
import { cn } from '../lib/utils';

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

const SOURCES = 'Insomnia (v4, v5), Postman collections & environments, OpenAPI 3, Swagger 2, HAR, or cURL commands';

export function ImportModal({
  open,
  onClose,
  workspaceId,
  workspaceName,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  workspaceId: string | null;
  workspaceName?: string;
  onImported: (workspaceId: string) => void;
}) {
  const toast = useToast();
  const [tab, setTab] = useState('file');
  const [text, setText] = useState('');
  const [fileName, setFileName] = useState<string | null>(null);
  const [url, setUrl] = useState('');
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [into, setInto] = useState(false);
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setText('');
    setFileName(null);
    setUrl('');
    setPreview(null);
    setError(null);
    setInto(false);
    setReplace(false);
  }, [open]);

  // preview whenever the input changes
  useEffect(() => {
    if (!text.trim()) {
      setPreview(null);
      setError(null);
      return;
    }
    const t = setTimeout(() => {
      api
        .importPreview(text)
        .then(p => {
          setPreview(p);
          setError(null);
          setReplace(p.workspaces.some(w => w.exists));
          if (p.format === 'postman-environment') setInto(!!workspaceId);
        })
        .catch(e => {
          setPreview(null);
          setError(errorText(e));
        });
    }, 250);
    return () => clearTimeout(t);
  }, [text, workspaceId]);

  const readFile = async (f: File) => {
    setFileName(f.name);
    setText(await f.text());
  };

  const fetchUrl = async () => {
    if (!url.trim()) return;
    setBusy(true);
    try {
      setText(await api.fetchText(url.trim()));
      setFileName(url.trim());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const doImport = async () => {
    setBusy(true);
    try {
      const s = await api.importApply(text, into ? workspaceId : null, replace && !into);
      toast(`Imported ${s.requests} request${s.requests === 1 ? '' : 's'} into ${s.workspaces.join(', ')}`, 'success');
      onImported(s.workspaceIds[0]);
      onClose();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  const exists = preview?.workspaces.some(w => w.exists);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Import"
      width="max-w-2xl"
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!preview} loading={busy} onClick={doImport}>
            <Download className="size-3.5" /> Import
          </Button>
        </>
      }
    >
      <Tabs
        tabs={[
          { id: 'file', label: <span className="flex items-center gap-1.5"><FileUp className="size-3.5" /> File</span> },
          { id: 'paste', label: <span className="flex items-center gap-1.5"><ClipboardPaste className="size-3.5" /> Paste</span> },
          { id: 'url', label: <span className="flex items-center gap-1.5"><Link2 className="size-3.5" /> URL</span> },
        ]}
        value={tab}
        onChange={setTab}
      />
      <div className="flex flex-col gap-3 p-4">
        {tab === 'file' && (
          <div
            onDragOver={e => (e.preventDefault(), setDragging(true))}
            onDragLeave={() => setDragging(false)}
            onDrop={e => {
              e.preventDefault();
              setDragging(false);
              const f = e.dataTransfer.files[0];
              if (f) readFile(f);
            }}
            onClick={() => fileRef.current?.click()}
            className={cn(
              'flex cursor-pointer flex-col items-center gap-2 rounded-lg border-2 border-dashed px-6 py-8 text-center text-muted transition-colors',
              dragging ? 'border-accent bg-accent-soft' : 'border-app hover:border-accent',
            )}
          >
            <FileUp className="size-7 opacity-70" />
            <div className="font-medium text-app">{fileName ?? 'Drop a file here or click to choose'}</div>
            <div className="text-[12px]">{SOURCES}</div>
            <input
              ref={fileRef}
              type="file"
              className="hidden"
              accept=".json,.yaml,.yml,.har,.txt,.sh"
              onChange={e => e.target.files?.[0] && readFile(e.target.files[0])}
            />
          </div>
        )}
        {tab === 'paste' && (
          <div className="h-56 overflow-hidden rounded-md border border-app">
            <CodeEditor value={text} onChange={setText} language="text" placeholder={`Paste ${SOURCES}`} />
          </div>
        )}
        {tab === 'url' && (
          <div className="flex gap-2">
            <Input value={url} onChange={e => setUrl(e.target.value)} onKeyDown={e => e.key === 'Enter' && fetchUrl()} placeholder="https://petstore3.swagger.io/api/v3/openapi.json" />
            <Button onClick={fetchUrl} loading={busy}>
              Fetch
            </Button>
          </div>
        )}

        {error && <div className="rounded-md border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-[12.5px] text-rose-700 dark:text-rose-300">{error}</div>}

        {preview && (
          <div className="rounded-lg border border-app">
            <div className="flex items-center gap-2 border-b border-app px-3 py-2">
              <Badge tone="violet">{preview.formatLabel}</Badge>
              <span className="text-[12.5px] text-muted">detected</span>
            </div>
            {preview.workspaces.map((w, i) => (
              <div key={i} className="flex items-center gap-3 px-3 py-2">
                <span className="font-medium">{w.name}</span>
                {w.exists && <Badge tone="amber">already here</Badge>}
                <span className="flex-1" />
                <span className="text-[12px] text-muted">
                  {plural(w.requests, 'request')} · {plural(w.folders, 'folder')} · {plural(w.environments, 'environment')}
                </span>
              </div>
            ))}
            {preview.warnings.length > 0 && (
              <div className="flex flex-col gap-1 border-t border-app bg-amber-500/5 px-3 py-2 text-[12px] text-amber-800 dark:text-amber-300">
                {preview.warnings.map((w, i) => (
                  <div key={i} className="flex gap-1.5">
                    <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {w}
                  </div>
                ))}
              </div>
            )}
            <div className="flex flex-col gap-2 border-t border-app px-3 py-2.5">
              {workspaceId && (
                <Toggle checked={into} onChange={setInto} label={`Add to the current collection${workspaceName ? ` (${workspaceName})` : ''} instead of creating a new one`} />
              )}
              {exists && !into && <Toggle checked={replace} onChange={setReplace} label="Update the existing collection in place (keeps your secrets and history)" />}
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

const FORMATS: { id: ExportFormat; label: string; hint: string }[] = [
  { id: 'insomnia-v5', label: 'Insomnia v5 (YAML)', hint: 'Everything, including gRPC, WebSocket, MCP and AI requests. Opens in Insomnia too.' },
  { id: 'postman', label: 'Postman collection v2.1', hint: 'HTTP requests, folders, auth, scripts (insomnia.* → pm.*) and variables.' },
  { id: 'har', label: 'HAR', hint: 'HTTP requests only, as an HTTP Archive.' },
];

export function ExportModal({ open, onClose, workspaceId, workspaceName }: { open: boolean; onClose: () => void; workspaceId: string; workspaceName: string }) {
  const toast = useToast();
  const [format, setFormat] = useState<ExportFormat>('insomnia-v5');
  const [includePrivate, setIncludePrivate] = useState(false);
  const [includeCookies, setIncludeCookies] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [content, setContent] = useState('');
  const [warnings, setWarnings] = useState<string[]>([]);

  useEffect(() => {
    if (!open) return;
    setSaved(null);
    api
      .exportWorkspace(workspaceId, format, includePrivate, includeCookies)
      .then(x => {
        setContent(x.content);
        setWarnings(x.warnings);
      })
      .catch(e => toast(errorText(e), 'error'));
  }, [open, workspaceId, format, includePrivate, includeCookies, toast]);

  const save = async () => {
    setBusy(true);
    try {
      const x = await api.exportWorkspace(workspaceId, format, includePrivate, includeCookies);
      const path = await api.saveToDownloads(x.fileName, x.content);
      setSaved(path);
      toast(`Saved to ${path}`, 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Export “${workspaceName}”`}
      width="max-w-2xl"
      footer={
        <>
          {saved && (
            <Button variant="ghost" onClick={() => api.revealPath(saved)}>
              <FolderOpen className="size-3.5" /> Show in Finder
            </Button>
          )}
          <CopyButton text={content} label="Copy to clipboard" />
          <Button variant="primary" loading={busy} onClick={save}>
            <Download className="size-3.5" /> Save to Downloads
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 p-4">
        <div className="flex flex-col gap-1.5">
          {FORMATS.map(f => (
            <label key={f.id} className={cn('flex cursor-pointer items-start gap-2.5 rounded-md border px-3 py-2', format === f.id ? 'border-accent bg-accent-soft' : 'border-app hover:bg-muted')}>
              <input type="radio" className="mt-1" checked={format === f.id} onChange={() => setFormat(f.id)} />
              <span>
                <span className="font-medium">{f.label}</span>
                <span className="block text-[12px] text-muted">{f.hint}</span>
              </span>
            </label>
          ))}
        </div>
        <div className="flex flex-wrap gap-4">
          <Toggle checked={includePrivate} onChange={setIncludePrivate} label="Include private environments" />
          <Toggle checked={includeCookies} onChange={setIncludeCookies} label="Include cookies" />
        </div>
        {warnings.map((w, i) => (
          <div key={i} className="flex gap-1.5 text-[12px] text-amber-700 dark:text-amber-300">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {w}
          </div>
        ))}
        <div className="h-56 overflow-hidden rounded-md border border-app">
          <CodeEditor value={content} readOnly language={format === 'insomnia-v5' ? 'text' : 'json'} />
        </div>
      </div>
    </Modal>
  );
}

const LANG_KEY = 'irs-code-lang';

export function CodeModal({ open, onClose, requestId, requestName }: { open: boolean; onClose: () => void; requestId: string; requestName: string }) {
  const [targets, setTargets] = useState<{ id: CodeTargetId; label: string }[]>([]);
  const [target, setTarget] = useState<CodeTargetId>(() => {
    try {
      return (localStorage.getItem(LANG_KEY) as CodeTargetId) || 'curl';
    } catch {
      return 'curl';
    }
  });
  const [code, setCode] = useState('');
  const [notes, setNotes] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open && !targets.length) api.codeTargets().then(setTargets);
  }, [open, targets.length]);
  useEffect(() => {
    if (!open) return;
    try {
      localStorage.setItem(LANG_KEY, target);
    } catch {
      /* ignore */
    }
    api
      .codeGenerate(requestId, target)
      .then(r => {
        setCode(r.code);
        setNotes(r.notes);
        setError(null);
      })
      .catch(e => {
        setCode('');
        setError(errorText(e));
      });
  }, [open, requestId, target]);

  return (
    <Modal open={open} onClose={onClose} title={<span className="flex items-center gap-2"><FileCode2 className="size-4" /> Code for “{requestName}”</span>} width="max-w-3xl" footer={<CopyButton text={code} label="Copy code" />}>
      <Tabs tabs={targets.map(t => ({ id: t.id, label: t.label }))} value={target} onChange={v => setTarget(v as CodeTargetId)} />
      <div className="flex flex-col gap-2 p-3">
        <p className="text-[12px] text-muted">Rendered with the active environment, inherited headers and auth — exactly what Send would do. Secrets appear in plain text, so share carefully.</p>
        {error && <div className="text-[12.5px] text-rose-600 dark:text-rose-400">{error}</div>}
        {notes.map((n, i) => (
          <div key={i} className="flex gap-1.5 text-[12px] text-amber-700 dark:text-amber-300">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {n}
          </div>
        ))}
        <div className="h-80 overflow-hidden rounded-md border border-app">
          <CodeEditor value={code} readOnly language={target === 'js-fetch' ? 'javascript' : 'text'} />
        </div>
      </div>
    </Modal>
  );
}
