insomnia.test('initialize succeeds', () => {
  insomnia.response.to.have.status(200);
  insomnia.expect(insomnia.response.json().result.serverInfo.name).to.equal('irs-mock-mcp');
});
const sid = insomnia.response.headers.get('mcp-session-id');
insomnia.test('server issued a session id', () => insomnia.expect(sid).to.match(/^sess-/));
insomnia.environment.set('mcp_session', sid);
console.log('session', sid);
