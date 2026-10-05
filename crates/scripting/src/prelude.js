// insomnia-rs script runtime: re-implements the `insomnia.*` object model
// (packages/insomnia-scripting-environment) on top of a few host functions:
//   __host.log(level, text)            console capture
//   __host.render(template, varsJson)  Nunjucks-compatible rendering (Rust)
//   __host.sleep(ms) -> Promise        timers
//   __host.send(requestJson) -> Promise<responseJson>   HTTP via the Rust engine
//   __host.uuid() -> string            v4 UUID
//   __host.load(name) -> exports       evaluate a vendored library
'use strict';

(function () {
  const host = globalThis.__host;
  const hasOwn = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
  const clone = v => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));

  // ---------------------------------------------------------------- console
  function fmt(v) {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return v.stack ? `${v.name}: ${v.message}` : String(v);
    if (v === undefined) return 'undefined';
    if (typeof v === 'function') return `[Function ${v.name || 'anonymous'}]`;
    try {
      return JSON.stringify(v, null, 2);
    } catch (_) {
      return String(v);
    }
  }
  const scriptConsole = {};
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    scriptConsole[level] = (...args) => host.log(level, args.map(fmt).join(' '));
  }
  // libraries (e.g. ajv's default logger) expect a global console
  globalThis.console = scriptConsole;

  // ---------------------------------------------------------------- modules
  const moduleCache = {};
  const VENDORED = ['chai', 'lodash', 'crypto-js', 'moment', 'tv4', 'ajv'];
  const NODE_BUILTINS = [
    'fs', 'path', 'url', 'querystring', 'util', 'buffer', 'events', 'stream', 'assert', 'timers', 'punycode',
    'string_decoder', 'os', 'child_process', 'net', 'http', 'https', 'crypto', 'zlib',
  ];
  const uuidModule = {
    v4: () => host.uuid(),
    validate: s => typeof s === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(s),
    NIL: '00000000-0000-0000-0000-000000000000',
    version: s => parseInt(String(s).charAt(14), 16),
  };
  const loading = new Set();
  function scriptRequire(name) {
    if (hasOwn(moduleCache, name)) return moduleCache[name];
    // lodash reads the global `_` (our lazy getter) while initialising
    if (loading.has(name)) return undefined;
    let mod;
    if (VENDORED.includes(name)) {
      loading.add(name);
      try {
        mod = host.load(name);
      } finally {
        loading.delete(name);
      }
      if (name === 'chai') installChaiPlugins(mod);
    } else if (name === 'uuid') mod = uuidModule;
    else if (name === 'atob') mod = globalThis.atob;
    else if (name === 'btoa') mod = globalThis.btoa;
    else if (NODE_BUILTINS.includes(name) || name.startsWith('node:'))
      throw new Error(`NotSupported: require('${name}') — Node built-in modules are not available in the insomnia-rs sandbox`);
    else if (name === 'cheerio' || name === 'xml2js')
      throw new Error(`NotSupported: require('${name}') — needs a Node runtime; not available in the insomnia-rs sandbox`);
    else throw new Error(`Cannot find module '${name}'. Available: chai, lodash, uuid, crypto-js, moment, tv4, ajv, atob, btoa`);
    moduleCache[name] = mod;
    return mod;
  }

  // latin1 base64, like browsers
  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  globalThis.btoa = function (s) {
    s = String(s);
    let out = '';
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new Error('btoa: string contains characters outside of the Latin1 range');
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63] + (isNaN(b) ? '=' : B64[(n >> 6) & 63]) + (isNaN(c) ? '=' : B64[n & 63]);
    }
    return out;
  };
  globalThis.atob = function (s) {
    s = String(s).replace(/[\s=]/g, '');
    let out = '';
    let buf = 0, bits = 0;
    for (const ch of s) {
      const v = B64.indexOf(ch);
      if (v < 0) throw new Error('atob: invalid base64 input');
      buf = (buf << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((buf >> bits) & 255);
      }
    }
    return out;
  };

  // ---------------------------------------------------------------- timers
  let timerSeq = 0;
  const cancelled = new Set();
  const pendingTimers = new Set();
  function setTimeoutImpl(fn, ms, ...args) {
    const id = ++timerSeq;
    const p = host.sleep(Math.max(0, Number(ms) || 0)).then(() => {
      pendingTimers.delete(p);
      if (!cancelled.has(id) && typeof fn === 'function') fn(...args);
    });
    pendingTimers.add(p);
    return id;
  }
  function clearTimeoutImpl(id) {
    cancelled.add(id);
  }

  // ---------------------------------------------------------------- property lists
  class PropertyList {
    constructor(items, keyName = 'key') {
      this._items = (items || []).map(x => ({ ...x }));
      this._k = keyName;
    }
    _norm(item) {
      if (typeof item === 'string') {
        const i = item.indexOf(':');
        return i < 0 ? { key: item.trim(), value: '' } : { key: item.slice(0, i).trim(), value: item.slice(i + 1).trim() };
      }
      return { key: item.key ?? item.name, value: item.value === undefined ? '' : String(item.value), disabled: !!item.disabled };
    }
    _eq(a, b) {
      return String(a).toLowerCase() === String(b).toLowerCase();
    }
    add(item) {
      this._items.push(this._norm(item));
    }
    append(item) {
      this.add(item);
    }
    upsert(item) {
      const n = this._norm(item);
      const found = this._items.find(x => this._eq(x.key, n.key));
      // keep the existing key's casing (header names are case-insensitive)
      if (found) Object.assign(found, n, { key: found.key });
      else this._items.push(n);
    }
    remove(keyOrPredicate) {
      const pred = typeof keyOrPredicate === 'function' ? keyOrPredicate : x => this._eq(x.key, keyOrPredicate?.key ?? keyOrPredicate);
      this._items = this._items.filter(x => !pred(x));
    }
    clear() {
      this._items = [];
    }
    get(key) {
      const f = this._items.find(x => !x.disabled && this._eq(x.key, key));
      return f ? f.value : undefined;
    }
    one(key) {
      return this._items.find(x => this._eq(x.key, key));
    }
    has(key, value) {
      return this._items.some(x => !x.disabled && this._eq(x.key, key) && (value === undefined || x.value === value));
    }
    all() {
      return this._items.map(x => ({ ...x }));
    }
    each(fn) {
      this._items.forEach(x => fn(x));
    }
    map(fn) {
      return this._items.map(fn);
    }
    filter(fn) {
      return this._items.filter(fn);
    }
    count() {
      return this._items.length;
    }
    idx(i) {
      return this._items[i];
    }
    toObject() {
      const o = {};
      for (const x of this._items) if (!x.disabled) o[x.key] = x.value;
      return o;
    }
    toJSON() {
      return this.all();
    }
    toString() {
      return this._items.filter(x => !x.disabled).map(x => `${x.key}: ${x.value}`).join('\n');
    }
  }

  // ---------------------------------------------------------------- environments & variables
  function replaceIn(template, vars) {
    if (typeof template === 'object' && template !== null) return JSON.parse(replaceIn(JSON.stringify(template), vars));
    return host.render(String(template), JSON.stringify(vars));
  }

  class Environment {
    constructor(name, data) {
      this._name = name || '';
      this._data = { ...(data || {}) };
    }
    get name() {
      return this._name;
    }
    has(k) {
      return hasOwn(this._data, k);
    }
    get(k) {
      return this._data[k];
    }
    set(k, v) {
      if (typeof k !== 'string' || !k) throw new Error('environment.set: variable name must be a non-empty string');
      if (typeof v === 'function') throw new Error('environment.set: functions cannot be stored');
      this._data[k] = v === undefined ? undefined : clone(v);
    }
    unset(k) {
      delete this._data[k];
    }
    clear() {
      this._data = {};
    }
    replaceIn(t) {
      return replaceIn(t, this._data);
    }
    toObject() {
      return { ...this._data };
    }
    toJSON() {
      return this.toObject();
    }
  }

  class Variables {
    constructor(scopes) {
      this._s = scopes; // { globals, base, env, folders: [{name, environment}], iteration, locals }
    }
    _merged() {
      // Same precedence as Insomnia's Variables.get: local > folders (inner wins)
      // > iterationData > environment > base (collection) > globals.
      const m = {};
      Object.assign(m, this._s.globals._data, this._s.base._data, this._s.env._data, this._s.iteration._data);
      for (const f of this._s.folders) Object.assign(m, f.environment || {});
      Object.assign(m, this._s.locals);
      return m;
    }
    has(k) {
      return hasOwn(this._merged(), k);
    }
    get(k) {
      return this._merged()[k];
    }
    set(k, v) {
      if (typeof k !== 'string' || !k) throw new Error('variables.set: variable name must be a non-empty string');
      if (Number.isNaN(v)) {
        scriptConsole.warn(`Variable "${k}" has a NaN value`);
        return;
      }
      this._s.locals[k] = clone(v);
    }
    unset(k) {
      delete this._s.locals[k];
    }
    replaceIn(t) {
      return replaceIn(t, this._merged());
    }
    toObject() {
      return this._merged();
    }
    localVarsToObject() {
      return { ...this._s.locals };
    }
  }

  // ---------------------------------------------------------------- request
  function splitUrl(str) {
    const s = String(str);
    const hash = s.indexOf('#');
    const noHash = hash >= 0 ? s.slice(0, hash) : s;
    const q = noHash.indexOf('?');
    if (q < 0) return { base: noHash, query: [] };
    const dec = x => {
      try {
        return decodeURIComponent(x.replace(/\+/g, ' '));
      } catch (_) {
        return x;
      }
    };
    const query = noHash
      .slice(q + 1)
      .split('&')
      .filter(Boolean)
      .map(p => {
        const i = p.indexOf('=');
        return i < 0 ? { key: dec(p), value: '' } : { key: dec(p.slice(0, i)), value: dec(p.slice(i + 1)) };
      });
    return { base: noHash.slice(0, q), query };
  }
  const enc = s => encodeURIComponent(String(s)).replace(/%7B/g, '{').replace(/%7D/g, '}').replace(/%20/g, '%20');

  class Url {
    constructor(req) {
      this._req = req;
    }
    get query() {
      return this._req._query;
    }
    getQueryString() {
      return this._req._query
        .all()
        .filter(x => !x.disabled)
        .map(x => `${enc(x.key)}=${enc(x.value)}`)
        .join('&');
    }
    toString() {
      const qs = this.getQueryString();
      return qs ? `${this._req._url}${this._req._url.includes('?') ? '&' : '?'}${qs}` : this._req._url;
    }
    _parts() {
      const m = /^([a-z][a-z0-9+.-]*):\/\/([^/?#:]*)(?::(\d+))?([^?#]*)/i.exec(this._req._url);
      return m ? { protocol: m[1], host: m[2], port: m[3] || '', path: m[4] || '/' } : { protocol: '', host: '', port: '', path: this._req._url };
    }
    get protocol() {
      return this._parts().protocol;
    }
    get host() {
      return this._parts().host.split('.');
    }
    get port() {
      return this._parts().port;
    }
    get path() {
      return this._parts().path.split('/').filter(Boolean);
    }
    getHost() {
      return this._parts().host;
    }
    getPath() {
      return this._parts().path;
    }
    getPathWithQuery() {
      const qs = this.getQueryString();
      return this.getPath() + (qs ? `?${qs}` : '');
    }
    getRemote() {
      const p = this._parts();
      return p.port ? `${p.host}:${p.port}` : p.host;
    }
    addQueryParams(params) {
      const list = typeof params === 'string' ? splitUrl(`?${params}`).query : params;
      for (const p of list) this._req._query.add(p);
    }
    removeQueryParams(params) {
      for (const p of [].concat(params)) this._req._query.remove(typeof p === 'string' ? p : p.key);
    }
    update(str) {
      this._req.url = str;
    }
    toJSON() {
      return this.toString();
    }
  }

  function authToScript(a) {
    a = a || { type: 'inherit' };
    const kv = o => Object.entries(o).map(([key, value]) => ({ key, value: value ?? '', type: 'string' }));
    switch (a.type) {
      case 'basic':
        return { type: 'basic', basic: kv({ username: a.username, password: a.password }), disabled: !!a.disabled };
      case 'bearer':
        return { type: 'bearer', bearer: kv({ token: a.token, prefix: a.prefix || '' }), disabled: !!a.disabled };
      case 'apikey':
        return { type: 'apikey', apikey: kv({ key: a.key, value: a.value, in: a.addTo || 'header' }), disabled: !!a.disabled };
      case 'none':
        return { type: 'noauth' };
      default:
        return { type: 'inherit' };
    }
  }
  function authFromScript(s) {
    const get = (list, k) => ((list || []).find(x => x.key === k) || {}).value ?? '';
    switch (s.type) {
      case 'basic':
        return { type: 'basic', username: get(s.basic, 'username'), password: get(s.basic, 'password'), disabled: !!s.disabled };
      case 'bearer':
        return { type: 'bearer', token: get(s.bearer, 'token'), prefix: get(s.bearer, 'prefix') || null, disabled: !!s.disabled };
      case 'apikey':
        return { type: 'apikey', key: get(s.apikey, 'key'), value: get(s.apikey, 'value'), addTo: get(s.apikey, 'in') || 'header', disabled: !!s.disabled };
      case 'noauth':
      case 'none':
        return { type: 'none' };
      default:
        return { type: 'inherit' };
    }
  }

  class RequestAuth {
    constructor(raw) {
      Object.assign(this, authToScript(raw));
    }
    update(obj, type) {
      for (const k of Object.keys(this)) delete this[k];
      const next = { ...obj };
      if (type) next.type = type;
      // accept `{type:'bearer', bearer:{token:'x'}}` as well as the list form
      for (const k of ['basic', 'bearer', 'apikey'])
        if (next[k] && !Array.isArray(next[k])) next[k] = Object.entries(next[k]).map(([key, value]) => ({ key, value }));
      Object.assign(this, next);
    }
    parameters() {
      return new PropertyList(this[this.type] || []);
    }
    toJSON() {
      return { ...this };
    }
  }

  class ScriptRequest {
    constructor(raw) {
      this.id = raw.id;
      this.name = raw.name;
      this.method = raw.method;
      this._url = raw.url;
      this._query = new PropertyList(raw.query);
      this.headers = new PropertyList(raw.headers);
      this.body = Object.assign(Object.create({ update(b) {
        for (const k of Object.keys(this)) delete this[k];
        Object.assign(this, typeof b === 'string' ? { mode: 'raw', raw: b } : b);
      }, toString() {
        return this.mode === 'raw' ? this.raw || '' : JSON.stringify(this);
      } }), raw.body || {});
      this.auth = new RequestAuth(raw.auth);
      this._urlObj = new Url(this);
    }
    get url() {
      return this._urlObj;
    }
    set url(v) {
      const s = typeof v === 'string' ? v : String(v);
      const { base, query } = splitUrl(s);
      this._url = base;
      this._query = new PropertyList(query);
    }
    addHeader(h) {
      this.headers.add(h);
    }
    removeHeader(k) {
      this.headers.remove(k);
    }
    upsertHeader(h) {
      this.headers.upsert(h);
    }
    addQueryParams(p) {
      this.url.addQueryParams(p);
    }
    removeQueryParams(p) {
      this.url.removeQueryParams(p);
    }
    getHeaders() {
      return this.headers.toObject();
    }
    toJSON() {
      return this._dump();
    }
    _dump() {
      const body = {};
      for (const k of Object.keys(this.body)) body[k] = this.body[k];
      const auth = {};
      for (const k of Object.keys(this.auth)) auth[k] = this.auth[k];
      return {
        id: this.id,
        name: this.name,
        method: String(this.method || 'GET').toUpperCase(),
        url: this._url,
        query: this._query.all(),
        headers: this.headers.all(),
        body,
        auth: authFromScript(auth),
      };
    }
  }

  // ---------------------------------------------------------------- response
  class ScriptResponse {
    constructor(raw) {
      this.__isResponse = true;
      this.code = raw.code;
      this.status = raw.status || '';
      this.headers = new PropertyList(raw.headers);
      this.body = raw.body ?? '';
      this.stream = this.body;
      this.responseTime = raw.responseTime ?? 0;
      this.cookies = new PropertyList([]);
    }
    text() {
      return this.body;
    }
    json(reviver) {
      try {
        return JSON.parse(this.body, reviver);
      } catch (e) {
        throw new Error(`response body is not valid JSON: ${e.message}`);
      }
    }
    reason() {
      return this.status;
    }
    size() {
      return { body: this.body.length, header: this.headers.toString().length, total: this.body.length };
    }
    contentInfo() {
      return { contentType: this.headers.get('content-type') || '', mimeType: (this.headers.get('content-type') || '').split(';')[0] };
    }
    get to() {
      return scriptRequire('chai').expect(this).to;
    }
    toJSON() {
      return { code: this.code, status: this.status, headers: this.headers.all(), body: this.body, responseTime: this.responseTime };
    }
  }

  function installChaiPlugins(chai) {
    chai.use((_chai, utils) => {
      const A = _chai.Assertion;
      const isResp = o => o && o.__isResponse;
      A.addMethod('status', function (expected) {
        const r = utils.flag(this, 'object');
        if (typeof expected === 'number')
          this.assert(r.code === expected, `expected response to have status code #{exp} but got #{act}`, `expected response to not have status code #{act}`, expected, r.code);
        else
          this.assert(r.status === expected, `expected response to have status reason #{exp} but got #{act}`, `expected response to not have status reason #{act}`, expected, r.status);
      });
      A.addMethod('header', function (name, value) {
        const r = utils.flag(this, 'object');
        const has = r.headers.has(name);
        if (value === undefined) this.assert(has, `expected response to have header ${name}`, `expected response to not have header ${name}`);
        else this.assert(r.headers.get(name) === value, `expected header ${name} to be #{exp} but got #{act}`, `expected header ${name} to not be #{exp}`, value, r.headers.get(name));
      });
      A.addMethod('body', function (expected) {
        const r = utils.flag(this, 'object');
        if (expected === undefined) this.assert(r.body !== '', 'expected response to have a body', 'expected response to not have a body');
        else if (expected instanceof RegExp) this.assert(expected.test(r.body), 'expected body to match #{exp}', 'expected body to not match #{exp}', expected);
        else if (typeof expected === 'string') this.assert(r.body === expected, 'expected body to equal #{exp}', 'expected body to not equal #{exp}', expected, r.body);
        else new A(r.json()).to.deep.equal(expected);
      });
      A.addMethod('jsonBody', function (path, value) {
        const r = utils.flag(this, 'object');
        const data = r.json();
        if (path === undefined) return;
        if (typeof path === 'object') return new A(data).to.deep.equal(path);
        const got = String(path).split('.').reduce((o, k) => (o == null ? undefined : o[k]), data);
        if (value === undefined) this.assert(got !== undefined, `expected JSON body to have path ${path}`, `expected JSON body to not have path ${path}`);
        else new A(got).to.deep.equal(value);
      });
      for (const [name, check, msg] of [
        ['success', r => r.code >= 200 && r.code < 300, 'a 2xx status'],
        ['error', r => r.code >= 400 && r.code < 600, 'a 4xx/5xx status'],
        ['clientError', r => r.code >= 400 && r.code < 500, 'a 4xx status'],
        ['serverError', r => r.code >= 500, 'a 5xx status'],
        ['withBody', r => r.body !== '', 'a body'],
        ['json', r => { try { JSON.parse(r.body); return true; } catch (_) { return false; } }, 'a JSON body'],
      ]) {
        A.addProperty(name, function () {
          const r = utils.flag(this, 'object');
          if (!isResp(r)) throw new Error(`.${name} can only be used on insomnia.response`);
          this.assert(check(r), `expected response to have ${msg} (status ${r.code})`, `expected response to not have ${msg} (status ${r.code})`);
        });
      }
      utils.overwriteProperty(A.prototype, 'ok', _super => function () {
        const r = utils.flag(this, 'object');
        if (isResp(r)) this.assert(r.code === 200, 'expected response status to be 200 but got #{act}', 'expected response status to not be 200', 200, r.code);
        else _super.call(this);
      });
    });
  }

  // ---------------------------------------------------------------- cookies
  function cookieMatches(c, url) {
    const m = /^[a-z]+:\/\/([^/?#:]+)/i.exec(String(url));
    const host = m ? m[1].toLowerCase() : String(url).toLowerCase();
    const d = (c.domain || '').replace(/^\./, '').toLowerCase();
    return host === d || (!c.hostOnly && host.endsWith(`.${d}`));
  }
  function makeJar(store) {
    const done = (cb, err, v) => {
      if (typeof cb === 'function') cb(err, v);
    };
    const hostOf = url => (/^[a-z]+:\/\/([^/?#:]+)/i.exec(String(url)) || [null, String(url)])[1].toLowerCase();
    return {
      set(url, name, value, cb) {
        if (typeof name === 'object') {
          cb = value;
          value = name;
          name = value.key ?? value.name;
        }
        const v = typeof value === 'object' && value !== null ? value : { value };
        const c = {
          name,
          value: String(v.value ?? ''),
          domain: v.domain ?? hostOf(url),
          path: v.path ?? '/',
          secure: !!v.secure,
          httpOnly: !!v.httpOnly,
          hostOnly: v.domain === undefined,
          expires: v.expires ? new Date(v.expires).getTime() : null,
        };
        store.list = store.list.filter(x => !(x.name === c.name && x.domain === c.domain && x.path === c.path));
        store.list.push(c);
        store.changed = true;
        done(cb, null, c);
      },
      get(url, name, cb) {
        const c = store.list.find(x => x.name === name && cookieMatches(x, url));
        done(cb, null, c ? c.value : undefined);
      },
      getAll(url, cb) {
        done(cb, null, store.list.filter(x => cookieMatches(x, url)).map(x => ({ ...x, key: x.name })));
      },
      unset(url, name, cb) {
        const before = store.list.length;
        store.list = store.list.filter(x => !(x.name === name && cookieMatches(x, url)));
        store.changed = store.changed || store.list.length !== before;
        done(cb, null);
      },
      clear(url, cb) {
        store.list = store.list.filter(x => !cookieMatches(x, url));
        store.changed = true;
        done(cb, null);
      },
    };
  }

  // ---------------------------------------------------------------- sendRequest
  function normalizeOutgoing(req) {
    if (typeof req === 'string') return { method: 'GET', url: req, headers: [], body: null };
    const r = req instanceof ScriptRequest ? req._dump() : req;
    let url = r.url;
    if (url instanceof Url) url = url.toString();
    else if (url && typeof url === 'object') url = url.raw || url.toString();
    if (r instanceof Object && r.query && Array.isArray(r.query) && r.query.length) {
      const qs = r.query.filter(x => !x.disabled).map(x => `${enc(x.key)}=${enc(x.value)}`).join('&');
      if (qs) url += (String(url).includes('?') ? '&' : '?') + qs;
    }
    let headers = r.headers || r.header || [];
    if (headers instanceof PropertyList) headers = headers.all();
    if (!Array.isArray(headers)) headers = Object.entries(headers).map(([key, value]) => ({ key, value }));
    headers = headers.filter(h => !h.disabled).map(h => [String(h.key ?? h.name), String(h.value)]);
    let body = null;
    const b = r.body;
    if (typeof b === 'string') body = b;
    else if (b && b.mode === 'raw') body = b.raw ?? '';
    else if (b && b.mode === 'urlencoded') {
      body = (b.urlencoded || []).filter(x => !x.disabled).map(x => `${encodeURIComponent(x.key)}=${encodeURIComponent(x.value)}`).join('&');
      if (!headers.some(([k]) => k.toLowerCase() === 'content-type')) headers.push(['Content-Type', 'application/x-www-form-urlencoded']);
    } else if (b && b.mode === 'graphql') {
      body = JSON.stringify(b.graphql || {});
      if (!headers.some(([k]) => k.toLowerCase() === 'content-type')) headers.push(['Content-Type', 'application/json']);
    } else if (b && typeof b === 'object' && !b.mode && Object.keys(b).length) body = JSON.stringify(b);
    return { method: String(r.method || 'GET').toUpperCase(), url: String(url), headers, body };
  }

  function sendRequest(req, cb) {
    let out;
    try {
      out = normalizeOutgoing(req);
    } catch (e) {
      const err = `sendRequest: invalid request (${e.message})`;
      if (typeof cb === 'function') cb(err);
      return Promise.reject(new Error(err));
    }
    return host.send(JSON.stringify(out)).then(
      json => {
        const r = JSON.parse(json);
        if (r.error) {
          if (typeof cb === 'function') {
            cb(r.error);
            return undefined;
          }
          throw new Error(r.error);
        }
        const resp = new ScriptResponse(r);
        if (typeof cb === 'function') cb(null, resp);
        return resp;
      },
    );
  }

  // ---------------------------------------------------------------- tests
  const testResults = [];
  const pendingTests = [];
  function makeTest(category) {
    const test = function (name, fn) {
      const start = Date.now();
      const record = (status, err) =>
        testResults.push({
          name: String(name),
          status,
          error: err ? describeError(err) : null,
          durationMs: Date.now() - start,
          category,
        });
      try {
        const r = typeof fn === 'function' ? fn() : undefined;
        if (r && typeof r.then === 'function') pendingTests.push(r.then(() => record('passed'), e => record('failed', e)));
        else record('passed');
      } catch (e) {
        record('failed', e);
      }
    };
    test.skip = name => testResults.push({ name: String(name), status: 'skipped', error: null, durationMs: 0, category });
    return test;
  }
  function describeError(e) {
    if (e && e.name === 'AssertionError') return e.message;
    return e && e.message ? e.message : String(e);
  }

  // ---------------------------------------------------------------- entry
  globalThis.__irs = {
    async run(ctxJson, userFn) {
      const c = JSON.parse(ctxJson);
      const env = new Environment(c.environment.name, c.environment.data);
      const base = new Environment(c.baseEnvironment.name, c.baseEnvironment.data);
      const globals = new Environment(c.globals.name, c.globals.data);
      const iteration = new Environment('iterationData', c.iterationData || {});
      const locals = { ...(c.localVariables || {}) };
      const folders = c.folders || [];
      const variables = new Variables({ globals, base, env, folders, iteration, locals });
      const request = new ScriptRequest(c.request);
      const cookieStore = { list: (c.cookies || []).map(x => ({ ...x })), changed: false };
      const execution = { _skip: false, _next: null, location: [...(c.location || []), c.request.name] };
      execution.skipRequest = () => {
        if (c.info.eventName !== 'prerequest') throw new Error('execution.skipRequest() can only be used in pre-request scripts');
        execution._skip = true;
      };
      execution.setNextRequest = idOrName => {
        execution._next = idOrName === null ? '__stop__' : String(idOrName);
      };
      execution.location.current = c.request.name;
      const requestCookies = new PropertyList(
        cookieStore.list.filter(x => cookieMatches(x, c.request.url)).map(x => ({ key: x.name, value: x.value })),
      );
      const insomnia = {
        environment: env,
        baseEnvironment: base,
        collectionVariables: base,
        globals,
        iterationData: iteration,
        variables,
        request,
        response: c.response ? new ScriptResponse(c.response) : undefined,
        info: { ...c.info },
        execution,
        test: makeTest(c.info.eventName === 'prerequest' ? 'pre-request' : 'after-response'),
        sendRequest,
        parentFolders: {
          get: name => {
            const f = folders.find(x => x.name === name);
            return f ? { name: f.name, environment: new Environment(f.name, f.environment) } : undefined;
          },
          toObject: () => folders,
        },
        settings: {},
        vault: new Proxy({}, { get: () => { throw new Error('NotSupported: insomnia.vault (arrives with vault support)'); } }),
      };
      insomnia.cookies = Object.assign(requestCookies, { jar: () => makeJar(cookieStore) });
      Object.defineProperty(insomnia, 'expect', { get: () => scriptRequire('chai').expect, enumerable: true });

      const error = await (async () => {
        try {
          await userFn(insomnia, insomnia, insomnia, scriptRequire, scriptConsole, setTimeoutImpl, clearTimeoutImpl);
          // let timers and async tests finish
          for (let i = 0; i < 100 && (pendingTimers.size || pendingTests.length); i++) {
            await Promise.all([...pendingTimers, ...pendingTests.splice(0)]);
          }
          return null;
        } catch (e) {
          return { message: describeError(e), stack: e && e.stack ? String(e.stack) : '' };
        }
      })();

      return JSON.stringify({
        request: request._dump(),
        environment: env.toObject(),
        baseEnvironment: base.toObject(),
        globals: globals.toObject(),
        localVariables: locals,
        cookies: cookieStore.changed ? cookieStore.list : null,
        tests: testResults,
        execution: { skipRequest: execution._skip, nextRequest: execution._next },
        error,
      });
    },
  };

  Object.defineProperty(globalThis, '_', { get: () => scriptRequire('lodash'), configurable: true });
})();
