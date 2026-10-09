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
  | { type: 'apikey'; key: string; value: string; addTo?: string | null; disabled?: boolean }
  | { type: 'digest'; username: string; password: string; disabled?: boolean }
  | ({ type: 'oauth2' } & OAuth2Config)
  | ({ type: 'oauth1' } & OAuth1Config)
  | ({ type: 'iam' } & AwsIamConfig)
  | { type: 'netrc'; disabled?: boolean };

export interface OAuth2Config {
  grantType: 'client_credentials' | 'password' | 'authorization_code';
  accessTokenUrl: string;
  authorizationUrl: string;
  clientId: string;
  clientSecret: string;
  scope: string;
  audience: string;
  resource: string;
  username: string;
  password: string;
  redirectUrl: string;
  usePkce: boolean;
  credentialsInBody: boolean;
  tokenPrefix: string;
  disabled?: boolean;
}

export interface OAuth1Config {
  consumerKey: string;
  consumerSecret: string;
  tokenKey: string;
  tokenSecret: string;
  signatureMethod: 'HMAC-SHA1' | 'HMAC-SHA256' | 'PLAINTEXT';
  realm: string;
  callback: string;
  verifier: string;
  nonce: string;
  timestamp: string;
  includeBodyHash: boolean;
  disabled?: boolean;
}

export interface AwsIamConfig {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken: string;
  region: string;
  service: string;
  disabled?: boolean;
}

export interface TokenStatus {
  hasToken: boolean;
  preview?: string | null;
  expiresAt?: number | null;
  hasRefreshToken: boolean;
  scope?: string | null;
  tokenType?: string | null;
}

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

export interface McpSampling {
  enabled: boolean;
  providerId?: string | null;
  model?: string | null;
  maxTokens: number;
}

export interface McpServer extends Meta {
  name: string;
  description: string;
  transport: McpTransport;
  headers: KeyValue[];
  env: KeyValue[];
  roots: { uri: string; name?: string | null }[];
  authentication: Auth;
  sslValidation?: boolean | null;
  sampling: McpSampling;
  /** AI provider that reads tool descriptions to label tools; null sends nothing. */
  actionProviderId?: string | null;
}

// ---- Realtime

export type RtKind = 'websocket' | 'sse' | 'socketio';

export interface RealtimeRequest extends Meta {
  name: string;
  description: string;
  kind: RtKind;
  url: string;
  headers: KeyValue[];
  authentication: Auth;
  payload: string;
  payloadFormat: 'text' | 'json';
  subprotocols: string[];
  event: string;
  namespace: string;
  socketioAuth: string;
  method: string;
  body: string;
}

export interface RtEvent {
  seq: number;
  direction: 'out' | 'in' | 'info' | 'error';
  timestampMs: number;
  kind: string;
  name?: string | null;
  data: string;
  size: number;
}

// ---- gRPC

export interface ProtoFile extends Meta {
  name: string;
  contents: string;
}

export interface GrpcRequest extends Meta {
  name: string;
  description: string;
  url: string;
  schemaSource: 'reflection' | 'protos';
  method: string;
  message: string;
  metadata: KeyValue[];
  timeoutMs: number;
}

export interface GrpcMethod {
  name: string;
  path: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
  inputType: string;
  outputType: string;
  example: unknown;
}

export interface GrpcService {
  name: string;
  methods: GrpcMethod[];
}

export interface GrpcUnaryResult {
  status: { code: number; codeName: string; message: string };
  response?: unknown;
  headers: [string, string][];
  trailers: [string, string][];
  latencyMs: number;
}

// ---- AI

export type KeySource = { type: 'none' } | { type: 'keychain' } | { type: 'env'; var: string } | { type: 'template'; template: string };

export interface LlmProvider extends Meta {
  name: string;
  kind: 'anthropic' | 'openai' | 'ollama' | 'openai-compatible';
  baseUrl: string;
  keySource: KeySource;
  defaultModel: string;
  headers: KeyValue[];
  hasKey: boolean;
  suggestedModels: string[];
}

export interface LlmPromptMessage {
  role: 'user' | 'assistant';
  text: string;
}

export interface LlmRequest extends Meta {
  name: string;
  description: string;
  providerId?: string | null;
  model: string;
  system: string;
  messages: LlmPromptMessage[];
  maxTokens: number;
  temperature?: number | null;
  mcpServerIds: string[];
  maxTurns: number;
  autoApprove: 'none' | 'read-only' | 'all';
  /** Tools the user chose to always allow (`server id::tool name`). */
  alwaysAllow?: string[];
}

