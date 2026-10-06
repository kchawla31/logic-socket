ls.test('initialize succeeds', () => {
  ls.response.to.have.status(200);
  ls.expect(ls.response.json().result.serverInfo.name).to.equal('lsock-mock-mcp');
});
const sid = ls.response.headers.get('mcp-session-id');
ls.test('server issued a session id', () => ls.expect(sid).to.match(/^sess-/));
ls.environment.set('mcp_session', sid);
console.log('session', sid);
