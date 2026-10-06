/** Logic Socket scripting API (`ls.*`), used for editor completions. `pm.*` and `$.*` are aliases. */
export interface ApiEntry {
  path: string;
  /** Call signature shown after the name, e.g. `(key, value)`; empty for properties. */
  args?: string;
  doc: string;
}

const scope = (name: string, label: string): ApiEntry[] => [
  { path: `ls.${name}.get`, args: '(key)', doc: `Read a variable from ${label}` },
  { path: `ls.${name}.set`, args: '(key, value)', doc: `Write a variable to ${label} (saved after the script)` },
  { path: `ls.${name}.has`, args: '(key)', doc: `Is the variable defined in ${label}?` },
  { path: `ls.${name}.unset`, args: '(key)', doc: `Remove a variable from ${label}` },
  { path: `ls.${name}.clear`, args: '()', doc: `Remove every variable from ${label}` },
  { path: `ls.${name}.replaceIn`, args: '(template)', doc: `Render {{ variables }} in a string using ${label}` },
  { path: `ls.${name}.toObject`, args: '()', doc: `All variables in ${label} as an object` },
  { path: `ls.${name}.name`, doc: `Name of ${label}` },
];

const headers = (base: string): ApiEntry[] => [
  { path: `${base}.headers.get`, args: '(name)', doc: 'Header value (case-insensitive)' },
  { path: `${base}.headers.has`, args: '(name, value?)', doc: 'Is the header present?' },
  { path: `${base}.headers.toObject`, args: '()', doc: 'Headers as { name: value }' },
  { path: `${base}.headers.each`, args: '(fn)', doc: 'Visit every header' },
  { path: `${base}.headers.count`, args: '()', doc: 'Number of headers' },
];

