import { autocompletion, type CompletionContext } from '@codemirror/autocomplete';
import { editorTheme } from '../lib/editorTheme';
import { javascript } from '@codemirror/lang-javascript';
import { EditorView } from '@codemirror/view';
import CodeMirror from '@uiw/react-codemirror';
import { BookOpen, ChevronDown } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { SCRIPT_API } from '../lib/script-api';
import { cn } from '../lib/utils';
import { Button } from './ui';

type Phase = 'pre' | 'after';

/** Ready-made examples, inserted at the cursor. */
const EXAMPLES: { label: string; phase: Phase | 'both'; code: string }[] = [
  {
    label: 'Test: status is 200',
    phase: 'after',
    code: "ls.test('status is 200', () => {\n  ls.response.to.have.status(200);\n});\n",
  },
  {
    label: 'Test: JSON body field',
    phase: 'after',
    code: "ls.test('has an id', () => {\n  const body = ls.response.json();\n  ls.expect(body).to.have.property('id');\n});\n",
  },
  {
    label: 'Save a token from the response',
    phase: 'after',
    code: "const { token } = ls.response.json();\nls.environment.set('token', token);\n",
  },
  {
    label: 'Add a header',
    phase: 'pre',
    code: "ls.request.headers.upsert({ key: 'X-Request-Id', value: require('uuid').v4() });\n",
  },
  {
    label: 'Sign the request (HMAC-SHA256)',
    phase: 'pre',
    code:
      "const CryptoJS = require('crypto-js');\nconst ts = Date.now().toString();\nconst sig = CryptoJS.HmacSHA256(ts + ls.request.body.raw, ls.environment.get('secret')).toString();\nls.request.headers.upsert({ key: 'X-Timestamp', value: ts });\nls.request.headers.upsert({ key: 'X-Signature', value: sig });\n",
  },
  {
    label: 'Fetch a token first (sendRequest)',
    phase: 'pre',
    code:
      "const res = await ls.sendRequest({\n  url: ls.environment.get('base_url') + '/oauth/token',\n  method: 'POST',\n  header: [{ key: 'Content-Type', value: 'application/json' }],\n  body: { mode: 'raw', raw: JSON.stringify({ client_id: ls.environment.get('client_id') }) },\n});\nls.variables.set('access_token', res.json().access_token);\n",
  },
  {
    label: 'Fetch a token first (axios)',
    phase: 'pre',
    code:
      "const axios = require('axios');\nconst res = await axios.post(ls.environment.get('base_url') + '/oauth/token', {\n  client_id: ls.environment.get('client_id'),\n});\nls.variables.set('access_token', res.data.access_token);\n",
  },
  {
    label: 'Fetch a token first (curl)',
    phase: 'pre',
    code:
      "const curl = require('curl');\nconst res = await curl.post(ls.environment.get('base_url') + '/oauth/token', {\n  client_id: ls.environment.get('client_id'),\n});\nls.variables.set('access_token', JSON.parse(res.body).access_token);\n",
  },
  { label: 'Skip this request', phase: 'pre', code: 'ls.execution.skipRequest();\n' },
  {
    label: 'Poll until ready (runner)',
    phase: 'after',
    code: "if (ls.response.json().status !== 'ready') {\n  ls.execution.setNextRequest(ls.info.requestName);\n}\n",
  },
  { label: 'Log to the Console tab', phase: 'both', code: "console.log('value:', ls.variables.get('base_url'));\n" },
];

const completions = SCRIPT_API.map(e => ({
  label: e.path,
  apply: e.args ? `${e.path}()` : e.path,
  type: e.args ? 'method' : 'property',
  detail: e.args ?? '',
  info: e.doc,
}));

/** Completions for `ls.`, and the `pm.` / `$.` aliases. */
function scriptCompletions(ctx: CompletionContext) {
  const word = ctx.matchBefore(/(ls|\$|pm)(\.[\w]*)*\.?/);
  if (!word || (word.from === word.to && !ctx.explicit)) return null;
  const alias = word.text.split('.')[0];
  const prefix = word.text.replace(/^(\$|pm)\./, 'ls.');
  return {
    from: word.from,
    options: completions
      .filter(c => c.label.startsWith(prefix.replace(/\.$/, '')) || c.label.startsWith(prefix))
      .map(c => (alias === 'ls' ? c : { ...c, label: c.label.replace(/^ls/, alias), apply: c.apply.replace(/^ls/, alias) })),
    validFor: /^[\w.$]*$/,
  };
}

export function ScriptEditor({ value, onChange, phase }: { value: string; onChange: (v: string) => void; phase: Phase }) {
  const viewRef = useRef<EditorView | null>(null);
  const [menu, setMenu] = useState(false);
  const extensions = useMemo(
    () => [javascript(), EditorView.lineWrapping, autocompletion({ override: [scriptCompletions], activateOnTyping: true })],
    [],
  );
  useEffect(() => {
    const close = () => setMenu(false);
    window.addEventListener('click', close);
    return () => window.removeEventListener('click', close);
  }, []);

  const insert = (code: string) => {
    const v = viewRef.current;
    if (!v) return onChange((value ? `${value}\n` : '') + code);
    const pos = v.state.selection.main.head;
    v.dispatch({ changes: { from: pos, insert: code }, selection: { anchor: pos + code.length } });
    v.focus();
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-app px-3 py-1.5 text-[12px] text-muted">
        <span>
          {phase === 'pre'
            ? 'Runs before the request is sent. Change the request, set variables, or skip it.'
            : 'Runs after the response arrives. Write tests and save values for later requests.'}
        </span>
        <div className="flex-1" />
        <div className="relative">
          <Button
            size="sm"
            variant="ghost"
            onClick={e => {
              e.stopPropagation();
              setMenu(!menu);
            }}
          >
            <BookOpen className="size-3.5" /> Snippets <ChevronDown className="size-3" />
          </Button>
          {menu && (
            <div className="absolute top-full right-0 z-40 mt-1 w-72 rounded-lg border border-app bg-app py-1 shadow-xl">
              {EXAMPLES.filter(x => x.phase === phase || x.phase === 'both').map(x => (
                <button key={x.label} className="block w-full px-3 py-1.5 text-left text-[12.5px] text-app hover:bg-muted" onClick={() => insert(x.code)}>
                  {x.label}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
      <div className={cn('min-h-0 flex-1 overflow-hidden')}>
        <CodeMirror
          value={value}
          onChange={onChange}
          onCreateEditor={v => (viewRef.current = v)}
          theme={editorTheme}
          extensions={extensions}
          height="100%"
          style={{ height: '100%' }}
          placeholder={phase === 'pre' ? "// e.g. ls.environment.set('ts', Date.now());" : "// e.g. ls.test('ok', () => ls.response.to.have.status(200));"}
          basicSetup={{ autocompletion: false, foldGutter: true, lineNumbers: true }}
        />
      </div>
      <div className="shrink-0 border-t border-app px-3 py-1 text-[11px] text-muted">
        Type <code className="font-mono">ls.</code> for completions · <code className="font-mono">require()</code>: chai, lodash, uuid, crypto-js, moment, tv4, ajv, curl, axios, node-fetch, got, request, superagent
      </div>
    </div>
  );
}
