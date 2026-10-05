// Typed wrappers over the Tauri commands in src-tauri/src/lib.rs.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface Meta {
  id: string;
  type: string;
  parentId: string | null;
  sortKey: number;
  created: number;
  modified: number;
}

export interface KeyValue {
  id?: string | null;
  name: string;
  value: string;
  description?: string | null;
  disabled?: boolean;
}

export type Auth =
  | { type: 'inherit' }
  | { type: 'none' }
  | { type: 'basic'; username: string; password: string; disabled?: boolean }
  | { type: 'bearer'; token: string; prefix?: string | null; disabled?: boolean }
  | { type: 'apikey'; key: string; value: string; addTo?: string | null; disabled?: boolean };

export interface BodyParam {
  id?: string | null;
  name: string;
  value: string;
  disabled?: boolean;
  type?: string | null;
  fileName?: string | null;
}

export interface Body {
  mimeType?: string | null;
  text?: string | null;
  fileName?: string | null;
  params: BodyParam[];
}

export interface RequestSettings {
  storeCookies: boolean;
  sendCookies: boolean;
  disableRenderBody: boolean;
  encodeUrl: boolean;
  followRedirects: 'global' | 'on' | 'off';
  disableUserAgent: boolean;
}

export interface Request extends Meta {
  name: string;
  description: string;
  method: string;
  url: string;
  parameters: KeyValue[];
  pathParameters: KeyValue[];
  headers: KeyValue[];
  body: Body;
  authentication: Auth;
  preRequestScript?: string | null;
  afterResponseScript?: string | null;
  settings: RequestSettings;
}

export interface Folder extends Meta {
  name: string;
  description: string;
  environment: Record<string, unknown>;
  headers: KeyValue[];
  authentication: Auth;
  preRequestScript?: string | null;
  afterResponseScript?: string | null;
}

export interface TestResult {
  name: string;
  passed: boolean;
  skipped: boolean;
  error?: string | null;
  durationMs: number;
  category: string;
}

export interface ConsoleEntry {
  level: 'log' | 'info' | 'warn' | 'error' | 'debug' | string;
  text: string;
  timestampMs: number;
  source: string;
}

export interface Workspace extends Meta {
  name: string;
  description: string;
  scope: string;
  activeEnvironmentId?: string | null;
}

export interface Environment extends Meta {
  name: string;
  data: Record<string, unknown>;
  color?: string | null;
  isPrivate: boolean;
  secretKeys: string[];
}

export type McpTransport =
  | { kind: 'streamable-http'; url: string }
  | { kind: 'stdio'; command: string; args: string[]; cwd?: string | null };

export interface McpServer extends Meta {
  name: string;
  description: string;
  transport: McpTransport;
  headers: KeyValue[];
  env: KeyValue[];
  roots: { uri: string; name?: string | null }[];
  authentication: Auth;
  sslValidation?: boolean | null;
}

export interface TimelineEntry {
  kind: string;
  text: string;
  atMs: number;
}

export interface Timings {
  dnsMs?: number | null;
  connectMs?: number | null;
  tlsMs?: number | null;
  ttfbMs: number;
  downloadMs: number;
  totalMs: number;
}

export interface ResponseView extends Meta {
  environmentId?: string | null;
  method: string;
  url: string;
  statusCode: number;
  statusMessage: string;
  httpVersion: string;
  headers: KeyValue[];
  contentType: string;
  bytes: number;
  timings: Timings;
  timeline: TimelineEntry[];
  error?: string | null;
  testResults: TestResult[];
  console: ConsoleEntry[];
  scriptError?: string | null;
  bodyText: string;
  bodyTruncated: boolean;
  bodyImage?: string | null;
}

export interface ResponseSummary {
  id: string;
  created: number;
  statusCode: number;
  statusMessage: string;
  error?: string | null;
  totalMs: number;
  bytes: number;
  method: string;
  url: string;
}

export interface TreeNode {
  id: string;
  kind: 'folder' | 'request' | 'mcp';
  name: string;
  method?: string | null;
  transport?: string | null;
  sortKey: number;
  children: TreeNode[];
}

export interface EnvList {
  base: Environment;
  subs: Environment[];
  activeId?: string | null;
}

export interface VarRef {
  name: string;
  resolved: boolean;
  value?: unknown;
  source?: string | null;
}

export interface Preview {
  rendered?: string | null;
  error?: string | null;
  refs: VarRef[];
}

export interface VarInfo {
  name: string;
  value: unknown;
  source: string;
}

