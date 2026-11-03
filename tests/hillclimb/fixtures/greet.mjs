// The real fixture CLI starts with one complete greeting command, without help.
const [, , name, extra] = process.argv;
if (name === undefined || extra !== undefined) throw new Error('Expected exactly one name.');
console.log(`Hello, ${name}`);
