// logic-socket script runtime: re-implements the `ls.*` object model
// (ls.* — pm.* and $.* are aliases) on top of a few host functions:
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
  // Filled at the end of this file, once `sendRequest` exists.
  let requestClients;
  const VENDORED = ['chai', 'lodash', 'crypto-js', 'moment', 'tv4', 'ajv'];
  const AVAILABLE = 'chai, lodash, uuid, crypto-js, moment, tv4, ajv, atob, btoa, curl, axios, node-fetch, cross-fetch, got, undici, request, postman-request, superagent';
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
      throw new Error(`NotSupported: require('${name}') — Node built-in modules are not available in the logic-socket sandbox`);
    else if (name === 'cheerio' || name === 'xml2js')
      throw new Error(`NotSupported: require('${name}') — needs a Node runtime; not available in the logic-socket sandbox`);
    else if (requestClients && hasOwn(requestClients, name)) mod = requestClients[name];
    else throw new Error(`Cannot find module '${name}'. Available: ${AVAILABLE}`);
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
      // Precedence, same as `{{ }}` rendering: local > iterationData
      // > folders (inner wins) > environment > base (collection) > globals.
      const m = {};
      Object.assign(m, this._s.globals._data, this._s.base._data, this._s.env._data);
      for (const f of this._s.folders) Object.assign(m, f.environment || {});
      Object.assign(m, this._s.iteration._data, this._s.locals);
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
    const frag = hash >= 0 ? s.slice(hash) : '';
    const q = noHash.indexOf('?');
    if (q < 0) return { base: noHash, query: [], hash: frag };
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
    return { base: noHash.slice(0, q), query, hash: frag };
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
      return `${this._req._url}${qs ? `?${qs}` : ''}${this._req._hash || ''}`;
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
      case 'inherit':
        return { type: 'inherit' };
      default:
        // oauth2, oauth1, iam, digest, netrc: visible by type, preserved verbatim
        return { type: a.type, _raw: a };
    }
  }
  function authFromScript(s) {
    if (s._raw && s._raw.type === s.type) return s._raw;
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
      // and the flat form: `update({ username, password }, 'basic')`
      const t = next.type;
      if (['basic', 'bearer', 'apikey'].includes(t) && !next[t]) {
        const flat = Object.entries(next).filter(([key]) => !['type', 'disabled'].includes(key));
        next[t] = flat.map(([key, value]) => ({ key, value }));
        for (const [key] of flat) delete next[key];
      }
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
      // A query string typed into the URL is part of the parameter list, so
      // query edits see it and toString() does not repeat it.
      const { base, query, hash } = splitUrl(raw.url);
      this._url = base;
      this._hash = hash;
      this._query = new PropertyList(query.concat(raw.query || []));
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
      const { base, query, hash } = splitUrl(s);
      this._url = base;
      this._hash = hash;
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
        url: this._url + (this._hash || ''),
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
          if (!isResp(r)) throw new Error(`.${name} can only be used on ls.response`);
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

  // ---------------------------------------------------------------- request clients
  // require('curl' | 'axios' | 'node-fetch' | 'got' | 'request' | 'superagent' | …)
  // and the matching dynamic import(). Every call goes through host.send.
  // Node's http, https, net, and child_process stay unavailable.

  function pairsFromHeaders(headers) {
    if (!headers) return [];
    if (Array.isArray(headers)) {
      return headers.filter(h => h && !h.disabled).map(h => {
        if (Array.isArray(h)) return [String(h[0]), String(h[1] ?? '')];
        if (typeof h === 'string') {
          const i = h.indexOf(':');
          return i < 0 ? [h.trim(), ''] : [h.slice(0, i).trim(), h.slice(i + 1).trim()];
        }
        return [String(h.key ?? h.name), String(h.value ?? '')];
      });
    }
    if (typeof headers.entries === 'function') {
      const out = [];
      for (const pair of headers.entries()) out.push([String(pair[0]), String(pair[1] ?? '')]);
      return out;
    }
    if (typeof headers === 'object') {
      return Object.entries(headers).flatMap(([k, v]) =>
        v == null ? [] : Array.isArray(v) ? v.map(x => [k, String(x)]) : [[k, String(v)]]);
    }
    return [];
  }

  function upsertHeader(headers, name, value) {
    const i = headers.findIndex(([k]) => k.toLowerCase() === name.toLowerCase());
    if (i >= 0) headers[i] = [headers[i][0], value];
    else headers.push([name, value]);
  }

  function withQuery(url, params) {
    if (!params || typeof params !== 'object') return String(url);
    const qs = Object.entries(params)
      .filter(([, v]) => v !== undefined && v !== null)
      .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
      .join('&');
    if (!qs) return String(url);
    const s = String(url);
    return s + (s.includes('?') ? '&' : '?') + qs;
  }

  function joinBase(base, url) {
    const u = url == null ? '' : String(url);
    if (!base || /^[a-z][a-z0-9+.-]*:/i.test(u)) return u;
    return String(base).replace(/\/+$/, '') + '/' + u.replace(/^\/+/, '');
  }

  function responseHeaderMap(raw) {
    const headers = {};
    const list = raw.headers && typeof raw.headers.all === 'function' ? raw.headers.all() : [];
    for (const h of list) {
      if (!h || h.disabled) continue;
      headers[String(h.key).toLowerCase()] = String(h.value);
    }
    return headers;
  }

  function clientSend(spec) {
    if (!spec.url) return Promise.reject(new Error('request: missing url'));
    const req = {
      method: String(spec.method || 'GET').toUpperCase(),
      url: String(spec.url),
      header: (spec.headers || []).map(([key, value]) => ({ key, value })),
    };
    if (spec.body != null) req.body = { mode: 'raw', raw: String(spec.body) };
    return sendRequest(req);
  }

  // Strings are sent as-is. Objects are JSON, and a JSON content type is added.
  function encodeBody(data) {
    if (data == null) return null;
    if (typeof data === 'string') return { body: data, json: false };
    return { body: JSON.stringify(data), json: true };
  }

  function jsonResult(raw) {
    const headers = responseHeaderMap(raw);
    return {
      status: raw.code,
      statusCode: raw.code,
      statusText: raw.status,
      body: raw.body ?? '',
      headers,
      responseTime: raw.responseTime,
      json() {
        return JSON.parse(raw.body);
      },
    };
  }

  // libcurl option numbers scripts already use via node-libcurl.
  const CURL_OPTIONS = {
    URL: 10002,
    USERPWD: 10005,
    POSTFIELDS: 10015,
    REFERER: 10016,
    USERAGENT: 10018,
    COOKIE: 10022,
    HTTPHEADER: 10023,
    CUSTOMREQUEST: 10036,
    NOBODY: 44,
    FAILONERROR: 45,
    POST: 47,
    FOLLOWLOCATION: 52,
    HTTPGET: 80,
    USERNAME: 10173,
    PASSWORD: 10174,
    XOAUTH2_BEARER: 10220,
  };

  function curlOptName(opt) {
    if (typeof opt === 'number') {
      for (const [k, v] of Object.entries(CURL_OPTIONS)) if (v === opt) return k;
      return `#${opt}`;
    }
    return String(opt).replace(/^CURLOPT_/i, '').toUpperCase();
  }

  class CurlHandle {
    constructor() {
      this._url = '';
      this._method = '';
      this._body = null;
      this._headers = [];
      this._user = '';
      this._pass = '';
      this._bearer = '';
      this._fail = false;
      this._closed = false;
      this._on = { end: [], error: [] };
    }
    setOpt(opt, value) {
      if (this._closed) throw new Error('curl handle is closed');
      switch (curlOptName(opt)) {
        case 'URL': this._url = String(value ?? ''); break;
        case 'CUSTOMREQUEST': this._method = String(value || 'GET').toUpperCase(); break;
        case 'POSTFIELDS': {
          const enc = encodeBody(value);
          this._body = enc ? enc.body : null;
          if (enc && enc.json) upsertHeader(this._headers, 'Content-Type', 'application/json');
          if (!this._method) this._method = 'POST';
          break;
        }
        case 'HTTPHEADER': this._headers = pairsFromHeaders(value); break;
        case 'POST': if (value) this._method = this._method || 'POST'; break;
        case 'HTTPGET': if (value) { this._method = 'GET'; this._body = null; } break;
        case 'NOBODY': if (value) { this._method = 'HEAD'; this._body = null; } break;
        case 'USERAGENT': upsertHeader(this._headers, 'User-Agent', String(value ?? '')); break;
        case 'REFERER': upsertHeader(this._headers, 'Referer', String(value ?? '')); break;
        case 'COOKIE': upsertHeader(this._headers, 'Cookie', String(value ?? '')); break;
        case 'USERPWD': {
          const s = String(value ?? '');
          const i = s.indexOf(':');
          this._user = i < 0 ? s : s.slice(0, i);
          this._pass = i < 0 ? '' : s.slice(i + 1);
          break;
        }
        case 'USERNAME': this._user = String(value ?? ''); break;
        case 'PASSWORD': this._pass = String(value ?? ''); break;
        case 'XOAUTH2_BEARER': this._bearer = String(value ?? ''); break;
        case 'FAILONERROR': this._fail = !!value; break;
        case 'FOLLOWLOCATION':
          if (!value) throw new Error('NotSupported: curl option FOLLOWLOCATION cannot be turned off');
          break;
        default:
          throw new Error(`NotSupported: curl option ${curlOptName(opt)}`);
      }
      return this;
    }
    on(event, fn) {
      if (this._on[event]) this._on[event].push(fn);
      return this;
    }
    close() { this._closed = true; }
    perform() {
      if (this._closed) return Promise.reject(new Error('curl handle is closed'));
      if (!this._url) return Promise.reject(new Error('curl: missing URL'));
      const headers = this._headers.map(h => h.slice());
      if (this._bearer) upsertHeader(headers, 'Authorization', `Bearer ${this._bearer}`);
      else if (this._user || this._pass) upsertHeader(headers, 'Authorization', 'Basic ' + globalThis.btoa(`${this._user}:${this._pass}`));
      return clientSend({ method: this._method || 'GET', url: this._url, headers, body: this._body }).then(raw => {
        if (this._fail && raw.code >= 400) {
          const err = new Error(`curl: HTTP error ${raw.code}`);
          this._on.error.forEach(fn => fn(err));
          throw err;
        }
        const lines = raw.headers.all().filter(h => !h.disabled).map(h => `${h.key}: ${h.value}`);
        const result = jsonResult(raw);
        result.headerLines = lines;
        this._on.end.forEach(fn => fn(raw.code, result.body, lines));
        return result;
      }, err => {
        this._on.error.forEach(fn => fn(err));
        throw err;
      });
    }
  }
  CurlHandle.option = CURL_OPTIONS;

  function makeCurl() {
    function curl(opts, cb) { return curl.request(opts, cb); }
    function run(opts) {
      const headers = pairsFromHeaders(opts.headers || opts.httpHeader);
      const enc = encodeBody(opts.data ?? opts.body ?? opts.postFields);
      if (enc && enc.json) upsertHeader(headers, 'Content-Type', 'application/json');
      if (opts.user || opts.username) {
        upsertHeader(headers, 'Authorization', 'Basic ' + globalThis.btoa(`${opts.user || opts.username}:${opts.password || opts.pass || ''}`));
      }
      return clientSend({
        method: opts.method || (enc ? 'POST' : 'GET'),
        url: opts.url,
        headers,
        body: enc && enc.body,
      }).then(jsonResult);
    }
    curl.request = function (opts, cb) {
      const p = run(typeof opts === 'string' ? { url: opts } : (opts || {}));
      if (typeof cb === 'function') {
        p.then(res => cb(null, res), err => cb(err));
        return undefined;
      }
      return p;
    };
    curl.get = (url, opts) => curl.request({ ...(opts || {}), url, method: 'GET' });
    curl.post = (url, data, opts) => curl.request({ ...(opts || {}), url, data, method: 'POST' });
    curl.put = (url, data, opts) => curl.request({ ...(opts || {}), url, data, method: 'PUT' });
    curl.patch = (url, data, opts) => curl.request({ ...(opts || {}), url, data, method: 'PATCH' });
    curl.delete = (url, opts) => curl.request({ ...(opts || {}), url, method: 'DELETE' });
    curl.Curl = CurlHandle;
    curl.option = CURL_OPTIONS;
    return curl;
  }

  function axiosResponse(raw, config) {
    const headers = responseHeaderMap(raw);
    let data = raw.body ?? '';
    if ((headers['content-type'] || '').includes('json')) {
      try { data = JSON.parse(raw.body); } catch (_) { /* keep the text */ }
    }
    return { data, status: raw.code, statusText: raw.status, headers, config };
  }

  function makeAxios() {
    function axios(config) { return axios.request(config); }
    axios.defaults = { validateStatus: status => status >= 200 && status < 300 };
    function request(config) {
      config = config || {};
      const headers = pairsFromHeaders(config.headers);
      const enc = encodeBody(config.data);
      if (enc && enc.json) upsertHeader(headers, 'Content-Type', 'application/json');
      if (config.auth) {
        upsertHeader(headers, 'Authorization', 'Basic ' + globalThis.btoa(`${config.auth.username || ''}:${config.auth.password || ''}`));
      }
      const validate = config.validateStatus || axios.defaults.validateStatus;
      return clientSend({
        method: config.method || 'GET',
        url: withQuery(joinBase(config.baseURL, config.url), config.params),
        headers,
        body: enc && enc.body,
      }).then(raw => {
        const response = axiosResponse(raw, config);
        if (!validate(raw.code)) {
          const err = new Error(`Request failed with status code ${raw.code}`);
          err.response = response;
          err.isAxiosError = true;
          throw err;
        }
        return response;
      });
    }
    axios.request = request;
    axios.get = (url, config) => request({ ...(config || {}), url, method: 'GET' });
    axios.delete = (url, config) => request({ ...(config || {}), url, method: 'DELETE' });
    axios.head = (url, config) => request({ ...(config || {}), url, method: 'HEAD' });
    axios.options = (url, config) => request({ ...(config || {}), url, method: 'OPTIONS' });
    axios.post = (url, data, config) => request({ ...(config || {}), url, data, method: 'POST' });
    axios.put = (url, data, config) => request({ ...(config || {}), url, data, method: 'PUT' });
    axios.patch = (url, data, config) => request({ ...(config || {}), url, data, method: 'PATCH' });
    axios.create = defaults => {
      const inst = config => inst.request(config);
      inst.request = config => {
        const d = defaults || {};
        const c = config || {};
        return request({
          ...d,
          ...c,
          headers: { ...(d.headers || {}), ...(c.headers || {}) },
          params: { ...(d.params || {}), ...(c.params || {}) },
        });
      };
      inst.get = (url, config) => inst.request({ ...(config || {}), url, method: 'GET' });
      inst.post = (url, data, config) => inst.request({ ...(config || {}), url, data, method: 'POST' });
      inst.put = (url, data, config) => inst.request({ ...(config || {}), url, data, method: 'PUT' });
      inst.patch = (url, data, config) => inst.request({ ...(config || {}), url, data, method: 'PATCH' });
      inst.delete = (url, config) => inst.request({ ...(config || {}), url, method: 'DELETE' });
      return inst;
    };
    return axios;
  }

  class FetchHeaders {
    constructor(init) {
      this._m = {};
      for (const [k, v] of pairsFromHeaders(init)) this._m[k.toLowerCase()] = v;
    }
    get(name) { return this._m[String(name).toLowerCase()]; }
    has(name) { return this.get(name) !== undefined; }
    entries() { return Object.entries(this._m); }
    forEach(fn) { for (const [k, v] of Object.entries(this._m)) fn(v, k); }
  }

  function fetchResponse(raw) {
    const headers = new FetchHeaders(responseHeaderMap(raw));
    const text = raw.body ?? '';
    return {
      ok: raw.code >= 200 && raw.code < 300,
      status: raw.code,
      statusText: raw.status,
      headers,
      text: async () => text,
      json: async () => JSON.parse(text),
    };
  }

  function makeFetch() {
    function fetch(url, init) {
      init = init || {};
      const headers = pairsFromHeaders(init.headers);
      const enc = encodeBody(init.body);
      if (enc && enc.json) upsertHeader(headers, 'Content-Type', 'application/json');
      return clientSend({ method: init.method || 'GET', url, headers, body: enc && enc.body }).then(fetchResponse);
    }
    fetch.Headers = FetchHeaders;
    return fetch;
  }

  function splitRequestArgs(a, b, c) {
    if (typeof a === 'string') {
      if (typeof b === 'function') return { opts: { url: a }, cb: b };
      return { opts: { ...(b || {}), url: (b && b.url) || a }, cb: c };
    }
    return { opts: { ...(a || {}) }, cb: typeof b === 'function' ? b : undefined };
  }

  function makeRequest() {
    function core(opts) {
      const headers = pairsFromHeaders(opts.headers);
      let data = opts.body;
      let jsonMode = false;
      if (opts.json && typeof opts.json === 'object') {
        data = opts.json;
        jsonMode = true;
      } else if (opts.json) jsonMode = true;
      const enc = data == null ? null : (jsonMode && typeof data !== 'string'
        ? { body: JSON.stringify(data), json: true }
        : encodeBody(data));
      if (enc && (enc.json || jsonMode) && typeof data !== 'string') upsertHeader(headers, 'Content-Type', 'application/json');
      if (opts.auth && opts.auth.user != null) {
        upsertHeader(headers, 'Authorization', 'Basic ' + globalThis.btoa(`${opts.auth.user}:${opts.auth.pass || ''}`));
      }
      return clientSend({
        method: opts.method || 'GET',
        url: withQuery(opts.url || opts.uri, opts.qs),
        headers,
        body: enc && enc.body,
      }).then(raw => {
        let body = raw.body ?? '';
        if (jsonMode) {
          try { body = JSON.parse(raw.body); } catch (_) { /* keep the text */ }
        }
        return { statusCode: raw.code, statusMessage: raw.status, headers: responseHeaderMap(raw), body };
      });
    }
    function request(a, b, c) {
      const { opts, cb } = splitRequestArgs(a, b, c);
      const p = core(opts);
      if (typeof cb === 'function') {
        p.then(res => cb(null, res, res.body), err => cb(err));
        return undefined;
      }
      return p;
    }
    for (const [name, method] of [['get', 'GET'], ['post', 'POST'], ['put', 'PUT'], ['patch', 'PATCH'], ['head', 'HEAD'], ['del', 'DELETE'], ['delete', 'DELETE']]) {
      request[name] = (a, b, c) => {
        const parsed = splitRequestArgs(a, b, c);
        parsed.opts.method = method;
        return request(parsed.opts, parsed.cb);
      };
    }
    return request;
  }

  function makeGot() {
    function got(url, options) {
      options = options || {};
      if (url && typeof url === 'object') {
        options = url;
        url = options.url;
      }
      const headers = pairsFromHeaders(options.headers);
      let body = options.body;
      let parse = false;
      if (options.json != null && typeof options.json !== 'boolean') {
        const enc = encodeBody(options.json);
        body = enc && enc.body;
        parse = true;
        upsertHeader(headers, 'Content-Type', 'application/json');
        upsertHeader(headers, 'Accept', 'application/json');
      } else if (options.json === true) parse = true;
      else if (body != null) {
        const enc = encodeBody(body);
        body = enc && enc.body;
        if (enc && enc.json) upsertHeader(headers, 'Content-Type', 'application/json');
      }
      const throwHttpErrors = options.throwHttpErrors !== false;
      return clientSend({
        method: options.method || 'GET',
        url: withQuery(url, options.searchParams),
        headers,
        body,
      }).then(raw => {
        const res = {
          statusCode: raw.code,
          statusMessage: raw.status,
          headers: responseHeaderMap(raw),
          body: raw.body ?? '',
        };
        if (parse) {
          try { res.body = JSON.parse(raw.body); } catch (_) { /* keep the text */ }
        }
        if (throwHttpErrors && (raw.code < 200 || raw.code >= 300)) {
          const err = new Error(`Response code ${raw.code} (${raw.status})`);
          err.response = res;
          err.statusCode = raw.code;
          throw err;
        }
        return res;
      });
    }
    got.get = (url, opts) => got(url, { ...(opts || {}), method: 'GET' });
    got.post = (url, opts) => got(url, { ...(opts || {}), method: 'POST' });
    got.put = (url, opts) => got(url, { ...(opts || {}), method: 'PUT' });
    got.patch = (url, opts) => got(url, { ...(opts || {}), method: 'PATCH' });
    got.delete = (url, opts) => got(url, { ...(opts || {}), method: 'DELETE' });
    got.extend = defaults => (url, opts) => got(url, { ...(defaults || {}), ...(opts || {}), headers: { ...((defaults || {}).headers), ...((opts || {}).headers) } });
    return got;
  }

  function makeSuperagent() {
    class SuperAgent {
      constructor(method, url) {
        this._method = method;
        this._url = url;
        this._headers = [];
        this._query = {};
        this._body = undefined;
        this._json = false;
      }
      set(k, v) {
        if (v === undefined && k && typeof k === 'object') {
          for (const [hk, hv] of Object.entries(k)) this._headers.push([hk, String(hv)]);
        } else this._headers.push([String(k), String(v)]);
        return this;
      }
      query(q) {
        if (typeof q === 'string') {
          for (const part of q.replace(/^\?/, '').split('&')) {
            if (!part) continue;
            const i = part.indexOf('=');
            this._query[decodeURIComponent(i < 0 ? part : part.slice(0, i))] = i < 0 ? '' : decodeURIComponent(part.slice(i + 1));
          }
        } else if (q && typeof q === 'object') Object.assign(this._query, q);
        return this;
      }
      type(t) {
        const map = { json: 'application/json', form: 'application/x-www-form-urlencoded', text: 'text/plain' };
        this.set('Content-Type', map[t] || t);
        if (t === 'json') this._json = true;
        return this;
      }
      send(body) { this._body = body; return this; }
      auth(user, pass) {
        this.set('Authorization', 'Basic ' + globalThis.btoa(`${user}:${pass || ''}`));
        return this;
      }
      then(resolve, reject) { return this.end().then(resolve, reject); }
      end(cb) {
        const headers = this._headers.map(h => h.slice());
        let body = null;
        if (this._body != null) {
          const asJson = this._json || (typeof this._body === 'object' && !headers.some(([k]) => k.toLowerCase() === 'content-type'));
          if (asJson && typeof this._body !== 'string') {
            body = JSON.stringify(this._body);
            upsertHeader(headers, 'Content-Type', 'application/json');
          } else body = String(this._body);
        }
        const p = clientSend({ method: this._method, url: withQuery(this._url, this._query), headers, body }).then(raw => {
          const headersMap = responseHeaderMap(raw);
          let parsed;
          const ct = headersMap['content-type'] || '';
          if (ct.includes('json')) {
            try { parsed = JSON.parse(raw.body); } catch (_) { parsed = undefined; }
          }
          const res = {
            status: raw.code,
            statusText: raw.status,
            text: raw.body ?? '',
            body: parsed === undefined ? (raw.body ?? '') : parsed,
            headers: headersMap,
            ok: raw.code >= 200 && raw.code < 300,
          };
          if (raw.code >= 400) {
            const err = new Error(`Unsuccessful HTTP response: ${raw.code}`);
            err.status = raw.code;
            err.response = res;
            throw err;
          }
          return res;
        });
        if (typeof cb === 'function') {
          p.then(res => cb(null, res), err => cb(err));
          return undefined;
        }
        return p;
      }
    }
    function superagent(method, url) { return new SuperAgent(String(method || 'GET').toUpperCase(), url); }
    for (const m of ['get', 'post', 'put', 'patch', 'delete', 'head']) {
      superagent[m] = url => new SuperAgent(m.toUpperCase(), url);
    }
    return superagent;
  }

  function makeUndici() {
    const fetch = makeFetch();
    return {
      fetch,
      request(url, opts) {
        return fetch(url, opts).then(res => ({
          statusCode: res.status,
          headers: res.headers,
          body: { text: () => res.text(), json: () => res.json() },
        }));
      },
    };
  }

  // ---------------------------------------------------------------- entry
  globalThis.__lsock = {
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
      const ls = {
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
        vault: new Proxy({}, { get: () => { throw new Error('NotSupported: ls.vault (arrives with vault support)'); } }),
      };
      ls.cookies = Object.assign(requestCookies, { jar: () => makeJar(cookieStore) });
      Object.defineProperty(ls, 'expect', { get: () => scriptRequire('chai').expect, enumerable: true });

      const error = await (async () => {
        try {
          await userFn(ls, ls, ls, scriptRequire, scriptConsole, setTimeoutImpl, clearTimeoutImpl);
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

  requestClients = {
    curl: makeCurl(),
    axios: makeAxios(),
    'node-fetch': makeFetch(),
    'cross-fetch': makeFetch(),
    got: makeGot(),
    undici: makeUndici(),
    request: makeRequest(),
    'postman-request': makeRequest(),
    superagent: makeSuperagent(),
  };
  // Dynamic import() reads this. require() reads `requestClients` directly.
  globalThis.__requestClients = requestClients;

  Object.defineProperty(globalThis, '_', { get: () => scriptRequire('lodash'), configurable: true });
})();