export type LlmBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: unknown }
  | { type: 'tool_result'; tool_use_id: string; content: string; is_error: boolean }
  | { type: 'thinking'; thinking: string; signature?: string | null }
  | { type: 'raw'; raw: unknown };

export interface LlmMessage {
  role: 'user' | 'assistant';
  content: LlmBlock[];
}

export interface ToolCallInfo {
  id: string;
  name: string;
  server: string;
  tool: string;
  input: unknown;
  action?: ToolActionInfo | null;
  allowKey?: string;
}

export interface ToolResultInfo {
  id: string;
  isError: boolean;
  denied: boolean;
  text: string;
  structured?: unknown;
  latencyMs: number;
}

export interface LlmRun extends Meta {
  providerName: string;
  model: string;
  transcript: LlmMessage[];
  toolCalls: { call: ToolCallInfo; result?: ToolResultInfo }[];
  inputTokens: number;
  outputTokens: number;
  turns: number;
  stopReason: string;
  ttftMs?: number | null;
  totalMs: number;
  error?: string | null;
  requestBodies: unknown[];
}

export type AgentEvent =
  | { type: 'turnStart'; turn: number }
  | { type: 'stream'; turn: number; event: { type: 'textDelta' | 'thinkingDelta'; text: string } | { type: 'toolUseStart'; id: string; name: string } | { type: 'toolInputDelta'; id: string; partialJson: string } }
  | { type: 'assistantMessage'; turn: number; message: LlmMessage; stopReason: string; usage: { inputTokens: number; outputTokens: number }; ttftMs?: number | null; totalMs: number }
  | { type: 'toolCall'; turn: number; call: ToolCallInfo; needsApproval: boolean }
  | { type: 'toolResult'; turn: number; result: ToolResultInfo }
  | { type: 'done'; usage: { inputTokens: number; outputTokens: number }; turns: number; stopReason: string }
  | { type: 'error'; message: string }
  | { type: 'saved'; run: LlmRun };

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
  kind: 'folder' | 'request' | 'mcp' | 'llm' | 'realtime' | 'grpc';
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
  scriptTimeoutMs: number;
  proxyUrl: string;
  noProxy: string;
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

/** Read / Create / Update / Delete, and how it was decided. */
export interface ToolActionInfo {
  action: 'read' | 'create' | 'update' | 'delete';
  source: 'server' | 'ai' | 'name';
}

