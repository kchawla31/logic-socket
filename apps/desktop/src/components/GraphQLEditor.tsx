// GraphQL body: query + variables editors, schema introspection with
// schema-aware completion/lint, and a schema explorer.
import { editorTheme } from '../lib/editorTheme';
import { EditorView } from '@codemirror/view';
import CodeMirror from '@uiw/react-codemirror';
import { graphql as graphqlLang, updateSchema } from 'cm6-graphql';
import {
  buildClientSchema,
  getIntrospectionQuery,
  getNamedType,
  type GraphQLField,
  type GraphQLNamedType,
  GraphQLObjectType,
  type GraphQLSchema,
  isInputObjectType,
  isInterfaceType,
  isObjectType,
  OperationDefinitionNode,
  parse,
  print,
} from 'graphql';
import { BookOpen, Braces, RefreshCw, Search, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { api, errorText } from '../lib/api';
import { cn, timeAgo } from '../lib/utils';
import { CodeEditor } from './editors';
import { Button, Select, Split, useToast } from './ui';

interface GqlBody {
  query: string;
  variables?: unknown;
  operationName?: string | null;
}

/** Schemas cached per request for this app session. */
const schemaCache = new Map<string, { schema: GraphQLSchema; at: number }>();

export function parseGqlBody(text: string | null | undefined): GqlBody {
  if (!text?.trim()) return { query: '' };
  try {
    const v = JSON.parse(text);
    if (v && typeof v === 'object' && typeof v.query === 'string') return v as GqlBody;
  } catch {
    /* not JSON */
  }
  return { query: text };
}

function operations(query: string): string[] {
  try {
    return parse(query)
      .definitions.filter((d): d is OperationDefinitionNode => d.kind === 'OperationDefinition')
      .map(d => d.name?.value)
      .filter((n): n is string => !!n);
  } catch {
    return [];
  }
}

type GraphQLOutputOrInput = Parameters<typeof getNamedType>[0] & { toString(): string };

function typeLabel(t: { toString(): string }): string {
  return t.toString();
}

function Explorer({ schema, onClose }: { schema: GraphQLSchema; onClose: () => void }) {
  const [stack, setStack] = useState<string[]>([]);
  const [q, setQ] = useState('');
  const current: GraphQLNamedType | null | undefined = stack.length ? schema.getType(stack[stack.length - 1]) : null;
  const roots = [schema.getQueryType(), schema.getMutationType(), schema.getSubscriptionType()].filter((t): t is GraphQLObjectType => !!t);
  const allTypes = Object.values(schema.getTypeMap()).filter(t => !t.name.startsWith('__'));
  const go = (name: string) => setStack(s => [...s, name]);
  const TypeLink = ({ t }: { t: GraphQLOutputOrInput }) => {
    const named = getNamedType(t) as GraphQLNamedType;
    return (
      <button className="font-mono text-amber-700 hover:underline dark:text-amber-400" onClick={() => go(named.name)}>
        {typeLabel(t)}
      </button>
    );
  };
  const fields: GraphQLField<unknown, unknown>[] =
    current && (isObjectType(current) || isInterfaceType(current)) ? Object.values(current.getFields()) : [];
  const inputFields = current && isInputObjectType(current) ? Object.values(current.getFields()) : [];
  return (
    <div className="flex h-full min-h-0 flex-col bg-subtle">
      <div className="flex shrink-0 items-center gap-1.5 border-b border-app px-2 py-1.5">
        {stack.length > 0 && (
          <button className="text-[12px] text-accent" onClick={() => setStack(s => s.slice(0, -1))}>
            ← {stack.length > 1 ? stack[stack.length - 2] : 'Schema'}
          </button>
        )}
        <span className="flex-1 truncate font-medium">{current?.name ?? 'Schema'}</span>
        <button aria-label="Close docs" onClick={onClose} className="text-muted hover:text-app">
          <X className="size-3.5" />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2.5 text-[12.5px]">
        {!current ? (
          <>
            {roots.map(r => (
              <div key={r.name} className="mb-1">
                <button className="font-mono font-semibold text-violet-700 hover:underline dark:text-violet-300" onClick={() => go(r.name)}>
                  {r.name}
                </button>
                <span className="ml-1.5 text-muted">{Object.keys(r.getFields()).length} fields</span>
              </div>
            ))}
            <div className="relative mt-3 mb-1.5">
              <Search className="pointer-events-none absolute top-1.5 left-2 size-3.5 text-muted" />
              <input value={q} onChange={e => setQ(e.target.value)} placeholder={`Search ${allTypes.length} types`} className="h-7 w-full rounded border border-app bg-app pl-7 text-[12px] outline-none" />
            </div>
            {allTypes
              .filter(t => !q || t.name.toLowerCase().includes(q.toLowerCase()))
              .slice(0, 200)
              .map(t => (
                <button key={t.name} className="block py-0.5 font-mono text-amber-700 hover:underline dark:text-amber-400" onClick={() => go(t.name)}>
                  {t.name}
                </button>
              ))}
          </>
        ) : (
          <>
            {current.description && <p className="mb-2 text-muted">{current.description}</p>}
            {fields.map(f => (
              <div key={f.name} className="mb-2">
                <div>
                  <span className="font-mono font-medium text-sky-700 dark:text-sky-400">{f.name}</span>
                  {f.args.length > 0 && (
                    <span className="font-mono text-muted">
                      (
                      {f.args.map((a, i) => (
                        <span key={a.name}>
                          {i > 0 && ', '}
                          {a.name}: <TypeLink t={a.type} />
                        </span>
                      ))}
                      )
                    </span>
                  )}
                  <span className="text-muted">: </span>
                  <TypeLink t={f.type} />
                  {f.deprecationReason && <span className="ml-1 text-[11px] text-rose-600">deprecated</span>}
                </div>
                {f.description && <div className="text-[12px] text-muted">{f.description}</div>}
              </div>
            ))}
            {inputFields.map(f => (
              <div key={f.name} className="mb-1.5 font-mono">
                <span className="text-sky-700 dark:text-sky-400">{f.name}</span>: <TypeLink t={f.type} />
              </div>
            ))}
            {'getValues' in current && (current as unknown as { getValues(): { name: string; description?: string | null }[] }).getValues().map(v => (
              <div key={v.name} className="font-mono text-emerald-700 dark:text-emerald-400">
                {v.name}
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

export function GraphQLEditor({ requestId, text, onChange }: { requestId: string; text: string; onChange: (text: string) => void }) {
  const toast = useToast();
  const body = useMemo(() => parseGqlBody(text), [text]);
  const [varsText, setVarsText] = useState(() => (body.variables ? JSON.stringify(body.variables, null, 2) : ''));
  const [varsError, setVarsError] = useState<string | null>(null);
  const [cached, setCached] = useState(() => schemaCache.get(requestId) ?? null);
  const [loading, setLoading] = useState(false);
  const [docs, setDocs] = useState(false);
  const view = useRef<EditorView | null>(null);
  const extensions = useMemo(() => [graphqlLang(cached?.schema), EditorView.lineWrapping], []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (view.current && cached) updateSchema(view.current, cached.schema);
  }, [cached]);

  const write = (next: GqlBody) => {
    const out: GqlBody = { query: next.query };
    if (next.variables !== undefined && next.variables !== null && !(typeof next.variables === 'object' && Object.keys(next.variables as object).length === 0)) out.variables = next.variables;
    if (next.operationName) out.operationName = next.operationName;
    onChange(JSON.stringify(out, null, 2));
  };

  const fetchSchema = async () => {
    setLoading(true);
    try {
      const res = await api.graphqlQuery(requestId, getIntrospectionQuery());
      const schema = buildClientSchema(res.data as never);
      const entry = { schema, at: Date.now() };
      schemaCache.set(requestId, entry);
      setCached(entry);
      toast(`Schema loaded — ${Object.keys(schema.getTypeMap()).filter(t => !t.startsWith('__')).length} types`, 'success');
    } catch (e) {
      toast(`Could not fetch schema: ${errorText(e)}`, 'error');
    } finally {
      setLoading(false);
    }
  };

  const ops = operations(body.query);
  const prettify = () => {
    try {
      write({ ...body, query: print(parse(body.query)) });
    } catch (e) {
      toast(`Query has a syntax error: ${errorText(e)}`, 'error');
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5 text-[12px]">
        <Button size="sm" variant="ghost" onClick={fetchSchema} loading={loading}>
          {!loading && <RefreshCw className="size-3.5" />} {cached ? 'Refresh schema' : 'Fetch schema'}
        </Button>
        {cached && <span className="text-muted">schema {timeAgo(cached.at)}</span>}
        {ops.length > 1 && (
          <Select className="h-7 text-[12px]" value={body.operationName ?? ''} onChange={e => write({ ...body, operationName: e.target.value || null })}>
            <option value="">Operation…</option>
            {ops.map(o => (
              <option key={o}>{o}</option>
            ))}
          </Select>
        )}
        <div className="flex-1" />
        <Button size="sm" variant="ghost" onClick={prettify}>
          <Braces className="size-3.5" /> Prettify
        </Button>
        <Button size="sm" variant="ghost" onClick={() => (cached ? setDocs(!docs) : fetchSchema().then(() => setDocs(true)))}>
          <BookOpen className="size-3.5" /> Docs
        </Button>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          <Split direction="col" initial={300} min={100} storageKey="lsock-split-gql">
            <div className={cn('h-full min-h-0 overflow-hidden')}>
              <CodeMirror
                value={body.query}
                onChange={query => write({ ...body, query, operationName: operations(query).includes(body.operationName ?? '') ? body.operationName : null })}
                onCreateEditor={v => {
                  view.current = v;
                  if (cached) updateSchema(v, cached.schema);
                }}
                theme={editorTheme}
                extensions={extensions}
                height="100%"
                style={{ height: '100%' }}
                placeholder={'query {\n  me { id name }\n}'}
                basicSetup={{ autocompletion: true, lineNumbers: true, foldGutter: true }}
              />
            </div>
            <div className="flex h-full min-h-0 flex-col">
              <div className="flex shrink-0 items-center gap-2 border-b border-app bg-subtle px-3 py-1 text-[11px] font-semibold tracking-wide text-muted uppercase">
                Variables
                {varsError && <span className="font-normal normal-case text-rose-600">{varsError}</span>}
              </div>
              <CodeEditor
                value={varsText}
                placeholder={'{\n  "id": "{{ _.user_id }}"\n}'}
                onChange={t => {
                  setVarsText(t);
                  if (!t.trim()) {
                    setVarsError(null);
                    write({ ...body, variables: undefined });
                    return;
                  }
                  try {
                    const v = JSON.parse(t);
                    setVarsError(null);
                    write({ ...body, variables: v });
                  } catch {
                    setVarsError(t.includes('{{') ? null : 'invalid JSON');
                  }
                }}
              />
            </div>
          </Split>
        </div>
        {docs && cached && (
          <div className="w-80 shrink-0 border-l border-app">
            <Explorer schema={cached.schema} onClose={() => setDocs(false)} />
          </div>
        )}
      </div>
    </div>
  );
}
