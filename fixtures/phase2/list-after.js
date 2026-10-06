const tools = ls.response.json().result.tools;
ls.test('first page has tools', () => ls.expect(tools).to.be.an('array').that.is.not.empty);
ls.test('get_weather is read-only', () => {
  const t = tools.find(x => x.name === 'get_weather');
  ls.expect(t.annotations.readOnlyHint).to.equal(true);
});
