import { autocompletion, type CompletionContext } from '@codemirror/autocomplete';
import { javascript } from '@codemirror/lang-javascript';
import { EditorView } from '@codemirror/view';
import CodeMirror from '@uiw/react-codemirror';
import { BookOpen, ChevronDown } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import snippets from '../lib/script-snippets.json';
import { cn } from '../lib/utils';
import { useDark } from './editors';
import { Button } from './ui';

type Phase = 'pre' | 'after';

/** Ready-made examples, inserted at the cursor. */
const EXAMPLES: { label: string; phase: Phase | 'both'; code: string }[] = [
  {
    label: 'Test: status is 200',
    phase: 'after',
    code: "insomnia.test('status is 200', () => {\n  insomnia.response.to.have.status(200);\n});\n",
  },
  {
    label: 'Test: JSON body field',
    phase: 'after',
    code: "insomnia.test('has an id', () => {\n  const body = insomnia.response.json();\n  insomnia.expect(body).to.have.property('id');\n});\n",
  },
  {
    label: 'Save a token from the response',
    phase: 'after',
    code: "const { token } = insomnia.response.json();\ninsomnia.environment.set('token', token);\n",
  },
  {
    label: 'Add a header',
    phase: 'pre',
    code: "insomnia.request.headers.upsert({ key: 'X-Request-Id', value: require('uuid').v4() });\n",
  },
  {
    label: 'Sign the request (HMAC-SHA256)',
    phase: 'pre',
    code:
      "const CryptoJS = require('crypto-js');\nconst ts = Date.now().toString();\nconst sig = CryptoJS.HmacSHA256(ts + insomnia.request.body.raw, insomnia.environment.get('secret')).toString();\ninsomnia.request.headers.upsert({ key: 'X-Timestamp', value: ts });\ninsomnia.request.headers.upsert({ key: 'X-Signature', value: sig });\n",
  },
  {
    label: 'Fetch a token first (sendRequest)',
    phase: 'pre',
    code:
      "const res = await insomnia.sendRequest({\n  url: insomnia.environment.get('base_url') + '/oauth/token',\n  method: 'POST',\n  header: [{ key: 'Content-Type', value: 'application/json' }],\n  body: { mode: 'raw', raw: JSON.stringify({ client_id: insomnia.environment.get('client_id') }) },\n});\ninsomnia.variables.set('access_token', res.json().access_token);\n",
  },
  { label: 'Skip this request', phase: 'pre', code: 'insomnia.execution.skipRequest();\n' },
  {
    label: 'Poll until ready (runner)',
    phase: 'after',
    code: "if (insomnia.response.json().status !== 'ready') {\n  insomnia.execution.setNextRequest(insomnia.info.requestName);\n}\n",
  },
  { label: 'Log to the Console tab', phase: 'both', code: "console.log('value:', insomnia.variables.get('base_url'));\n" },
];

const completions = (snippets as { name: string; value: string; displayValue: string }[]).map(s => ({
  label: s.value.replace(/\(\)$/, ''),
  apply: s.value,
  type: s.value.endsWith(')') ? 'method' : 'property',
  detail: s.displayValue.endsWith(')') ? 'method' : '',
}));

function insomniaCompletions(ctx: CompletionContext) {
  const word = ctx.matchBefore(/(insomnia|\$|pm)(\.[\w]*)*\.?/);
  if (!word || (word.from === word.to && !ctx.explicit)) return null;
  const prefix = word.text.replace(/^(\$|pm)\./, 'insomnia.');
  return {
    from: word.from,
    options: completions
      .filter(c => c.label.startsWith(prefix.replace(/\.$/, '')) || c.label.startsWith(prefix))
      .map(c => ({ ...c, apply: word.text.startsWith('insomnia') ? c.apply : c.apply.replace(/^insomnia/, word.text.split('.')[0]) })),
    validFor: /^[\w.$]*$/,
  };
}

export function ScriptEditor({ value, onChange, phase }: { value: string; onChange: (v: string) => void; phase: Phase }) {
  const dark = useDark();
  const viewRef = useRef<EditorView | null>(null);
  const [menu, setMenu] = useState(false);
  const extensions = useMemo(
    () => [javascript(), EditorView.lineWrapping, autocompletion({ override: [insomniaCompletions], activateOnTyping: true })],
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
          theme={dark ? 'dark' : 'light'}
          extensions={extensions}
          height="100%"
          style={{ height: '100%' }}
          placeholder={phase === 'pre' ? "// e.g. insomnia.environment.set('ts', Date.now());" : "// e.g. insomnia.test('ok', () => insomnia.response.to.have.status(200));"}
          basicSetup={{ autocompletion: false, foldGutter: true, lineNumbers: true }}
        />
      </div>
      <div className="shrink-0 border-t border-app px-3 py-1 text-[11px] text-muted">
        Type <code className="font-mono">insomnia.</code> for completions · <code className="font-mono">require()</code>: chai, lodash, uuid, crypto-js, moment, tv4, ajv
      </div>
    </div>
  );
}
