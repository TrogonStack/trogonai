function runChild() {
  return 'child executed';
}

console.log(JSON.stringify({ subject: runChild(), coverage: Boolean(process.env.NODE_V8_COVERAGE) }));