export interface Settings extends Meta {
  timeoutMs: number;
  followRedirects: boolean;
  maxRedirects: number;
  validateCertificates: boolean;
  maxHistoryPerRequest: number;
  theme: string;
  maxConcurrentRequests: number;
}

// ---- MCP

export interface ParamRow {
  path: string;
  name: string;
  depth: number;
  typeLabel: string;
  required: boolean;
  default?: unknown;
  enumValues: unknown[];
  description?: string | null;
  constraints: string[];
}

export interface Hints {
  readOnly: boolean;
  destructive: boolean;
  idempotent: boolean;
  openWorld: boolean;
  declared: boolean;
}

export interface ToolView {
  name: string;
  title?: string | null;
  description?: string | null;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown> | null;
  annotations?: Record<string, unknown> | null;
  displayName: string;
  hints: Hints;
  params: ParamRow[];
  outputParams: ParamRow[];
  example: unknown;
  [k: string]: unknown;
}

export interface ServerInfo {
  protocolVersion: string;
  capabilities: Record<string, unknown>;
  serverInfo: { name: string; version: string; title?: string | null; [k: string]: unknown };
  instructions?: string | null;
}

export interface McpStatus {
  connected: boolean;
  server?: ServerInfo | null;
  sessionId?: string | null;
}

export interface LogEntry {
  seq: number;
  direction: 'out' | 'in' | 'info' | 'stderr' | 'error';
  kind: 'request' | 'response' | 'notification' | 'error' | 'other';
  timestampMs: number;
  method?: string | null;
  id?: unknown;
  latencyMs?: number | null;
  message: Record<string, unknown>;
}

export interface Listed<T> {
  items: T[];
  pages: number;
  elapsedMs: number;
}

export interface Resource {
  uri: string;
  name: string;
  title?: string | null;
  description?: string | null;
  mimeType?: string | null;
  size?: number | null;
}

export interface ResourceTemplate {
  uriTemplate: string;
  name: string;
  title?: string | null;
  description?: string | null;
  mimeType?: string | null;
}

export interface Prompt {
  name: string;
  title?: string | null;
  description?: string | null;
  arguments: { name: string; description?: string | null; required: boolean }[];
}

export interface CallToolResult {
  content: Record<string, unknown>[];
  structuredContent?: unknown;
  isError: boolean;
  [k: string]: unknown;
}

export interface Notification {
  method: string;
  params: unknown;
  timestampMs: number;
}

// ---- runner

export interface RunResult {
  iteration: number;
  requestId: string;
  name: string;
  method: string;
  url: string;
  status: number;
  statusMessage: string;
  durationMs: number;
  error?: string | null;
  scriptError?: string | null;
  skipped: boolean;
  tests: TestResult[];
  console: ConsoleEntry[];
  responseId?: string | null;
}

export interface RunSummary {
  iterations: number;
  requests: number;
  requestsFailed: number;
  requestsSkipped: number;
  testsPassed: number;
  testsFailed: number;
  testsSkipped: number;
  durationMs: number;
  bailed: boolean;
  cancelled: boolean;
  results: RunResult[];
}

export type RunEvent =
  | { type: 'runStart'; requests: number; iterations: number }
  | { type: 'iterationStart'; iteration: number }
  | { type: 'requestStart'; iteration: number; requestId: string; name: string; method: string }
  | { type: 'requestEnd'; result: RunResult }
  | { type: 'iterationEnd'; iteration: number }
  | { type: 'warning'; iteration: number; message: string }
  | { type: 'done'; summary: RunSummary };

export function runFailed(r: RunResult): boolean {
  return !r.skipped && (!!r.error || !!r.scriptError || r.tests.some(t => !t.passed && !t.skipped));
}

