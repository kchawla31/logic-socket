// Every request in the folder speaks JSON-RPC and carries the MCP session.
ls.request.headers.upsert({ key: 'Content-Type', value: 'application/json' });
ls.request.headers.upsert({ key: 'Accept', value: 'application/json, text/event-stream' });
const sid = ls.environment.get('mcp_session');
if (sid) ls.request.headers.upsert({ key: 'Mcp-Session-Id', value: sid });