export const SCRIPT_API: ApiEntry[] = [
  { path: 'ls.test', args: "('name', () => { … })", doc: 'Define a test; failures show in the Tests tab' },
  { path: 'ls.test.skip', args: "('name', () => { … })", doc: 'Define a skipped test' },
  { path: 'ls.expect', args: '(value)', doc: 'Chai expect assertion' },
  { path: 'ls.sendRequest', args: '(request, callback?)', doc: 'Send another HTTP request (returns a promise of the response)' },

  ...scope('environment', 'the active environment'),
  ...scope('baseEnvironment', 'the base environment'),
  ...scope('collectionVariables', 'the base environment'),
  ...scope('globals', 'the global environment'),
  ...scope('iterationData', 'the current runner iteration'),
  { path: 'ls.variables.get', args: '(key)', doc: 'Resolved variable (local → iteration → folder → environment → global)' },
  { path: 'ls.variables.set', args: '(key, value)', doc: 'Set a local variable for this run' },
  { path: 'ls.variables.has', args: '(key)', doc: 'Is the variable defined in any scope?' },
  { path: 'ls.variables.unset', args: '(key)', doc: 'Remove a local variable' },
  { path: 'ls.variables.replaceIn', args: '(template)', doc: 'Render {{ variables }} in a string' },
  { path: 'ls.variables.toObject', args: '()', doc: 'Every resolved variable as an object' },

  { path: 'ls.request.url', doc: 'The request URL (object; toString() gives the text)' },
  { path: 'ls.request.url.toString', args: '()', doc: 'Full URL as text' },
  { path: 'ls.request.url.getHost', args: '()', doc: 'Host name' },
  { path: 'ls.request.url.getPath', args: '()', doc: 'Path without the query' },
  { path: 'ls.request.url.getPathWithQuery', args: '()', doc: 'Path and query string' },
  { path: 'ls.request.url.getQueryString', args: '()', doc: 'Query string' },
  { path: 'ls.request.url.addQueryParams', args: "([{ key, value }])", doc: 'Append query parameters' },
  { path: 'ls.request.url.removeQueryParams', args: "(['key'])", doc: 'Remove query parameters' },
  { path: 'ls.request.url.update', args: '(url)', doc: 'Replace the URL' },
  { path: 'ls.request.method', doc: 'HTTP method (assignable)' },
  { path: 'ls.request.name', doc: 'Request name' },
  { path: 'ls.request.body', doc: 'Request body ({ mode, raw, urlencoded, formdata })' },
  { path: 'ls.request.body.raw', doc: 'Raw body text' },
  { path: 'ls.request.auth', doc: 'Request authentication' },
  { path: 'ls.request.auth.update', args: '(params, type)', doc: 'Change the auth type and parameters' },
  { path: 'ls.request.headers.upsert', args: "({ key, value })", doc: 'Add or replace a header' },
  { path: 'ls.request.headers.add', args: "({ key, value })", doc: 'Add a header' },
  { path: 'ls.request.headers.remove', args: "('name')", doc: 'Remove a header' },
  ...headers('ls.request'),
  { path: 'ls.request.addHeader', args: "({ key, value })", doc: 'Add a header' },
  { path: 'ls.request.upsertHeader', args: "({ key, value })", doc: 'Add or replace a header' },
  { path: 'ls.request.removeHeader', args: "('name')", doc: 'Remove a header' },
  { path: 'ls.request.addQueryParams', args: "([{ key, value }])", doc: 'Append query parameters' },
  { path: 'ls.request.removeQueryParams', args: "(['key'])", doc: 'Remove query parameters' },

  { path: 'ls.response.code', doc: 'Status code, e.g. 200' },
  { path: 'ls.response.status', doc: 'Status text, e.g. "OK"' },
  { path: 'ls.response.responseTime', doc: 'Total time in milliseconds' },
  { path: 'ls.response.json', args: '()', doc: 'Body parsed as JSON' },
  { path: 'ls.response.text', args: '()', doc: 'Body as text' },
  { path: 'ls.response.size', args: '()', doc: 'Body and header sizes in bytes' },
  { path: 'ls.response.reason', args: '()', doc: 'Status text' },
  { path: 'ls.response.contentInfo', args: '()', doc: 'MIME type, charset and file name' },
  ...headers('ls.response'),
  { path: 'ls.response.cookies.get', args: "('name')", doc: 'Value of a cookie set by the response' },
  { path: 'ls.response.to.have.status', args: '(code)', doc: 'Assert the status code' },
  { path: 'ls.response.to.have.header', args: "('name', value?)", doc: 'Assert a header' },
  { path: 'ls.response.to.have.body', args: '(text)', doc: 'Assert the body' },
  { path: 'ls.response.to.have.jsonBody', args: '(path?, value?)', doc: 'Assert a JSON body' },
  { path: 'ls.response.to.be.success', doc: 'Assert a 2xx status' },
  { path: 'ls.response.to.be.json', doc: 'Assert the body is JSON' },

  { path: 'ls.cookies.get', args: "('name')", doc: 'Cookie sent with this request' },
  { path: 'ls.cookies.has', args: "('name')", doc: 'Is the cookie sent with this request?' },
  { path: 'ls.cookies.toObject', args: '()', doc: 'Request cookies as { name: value }' },
  { path: 'ls.cookies.jar', args: '()', doc: 'The cookie jar: set / get / getAll / unset / clear' },

  { path: 'ls.info.eventName', doc: '"prerequest" or "test"' },
  { path: 'ls.info.requestName', doc: 'Name of the running request' },
  { path: 'ls.info.requestId', doc: 'Id of the running request' },
  { path: 'ls.info.iteration', doc: 'Current runner iteration (0-based)' },
  { path: 'ls.info.iterationCount', doc: 'Total runner iterations' },
  { path: 'ls.execution.skipRequest', args: '()', doc: 'Skip sending this request (pre-request only)' },
  { path: 'ls.execution.setNextRequest', args: "('name or id' | null)", doc: 'Choose the next request in a collection run (null stops)' },
  { path: 'ls.execution.location', doc: 'Folder path of the running request' },
  { path: 'ls.parentFolders.get', args: "('folder name')", doc: 'A parent folder and its environment' },
  { path: 'ls.parentFolders.toObject', args: '()', doc: 'All parent folders' },
];
