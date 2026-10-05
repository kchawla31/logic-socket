// Every request in the folder speaks JSON-RPC and carries the MCP session.
insomnia.request.headers.upsert({ key: 'Content-Type', value: 'application/json' });
insomnia.request.headers.upsert({ key: 'Accept', value: 'application/json, text/event-stream' });
const sid = insomnia.environment.get('mcp_session');
if (sid) insomnia.request.headers.upsert({ key: 'Mcp-Session-Id', value: sid });