export interface ToolView {
  /** What the tool does; null when Logic Socket can't tell. */
  action?: ToolActionInfo | null;
  name: string;
  title?: string | null;
  description?: string | null;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown> | null;
  annotations?: Record<string, unknown> | null;
  displayName: string;
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

// ---- import / export / code / vault / git (Phase 5)

export interface ImportPreview {
  format: string;
  formatLabel: string;
  workspaces: { name: string; scope: string; requests: number; folders: number; environments: number; exists: boolean }[];
  warnings: string[];
}

export interface ImportSummary {
  format: string;
  workspaceIds: string[];
  workspaces: string[];
  requests: number;
  folders: number;
  environments: number;
  warnings: string[];
}

export type ExportFormat = 'logic-socket' | 'postman' | 'har' | 'insomnia-v5';

export interface Exported {
  fileName: string;
  content: string;
  warnings: string[];
}

export type CodeTargetId = 'curl' | 'httpie' | 'js-fetch' | 'python-requests' | 'go' | 'rust-reqwest';

export interface VaultStatus {
  hasKey: boolean;
  sealedValues: number;
}

export interface GitRepo extends Meta {
  name: string;
  path: string;
  remoteUrl: string;
  authorName: string;
  authorEmail: string;
  files: { workspaceId: string; path: string }[];
  workspaces: { workspaceId: string; name: string; path: string }[];
}

export interface GitChange {
  path: string;
  status: 'modified' | 'added' | 'deleted' | 'untracked' | 'conflict' | 'renamed';
  workspaceId?: string | null;
  workspace?: string | null;
}

export interface GitStatus {
  branch: string;
  upstream?: string | null;
  ahead: number;
  behind: number;
  changes: GitChange[];
  hasRemote: boolean;
  hasToken: boolean;
}

export interface GitSyncResult {
  workspaces: string[];
  conflicts: string[];
  warnings: string[];
}

export interface GitCommit {
  hash: string;
  author: string;
  email: string;
  timeMs: number;
  message: string;
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
  graphqlQuery: (requestId: string, query: string, variables?: unknown) =>
    invoke<{ data?: unknown; errors?: unknown }>('graphql_query', { requestId, query, variables: variables ?? null }),
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
  llmProviderList: () => invoke<LlmProvider[]>('llm_provider_list'),
  llmProviderCreate: (name: string, kind: string) => invoke<LlmProvider>('llm_provider_create', { name, kind }),
  llmProviderUpdate: (doc: LlmProvider) => invoke<LlmProvider>('llm_provider_update', { doc }),
  llmProviderDelete: (id: string) => invoke<void>('llm_provider_delete', { id }),
  llmProviderSetKey: (id: string, key: string) => invoke<void>('llm_provider_set_key', { id, key }),
  llmModels: (providerId: string) => invoke<string[]>('llm_models', { providerId }),
  llmRequestCreate: (parentId: string, name: string) => invoke<LlmRequest>('llm_request_create', { parentId, name }),
  llmRequestUpdate: (doc: LlmRequest) => invoke<LlmRequest>('llm_request_update', { doc }),
  llmRuns: (requestId: string) => invoke<LlmRun[]>('llm_runs', { requestId }),
  llmRunStart: (runId: string, requestId: string) => invoke<void>('llm_run_start', { runId, requestId }),
  llmApprove: (runId: string, callId: string, allow: boolean, reason?: string, always?: { requestId: string; allowKey: string }) =>
    invoke<void>('llm_approve', { runId, callId, allow, reason: reason ?? null, always: always ? [always.requestId, always.allowKey] : null }),
  llmCancel: (runId: string) => invoke<void>('llm_cancel', { runId }),
  rtCreate: (parentId: string, kind: RtKind) => invoke<RealtimeRequest>('rt_create', { parentId, kind }),
  rtUpdate: (doc: RealtimeRequest) => invoke<RealtimeRequest>('rt_update', { doc }),
  rtConnect: (id: string) => invoke<{ connected: boolean }>('rt_connect', { id }),
  rtStatus: (id: string) => invoke<{ connected: boolean }>('rt_status', { id }),
  rtSend: (id: string, text: string, event: string | null, ack: boolean) => invoke<number | null>('rt_send', { id, text, event, ack }),
  rtDisconnect: (id: string) => invoke<void>('rt_disconnect', { id }),
  rtLog: (id: string) => invoke<RtEvent[]>('rt_log', { id }),
  oauth2Status: (ownerId: string) => invoke<TokenStatus>('oauth2_status', { ownerId }),
  oauth2Authorize: (ownerId: string) => invoke<TokenStatus>('oauth2_authorize', { ownerId }),
  oauth2Clear: (ownerId: string) => invoke<void>('oauth2_clear', { ownerId }),
  mcpOauthSignIn: (serverId: string) => invoke<TokenStatus>('mcp_oauth_sign_in', { serverId }),
  mcpClassifyTools: (serverId: string) => invoke<number>('mcp_classify_tools', { serverId }),
  protoFileList: (workspaceId: string) => invoke<ProtoFile[]>('proto_file_list', { workspaceId }),
  protoFileCreate: (workspaceId: string, name: string, contents: string) => invoke<ProtoFile>('proto_file_create', { workspaceId, name, contents }),
  protoFileUpdate: (doc: ProtoFile) => invoke<ProtoFile>('proto_file_update', { doc }),
  grpcCreate: (parentId: string) => invoke<GrpcRequest>('grpc_create', { parentId }),
  grpcUpdate: (doc: GrpcRequest) => invoke<GrpcRequest>('grpc_update', { doc }),
  grpcMethods: (id: string, refresh: boolean) => invoke<GrpcService[]>('grpc_methods', { id, refresh }),
  grpcInvoke: (id: string) => invoke<GrpcUnaryResult>('grpc_invoke', { id }),
  grpcStreamStart: (id: string) => invoke<void>('grpc_stream_start', { id }),
  grpcSend: (id: string) => invoke<void>('grpc_send', { id }),
  grpcCommit: (id: string) => invoke<void>('grpc_commit', { id }),
  grpcCancel: (id: string) => invoke<void>('grpc_cancel', { id }),
  grpcLog: (id: string) => invoke<RtEvent[]>('grpc_log', { id }),
  importPreview: (text: string) => invoke<ImportPreview>('import_preview', { text }),
  importApply: (text: string, intoWorkspaceId: string | null, replace: boolean) =>
    invoke<ImportSummary>('import_apply', { text, intoWorkspaceId, replace }),
  fetchText: (url: string) => invoke<string>('fetch_text', { url }),
  exportWorkspace: (workspaceId: string, format: ExportFormat, includePrivate: boolean, includeCookies: boolean) =>
    invoke<Exported>('export_workspace', { workspaceId, format, includePrivate, includeCookies }),
  saveToDownloads: (fileName: string, content: string) => invoke<string>('save_to_downloads', { fileName, content }),
  revealPath: (path: string) => invoke<void>('reveal_path', { path }),
  codeTargets: () => invoke<{ id: CodeTargetId; label: string }[]>('code_targets'),
  codeGenerate: (requestId: string, target: CodeTargetId) => invoke<{ code: string; notes: string[] }>('code_generate', { requestId, target }),
  mcpCodeGenerate: (serverId: string, target: CodeTargetId, message?: unknown, sessionId?: string | null, protocolVersion?: string | null) =>
    invoke<{ code: string; notes: string[] }>('mcp_code_generate', { serverId, target, message: message ?? null, sessionId: sessionId ?? null, protocolVersion: protocolVersion ?? null }),
  envSetVar: (envId: string, key: string, value: unknown, secret: boolean) => invoke<Environment>('env_set_var', { envId, key, value, secret }),
  envReveal: (envId: string, key: string) => invoke<string>('env_reveal', { envId, key }),
  vaultStatus: () => invoke<VaultStatus>('vault_status'),
  vaultExportKey: () => invoke<string>('vault_export_key'),
  vaultImportKey: (key: string) => invoke<void>('vault_import_key', { key }),
  vaultReset: () => invoke<number>('vault_reset'),
  gitRepoList: () => invoke<GitRepo[]>('git_repo_list'),
  gitDefaultDir: (name: string) => invoke<string>('git_default_dir', { name }),
  gitOpen: (dir: string, name: string | null) => invoke<{ repo: GitRepo; sync: GitSyncResult }>('git_open', { dir, name }),
  gitClone: (url: string, dir: string, token: string | null) => invoke<{ repo: GitRepo; sync: GitSyncResult }>('git_clone', { url, dir, token }),
  gitRepoUpdate: (repo: GitRepo) => {
    const { workspaces: _w, ...doc } = repo;
    return invoke<GitRepo>('git_repo_update', { doc });
  },
  gitSetToken: (repoId: string, token: string | null) => invoke<void>('git_set_token', { repoId, token }),
  gitRemove: (repoId: string) => invoke<void>('git_remove', { repoId }),
  gitLink: (repoId: string, workspaceId: string) => invoke<GitRepo>('git_link', { repoId, workspaceId }),
  gitUnlink: (repoId: string, workspaceId: string, deleteFile: boolean) => invoke<GitRepo>('git_unlink', { repoId, workspaceId, deleteFile }),
  gitStatus: (repoId: string) => invoke<GitStatus>('git_status', { repoId }),
  gitDiff: (repoId: string, path: string) => invoke<string>('git_diff', { repoId, path }),
  gitCommit: (repoId: string, message: string, paths: string[]) => invoke<string>('git_commit', { repoId, message, paths }),
  gitPull: (repoId: string) => invoke<GitSyncResult>('git_pull', { repoId }),
  gitPush: (repoId: string) => invoke<string>('git_push', { repoId }),
  gitResolve: (repoId: string, path: string, take: 'ours' | 'theirs') => invoke<GitSyncResult>('git_resolve', { repoId, path, take }),
  gitAbortMerge: (repoId: string) => invoke<void>('git_abort_merge', { repoId }),
  gitDiscard: (repoId: string, path: string) => invoke<GitSyncResult>('git_discard', { repoId, path }),
  gitLog: (repoId: string, limit: number) => invoke<GitCommit[]>('git_log', { repoId, limit }),
  gitBranches: (repoId: string) => invoke<{ current: string; all: string[] }>('git_branches', { repoId }),
  gitCheckout: (repoId: string, branch: string, create: boolean) => invoke<GitSyncResult>('git_checkout', { repoId, branch, create }),
};

export function onRtEvent(cb: (id: string, ev: RtEvent) => void): Promise<UnlistenFn> {
  return listen<{ id: string; event: RtEvent }>('rt-event', e => cb(e.payload.id, e.payload.event));
}

export function onLlmEvent(cb: (runId: string, ev: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<{ runId: string; event: AgentEvent }>('llm-event', e => cb(e.payload.runId, e.payload.event));
}

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

export function onGrpcEvent(cb: (id: string, ev: RtEvent) => void): Promise<UnlistenFn> {
  return listen<{ id: string; event: RtEvent }>('grpc-event', e => cb(e.payload.id, e.payload.event));
}
