import { KeyRound, LogIn, RefreshCw, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';

import { Badge, Button, Select, Toggle, useToast } from '../components/ui';
import { VarInput } from '../components/VarInput';
import { api, type Auth, errorText, type TokenStatus } from '../lib/api';

const LABELS: Record<Auth['type'], string> = {
  inherit: 'Inherit from parent folder',
  none: 'No auth',
  basic: 'Basic',
  bearer: 'Bearer token',
  apikey: 'API key',
  digest: 'Digest',
  oauth2: 'OAuth 2.0',
  oauth1: 'OAuth 1.0a',
  iam: 'AWS Signature V4 (IAM)',
  netrc: 'Netrc file',
};

function Field({ label, children, hint }: { label: string; children: React.ReactNode; hint?: React.ReactNode }) {
  return (
    <label className="grid grid-cols-[150px_1fr] items-start gap-3">
      <span className="pt-1.5 text-muted">{label}</span>
      <div className="flex flex-col gap-1">
        {children}
        {hint && <span className="text-[11.5px] text-muted">{hint}</span>}
      </div>
    </label>
  );
}

function defaults(t: Auth['type']): Auth {
  switch (t) {
    case 'basic':
      return { type: 'basic', username: '', password: '' };
    case 'bearer':
      return { type: 'bearer', token: '', prefix: null };
    case 'apikey':
      return { type: 'apikey', key: '', value: '', addTo: 'header' };
    case 'digest':
      return { type: 'digest', username: '', password: '' };
    case 'oauth2':
      return {
        type: 'oauth2',
        grantType: 'client_credentials',
        accessTokenUrl: '',
        authorizationUrl: '',
        clientId: '',
        clientSecret: '',
        scope: '',
        audience: '',
        resource: '',
        username: '',
        password: '',
        redirectUrl: 'http://127.0.0.1:8970/callback',
        usePkce: true,
        credentialsInBody: false,
        tokenPrefix: '',
      };
    case 'oauth1':
      return {
        type: 'oauth1',
        consumerKey: '',
        consumerSecret: '',
        tokenKey: '',
        tokenSecret: '',
        signatureMethod: 'HMAC-SHA1',
        realm: '',
        callback: '',
        verifier: '',
        nonce: '',
        timestamp: '',
        includeBodyHash: false,
      };
    case 'iam':
      return { type: 'iam', accessKeyId: '', secretAccessKey: '', sessionToken: '', region: 'us-east-1', service: 'execute-api' };
    default:
      return { type: t } as Auth;
  }
}

function OAuth2Token({ ownerId, grantType }: { ownerId: string; grantType: string }) {
  const toast = useToast();
  const [status, setStatus] = useState<TokenStatus | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.oauth2Status(ownerId).then(setStatus).catch(() => setStatus(null));
  }, [ownerId]);
  const get = async () => {
    setBusy(true);
    try {
      setStatus(await api.oauth2Authorize(ownerId));
      toast('Token received', 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setBusy(false);
    }
  };
  const expired = status?.expiresAt != null && status.expiresAt < Date.now();
  return (
    <div className="rounded-lg border border-app bg-subtle p-3">
      <div className="flex items-center gap-2">
        <KeyRound className="size-4 text-muted" />
        <span className="font-medium">Token</span>
        {status?.hasToken ? (
          <>
            <code className="font-mono text-[12px]">{status.preview}</code>
            {status.expiresAt != null && <Badge tone={expired ? 'amber' : 'green'}>{expired ? 'expired' : `expires ${new Date(status.expiresAt).toLocaleTimeString()}`}</Badge>}
            {status.hasRefreshToken && <Badge>refreshable</Badge>}
          </>
        ) : (
          <span className="text-[12.5px] text-muted">
            {grantType === 'authorization_code' ? 'none yet — click Get token to sign in in your browser' : 'none yet — fetched automatically on send, or click Get token'}
          </span>
        )}
        <div className="flex-1" />
        <Button size="sm" variant="primary" onClick={get} loading={busy}>
          {!busy && (status?.hasToken ? <RefreshCw className="size-3.5" /> : <LogIn className="size-3.5" />)} {status?.hasToken ? 'New token' : 'Get token'}
        </Button>
        {status?.hasToken && (
          <Button size="sm" variant="ghost" onClick={() => api.oauth2Clear(ownerId).then(() => api.oauth2Status(ownerId).then(setStatus))}>
            <Trash2 className="size-3.5" /> Clear
          </Button>
        )}
      </div>
      {status?.scope && <div className="mt-1 text-[12px] text-muted">scope: {status.scope}</div>}
    </div>
  );
}