export const api = {
  treeGet: (workspaceId: string) => invoke<TreeNode[]>('tree_get', { workspaceId }),
  workspaceList: () => invoke<Workspace[]>('workspace_list'),
  workspaceCreate: (name: string) => invoke<Workspace>('workspace_create', { name }),
  workspaceUpdate: (doc: Workspace) => invoke<Workspace>('workspace_update', { doc }),
  docGet: <T>(id: string) => invoke<T>('doc_get', { id }),
  itemDelete: (id: string) => invoke<number>('item_delete', { id }),
  itemRename: (id: string, name: string) => invoke<void>('item_rename', { id, name }),
  itemMove: (id: string, parentId: string, sortKey: number) => invoke<void>('item_move', { id, parentId, sortKey }),
  itemDuplicate: (id: string) => invoke<string>('item_duplicate', { id }),
  requestCreate: (parentId: string, request?: Partial<Request>) =>
    invoke<Request>('request_create', { parentId, request: request ? { ...request } : null }),
  requestUpdate: (doc: Request) => invoke<Request>('request_update', { doc }),
  folderCreate: (parentId: string, name: string) => invoke<Folder>('folder_create', { parentId, name }),
  folderUpdate: (doc: Folder) => invoke<Folder>('folder_update', { doc }),
  mcpServerCreate: (parentId: string, name: string) => invoke<McpServer>('mcp_server_create', { parentId, name }),
  mcpServerUpdate: (doc: McpServer) => invoke<McpServer>('mcp_server_update', { doc }),
  curlParse: (text: string) => invoke<Request>('curl_parse', { text }),
  requestSend: (requestId: string) => invoke<ResponseView>('request_send', { requestId }),
  responseList: (requestId: string) => invoke<ResponseSummary[]>('response_list', { requestId }),
  responseGet: (id: string) => invoke<ResponseView>('response_get', { id }),
  responseClear: (requestId: string) => invoke<void>('response_clear', { requestId }),
  envList: (workspaceId: string) => invoke<EnvList>('env_list', { workspaceId }),
  envCreate: (workspaceId: string, name: string) => invoke<Environment>('env_create', { workspaceId, name }),
  envUpdate: (doc: Environment) => invoke<Environment>('env_update', { doc }),
  envSetActive: (workspaceId: string, envId: string | null) => invoke<void>('env_set_active', { workspaceId, envId }),
  renderPreview: (id: string, text: string) => invoke<Preview>('render_preview', { id, text }),
  contextVars: (id: string) => invoke<VarInfo[]>('context_vars', { id }),
  settingsGet: () => invoke<Settings>('settings_get'),
  settingsUpdate: (doc: Settings) => invoke<Settings>('settings_update', { doc }),
  mcpConnect: (serverId: string) => invoke<McpStatus>('mcp_connect', { serverId }),
  mcpDisconnect: (serverId: string) => invoke<void>('mcp_disconnect', { serverId }),
  mcpStatus: (serverId: string) => invoke<McpStatus>('mcp_status', { serverId }),
  mcpListTools: (serverId: string) => invoke<Listed<ToolView>>('mcp_list', { serverId, kind: 'tools' }),
  mcpListResources: (serverId: string) => invoke<Listed<Resource>>('mcp_list', { serverId, kind: 'resources' }),
  mcpListTemplates: (serverId: string) => invoke<Listed<ResourceTemplate>>('mcp_list', { serverId, kind: 'templates' }),
  mcpListPrompts: (serverId: string) => invoke<Listed<Prompt>>('mcp_list', { serverId, kind: 'prompts' }),
  mcpCallTool: (serverId: string, name: string, args: unknown) =>
    invoke<{ result: CallToolResult; latencyMs: number }>('mcp_call_tool', { serverId, name, args }),
  mcpReadResource: (serverId: string, uri: string) => invoke<unknown>('mcp_read_resource', { serverId, uri }),
  mcpGetPrompt: (serverId: string, name: string, args: unknown) => invoke<unknown>('mcp_get_prompt', { serverId, name, args }),
  mcpPing: (serverId: string) => invoke<number>('mcp_ping', { serverId }),
  mcpValidate: (schema: unknown, args: unknown) => invoke<string[]>('mcp_validate', { schema, args }),
  mcpLog: (serverId: string) => invoke<LogEntry[]>('mcp_log', { serverId }),
  mcpLogClear: (serverId: string) => invoke<void>('mcp_log_clear', { serverId }),
  mcpNotifications: (serverId: string) => invoke<Notification[]>('mcp_notifications', { serverId }),
  runnerStart: (run: { runId?: string; requestIds: string[]; iterations: number; delayMs: number; bail: boolean; dataText?: string | null }) =>
    invoke<string>('runner_start', { run }),
  runnerCancel: (runId: string) => invoke<void>('runner_cancel', { runId }),
  runnerExport: (runId: string, reporter: 'junit' | 'json' | 'spec') => invoke<string>('runner_export', { runId, reporter }),
};

export function onRunnerEvent(cb: (runId: string, ev: RunEvent) => void): Promise<UnlistenFn> {
  return listen<{ runId: string; event: RunEvent }>('runner-event', e => cb(e.payload.runId, e.payload.event));
}

export function onDbChanged(cb: () => void): Promise<UnlistenFn> {
  return listen('db-changed', () => cb());
}

export function onMcpLog(cb: (serverId: string, entry: LogEntry) => void): Promise<UnlistenFn> {
  return listen<{ serverId: string; entry: LogEntry }>('mcp-log', ev => cb(ev.payload.serverId, ev.payload.entry));
}

export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
