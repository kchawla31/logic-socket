const tools = insomnia.response.json().result.tools;
insomnia.test('first page has tools', () => insomnia.expect(tools).to.be.an('array').that.is.not.empty);
insomnia.test('get_weather is read-only', () => {
  const t = tools.find(x => x.name === 'get_weather');
  insomnia.expect(t.annotations.readOnlyHint).to.equal(true);
});
