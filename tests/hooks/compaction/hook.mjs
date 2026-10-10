// The SessionStart command the automatic-compaction journey configures. It is
// a real hook process: it reads the payload Jeden writes, answers nothing for
// the lifecycle source it was not told to refuse, and for the declared source
// it exits unsuccessfully with the journey's marker on stderr. When the
// control file asks for it, it first takes write permission off the session's
// own transcript inside the isolated session root and proves the append is
// refused, so the failure that follows has to be recorded on a file that
// genuinely cannot take it.
//
// Usage (as a hooks.json command): node hook.mjs CONTROL.json
import { chmodSync, closeSync, constants as files, openSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';
import { isAbsolute, relative, sep } from 'node:path';

const [, , controlPath] = process.argv;
if (!controlPath) throw new Error('usage: node hook.mjs CONTROL.json');
const control = JSON.parse(readFileSync(controlPath, 'utf8'));
const payload = JSON.parse(readFileSync(process.stdin.fd, 'utf8'));
writeFileSync(`${controlPath}.${payload.source}.payload.json`, JSON.stringify(payload));
if (payload.source !== control.refuseSource) process.exit();

if (control.lockTranscript) {
  const root = realpathSync(control.sessionRoot);
  const path = realpathSync(payload.transcript_path);
  const within = relative(root, path);
  if (!within || isAbsolute(within) || within.split(sep).includes('..')) {
    throw new Error(`refusing to change permissions outside the isolated session root ${root}: ${path}`);
  }
  const mode = statSync(path).mode;
  writeFileSync(`${controlPath}.before`, readFileSync(path));
  writeFileSync(`${controlPath}.lock.json`, JSON.stringify({ path, mode }));
  chmodSync(path, mode & ~(files.S_IWUSR | files.S_IWGRP | files.S_IWOTH));
  let cause;
  try {
    closeSync(openSync(path, 'a'));
  } catch (error) {
    cause = { code: error.code, errno: error.errno, message: error.message };
  }
  if (!cause) throw new Error(`the isolated transcript still accepts appends: ${path}`);
  writeFileSync(`${controlPath}.cause.json`, JSON.stringify(cause));
}
console.error(control.marker);
process.exitCode = constants.errno.EPERM;
