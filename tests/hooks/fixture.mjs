// A real configured hook used by the native CLI journey, not a model or transport substitute.
import { appendFileSync, chmodSync, closeSync, constants as files, openSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';
import { dirname, isAbsolute, join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

function refuseRecording(receipt, reason, boundary) {
  const root = process.env.JEDEN_SESSION_ROOT;
  if (!root) throw new Error('recording refusal requires an isolated JEDEN_SESSION_ROOT');
  const path = realpathSync(boundary.path);
  const within = relative(realpathSync(root), path);
  if (!within || isAbsolute(within) || within.split(sep).includes('..')) {
    throw new Error(`refusing to change permissions outside ${root}: ${path}`);
  }
  const mode = statSync(path).mode;
  writeFileSync(`${receipt}.before`, readFileSync(path));
  writeFileSync(`${receipt}.recording.json`, JSON.stringify({ path, mode, reason }));
  chmodSync(path, mode & ~boundary.permission);
  let cause;
  try {
    const descriptor = openSync(path, boundary.flags);
    closeSync(descriptor);
  } catch (error) {
    cause = { code: error.code, errno: error.errno, message: error.message };
  }
  if (!cause) throw new Error(`the isolated file still allows ${boundary.flags}: ${path}`);
  writeFileSync(`${receipt}.cause.json`, JSON.stringify(cause));
}

export function decide(payload) {
  const mode = process.env.JEDEN_TEST_HOOK_MODE;
  const rejected = process.env.JEDEN_TEST_REJECTED_TEXT;
  const accepted = process.env.JEDEN_TEST_ACCEPTED_TEXT;
  const receipt = process.env.JEDEN_TEST_HOOK_RECEIPT;
  if (!mode || !rejected || !accepted || !receipt) throw new Error('hook journey inputs are required');
  appendFileSync(receipt, `${JSON.stringify(payload)}\n`);
  if (!payload.last_assistant_message?.includes(rejected)) return { decision: 'approve' };
  const reason = `Qualification policy refused this text. Continue with a final answer containing only ${accepted}; do not repeat the refused text.`;
  if (mode === 'extension') throw new Error(reason);
  if (mode === 'signal') process.kill(process.pid, 'SIGTERM');
  if (mode === 'positive') {
    process.exitCode = constants.errno.EPERM;
    return { decision: 'approve' };
  }
  if (mode === 'recording') {
    refuseRecording(receipt, reason, {
      path: payload.transcript_path, flags: 'a',
      permission: files.S_IWUSR | files.S_IWGRP | files.S_IWOTH,
    });
  }
  if (mode === 'completion-state') {
    refuseRecording(receipt, reason, {
      path: join(dirname(payload.transcript_path), 'completion.json'), flags: 'r',
      permission: files.S_IRUSR | files.S_IRGRP | files.S_IROTH,
    });
  }
  return { decision: 'block', reason };
}

export default function register(api) {
  api.on('Stop', decide);
}

const [, invoked] = process.argv;
if (invoked && import.meta.url === pathToFileURL(invoked).href) {
  console.log(JSON.stringify(decide(JSON.parse(readFileSync(process.stdin.fd, 'utf8')))));
}
