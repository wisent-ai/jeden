// A real configured hook used by the native CLI journey, not a model or transport substitute.
import { appendFileSync, chmodSync, closeSync, constants as files, openSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';
import { isAbsolute, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

function refuseRecording(payload, receipt, reason) {
  const root = process.env.JEDEN_SESSION_ROOT;
  if (!root) throw new Error('recording refusal requires an isolated JEDEN_SESSION_ROOT');
  const transcript = realpathSync(payload.transcript_path);
  const within = relative(realpathSync(root), transcript);
  if (!within || isAbsolute(within) || within.split(sep).includes('..')) {
    throw new Error(`refusing to change transcript permissions outside ${root}: ${transcript}`);
  }
  const mode = statSync(transcript).mode;
  writeFileSync(`${receipt}.before`, readFileSync(transcript));
  writeFileSync(`${receipt}.recording.json`, JSON.stringify({ transcript, mode, reason }));
  chmodSync(transcript, mode & ~(files.S_IWUSR | files.S_IWGRP | files.S_IWOTH));
  let cause;
  try {
    const descriptor = openSync(transcript, 'a');
    closeSync(descriptor);
  } catch (error) {
    cause = { code: error.code, errno: error.errno, message: error.message };
  }
  if (!cause) throw new Error(`the isolated transcript still allows append: ${transcript}`);
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
  if (mode === 'recording') refuseRecording(payload, receipt, reason);
  return { decision: 'block', reason };
}

export default function register(api) {
  api.on('Stop', decide);
}

const [, invoked] = process.argv;
if (invoked && import.meta.url === pathToFileURL(invoked).href) {
  console.log(JSON.stringify(decide(JSON.parse(readFileSync(process.stdin.fd, 'utf8')))));
}