export function AuthEditor({
  auth,
  onChange,
  contextId,
  allowInherit = true,
}: {
  auth: Auth;
  onChange: (a: Auth) => void;
  contextId: string;
  allowInherit?: boolean;
}) {
  const disabled = 'disabled' in auth && !!auth.disabled;
  const v = (value: string, set: (v: string) => void, placeholder?: string) => <VarInput contextId={contextId} value={value} onChange={set} placeholder={placeholder} />;
  return (
    <div className="flex max-w-2xl flex-col gap-3 p-4">
      <Field label="Type">
        <Select value={auth.type} onChange={e => onChange(defaults(e.target.value as Auth['type']))} className="w-72">
          {(Object.keys(LABELS) as Auth['type'][])
            .filter(t => allowInherit || t !== 'inherit')
            .map(t => (
              <option key={t} value={t}>
                {LABELS[t]}
              </option>
            ))}
        </Select>
      </Field>
      {auth.type === 'inherit' && <p className="text-[12.5px] text-muted">Uses the auth of the nearest parent folder that defines one. If none does, no auth is sent.</p>}
      {(auth.type === 'basic' || auth.type === 'digest') && (
        <>
          <Field label="Username">{v(auth.username, username => onChange({ ...auth, username }))}</Field>
          <Field label="Password" hint={auth.type === 'digest' ? 'Sent after the server’s 401 challenge (MD5, SHA-256 and -sess supported).' : undefined}>
            {v(auth.password, password => onChange({ ...auth, password }))}
          </Field>
        </>
      )}
      {auth.type === 'bearer' && (
        <>
          <Field label="Token">{v(auth.token, token => onChange({ ...auth, token }), '{{ _.token }}')}</Field>
          <Field label="Prefix">{v(auth.prefix ?? '', p => onChange({ ...auth, prefix: p || null }), 'Bearer')}</Field>
        </>
      )}
      {auth.type === 'apikey' && (
        <>
          <Field label="Key">{v(auth.key, key => onChange({ ...auth, key }), 'X-API-Key')}</Field>
          <Field label="Value">{v(auth.value, value => onChange({ ...auth, value }))}</Field>
          <Field label="Add to">
            <Select value={auth.addTo ?? 'header'} onChange={e => onChange({ ...auth, addTo: e.target.value })} className="w-64">
              <option value="header">Header</option>
              <option value="queryParams">Query parameter</option>
              <option value="cookie">Cookie</option>
            </Select>
          </Field>
        </>
      )}
      {auth.type === 'oauth2' && (
        <>
          <Field label="Grant type">
            <Select value={auth.grantType} onChange={e => onChange({ ...auth, grantType: e.target.value as typeof auth.grantType })} className="w-72">
              <option value="client_credentials">Client credentials</option>
              <option value="authorization_code">Authorization code (browser sign-in)</option>
              <option value="password">Resource owner password</option>
            </Select>
          </Field>
          {auth.grantType === 'authorization_code' && (
            <>
              <Field label="Authorization URL">{v(auth.authorizationUrl, authorizationUrl => onChange({ ...auth, authorizationUrl }), 'https://auth.example.com/authorize')}</Field>
              <Field label="Redirect URL" hint="Must be http://localhost or http://127.0.0.1 with a free port — insomnia-rs listens there during sign-in. Register it with your provider.">
                {v(auth.redirectUrl, redirectUrl => onChange({ ...auth, redirectUrl }))}
              </Field>
              <Field label="PKCE (S256)">
                <Toggle checked={auth.usePkce} onChange={usePkce => onChange({ ...auth, usePkce })} />
              </Field>
            </>
          )}
          <Field label="Access token URL">{v(auth.accessTokenUrl, accessTokenUrl => onChange({ ...auth, accessTokenUrl }), 'https://auth.example.com/oauth/token')}</Field>
          <Field label="Client ID">{v(auth.clientId, clientId => onChange({ ...auth, clientId }))}</Field>
          <Field label="Client secret">{v(auth.clientSecret, clientSecret => onChange({ ...auth, clientSecret }), '{{ _.client_secret }}')}</Field>
          {auth.grantType === 'password' && (
            <>
              <Field label="Username">{v(auth.username, username => onChange({ ...auth, username }))}</Field>
              <Field label="Password">{v(auth.password, password => onChange({ ...auth, password }))}</Field>
            </>
          )}
          <Field label="Scope">{v(auth.scope, scope => onChange({ ...auth, scope }), 'read write')}</Field>
          <Field label="Audience">{v(auth.audience, audience => onChange({ ...auth, audience }))}</Field>
          <Field label="Client credentials in body">
            <Toggle checked={auth.credentialsInBody} onChange={credentialsInBody => onChange({ ...auth, credentialsInBody })} label={auth.credentialsInBody ? 'in the form body' : 'HTTP Basic header'} />
          </Field>
          <Field label="Header prefix">{v(auth.tokenPrefix, tokenPrefix => onChange({ ...auth, tokenPrefix }), 'Bearer')}</Field>
          <OAuth2Token ownerId={contextId} grantType={auth.grantType} />
        </>
      )}
      {auth.type === 'oauth1' && (
        <>
          <Field label="Signature method">
            <Select value={auth.signatureMethod} onChange={e => onChange({ ...auth, signatureMethod: e.target.value as typeof auth.signatureMethod })} className="w-48">
              {['HMAC-SHA1', 'HMAC-SHA256', 'PLAINTEXT'].map(m => (
                <option key={m}>{m}</option>
              ))}
            </Select>
          </Field>
          <Field label="Consumer key">{v(auth.consumerKey, consumerKey => onChange({ ...auth, consumerKey }))}</Field>
          <Field label="Consumer secret">{v(auth.consumerSecret, consumerSecret => onChange({ ...auth, consumerSecret }))}</Field>
          <Field label="Token">{v(auth.tokenKey, tokenKey => onChange({ ...auth, tokenKey }))}</Field>
          <Field label="Token secret">{v(auth.tokenSecret, tokenSecret => onChange({ ...auth, tokenSecret }))}</Field>
          <Field label="Realm">{v(auth.realm, realm => onChange({ ...auth, realm }))}</Field>
          <Field label="Body hash">
            <Toggle checked={auth.includeBodyHash} onChange={includeBodyHash => onChange({ ...auth, includeBodyHash })} label="Add oauth_body_hash for non-form bodies" />
          </Field>
        </>
      )}
      {auth.type === 'iam' && (
        <>
          <Field label="Access key ID">{v(auth.accessKeyId, accessKeyId => onChange({ ...auth, accessKeyId }), '{{ _.aws_access_key_id }}')}</Field>
          <Field label="Secret access key">{v(auth.secretAccessKey, secretAccessKey => onChange({ ...auth, secretAccessKey }), '{{ _.aws_secret_access_key }}')}</Field>
          <Field label="Session token" hint="Only for temporary credentials (STS).">
            {v(auth.sessionToken, sessionToken => onChange({ ...auth, sessionToken }))}
          </Field>
          <Field label="Region">{v(auth.region, region => onChange({ ...auth, region }), 'us-east-1')}</Field>
          <Field label="Service">{v(auth.service, service => onChange({ ...auth, service }), 'execute-api, s3, lambda…')}</Field>
        </>
      )}
      {auth.type === 'netrc' && <p className="text-[12.5px] text-muted">Uses the login and password for the request host from ~/.netrc (or $NETRC) as HTTP Basic.</p>}
      {auth.type !== 'inherit' && auth.type !== 'none' && (
        <Field label="Enabled">
          <Toggle checked={!disabled} onChange={on => onChange({ ...auth, disabled: !on } as Auth)} />
        </Field>
      )}
    </div>
  );
}
