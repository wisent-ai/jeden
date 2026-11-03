// Real test of the published quality report's privacy check, which reads the
// report's structure instead of searching for marker words. It signs real
// evidence with a fresh Ed25519 key, runs the built
// `jeden-quality-report build` and `privacy-scan`, and requires:
//   - clean evidence builds a report, and `privacy-scan` accepts it;
//   - an area holding a path (`/`) is refused at build, naming the field and
//     never the value;
//   - a prerequisite holding this machine's own IP address is refused by
//     `privacy-scan` of a machine report carrying it.
// Every command, its exit status and output go to the run's report.json under
// target/real-tests/report/<run>.
//
// Usage: node tests/report/privacy.mjs   (JEDEN_QUALITY_REPORT selects the
//   binary, default target/debug/jeden-quality-report)
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(new URL('../..', import.meta.url).pathname);
const binary = process.env.JEDEN_QUALITY_REPORT ?? join(root, 'target/debug/jeden-quality-report');
const run = `${new Date().toISOString().replace(/[-:.]/g, '')}-${process.pid}`;
const area = join(root, 'target/real-tests/report', run);
mkdirSync(area, { recursive: true });
const revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
const report = { revision, binary, commands: [], state: 'running' };
const save = () => writeFileSync(join(area, 'report.json'), JSON.stringify(report));

// What the schemas and the system declare, read rather than restated.
const schema = JSON.parse(readFileSync(join(root, 'report/schema/quality-evidence-v1.schema.json'), 'utf8'));
const schemaVersion = schema.properties.schema_version.const;
const now = spawnSync('date', ['+%s'], { encoding: 'utf8' }).stdout.trim();
const EVIDENCE_TYPE = 'application/vnd.jeden.quality-evidence.v1+json';
const REPORT_TYPE = 'application/vnd.jeden.quality-report.v1+json';

const pae = (type, payload) => Buffer.concat([
  Buffer.from(`DSSEv1 ${Buffer.byteLength(type)} ${type} ${payload.length} `),
  payload,
]);
const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const rawPublic = Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url');
const keyid = `ed25519:${createHash('sha256').update(rawPublic).digest('hex')}`;
const envelope = (type, payload) => ({
  payloadType: type,
  payload: payload.toString('base64'),
  signatures: [{ keyid, sig: sign(null, pae(type, payload), privateKey).toString('base64') }],
});
const digest = (text) => `sha256:${createHash('sha256').update(text).digest('hex')}`;
// An address this machine really has: the kind of value a report must not
// publish, measured here instead of written into the test.
const ownAddress = Object.values(networkInterfaces()).flat()
  .find((entry) => entry && entry.family === 'IPv4')?.address;
assert.ok(ownAddress, 'this machine reports an IPv4 interface address');

// The evidence payload in the struct's own field order: the aggregator
// requires canonical JSON. One case ran and passed.
const cases = ['clean-build'];
const evidence = (areaName) => Buffer.from(JSON.stringify({
  schema_version: schemaVersion,
  area: areaName,
  status: 'Passed',
  metrics: [{ name: 'cases-passed', numerator: cases.length, denominator: cases.length }],
  environment: 'ci',
  revision,
  evidence_uri: `artifact://runs/${run}`,
  evidence_digest: digest('evidence'),
  artifact_digests: {},
  prerequisites: [],
  observed_at_epoch_seconds: Number(now),
}));

function quality(name, args, expectedSuccess) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ name, program: binary, args, status: result.status,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  save();
  assert.ifError(result.error);
  // A process succeeded when it exited on its own with status zero; one a
  // signal ended has no status and did not.
  const succeeded = result.signal === null && !result.status;
  assert.equal(succeeded, expectedSuccess, `${name}: ${result.stderr}`);
  return result;
}

function build(name, payload) {
  const dir = join(area, name);
  mkdirSync(join(dir, 'evidence'), { recursive: true });
  writeFileSync(join(dir, 'evidence/one.json'), JSON.stringify(envelope(EVIDENCE_TYPE, payload)));
  writeFileSync(join(dir, 'root.json'), JSON.stringify({ ed25519_keys: { [keyid]: rawPublic.toString('hex') } }));
  return {
    dir,
    // An Ed25519 seed is as long as its public key.
    args: ['build', join(dir, 'evidence'), join(dir, 'root.json'), randomBytes(rawPublic.length).toString('hex'),
      revision, 'ci', now, now, join(dir, 'machine.json'), join(dir, 'human.md')],
  };
}

const clean = build('clean', evidence('quality-gate'));
quality('build-clean', clean.args, true);
quality('scan-clean', ['privacy-scan', join(clean.dir, 'machine.json')], true);

const pathArea = build('path-area', evidence('home/operator/notes'));
const refused = quality('build-path-area', pathArea.args, false);
assert.match(refused.stderr, /privacy scan rejected the area: it holds a path, address or assignment character/);
assert.doesNotMatch(refused.stderr, /operator/, 'the refusal never quotes the rejected value');

// A machine report whose prerequisite holds this machine's address:
// privacy-scan reads its structure without a trust root.
const machine = JSON.parse(readFileSync(join(clean.dir, 'machine.json'), 'utf8'));
const published = JSON.parse(Buffer.from(machine.payload, 'base64').toString('utf8'));
const [summary] = published.evidence;
published.status = 'ExternalBlocked';
summary.status = 'ExternalBlocked';
summary.prerequisites = [`runner at ${ownAddress}`];
const tampered = join(area, 'ip-prerequisite.json');
writeFileSync(tampered, JSON.stringify(envelope(REPORT_TYPE, Buffer.from(JSON.stringify(published)))));
const ip = quality('scan-ip-prerequisite', ['privacy-scan', tampered], false);
assert.match(ip.stderr, /privacy scan rejected the prerequisite: it holds an IP address/);
assert.ok(!ip.stderr.includes(ownAddress), 'the refusal never quotes the address');

report.state = 'passed';
save();
console.log(`evidence: ${join(area, 'report.json')}`);
