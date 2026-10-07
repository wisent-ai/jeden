// A real configured hook used by the native CLI journey, not a model or transport substitute.
import { appendFileSync, readFileSync } from 'node:fs';
import { constants } from 'node:os';
import { pathToFileURL } from 'node:url';

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
  return { decision: 'block', reason };
}

export default function register(api) {
  api.on('Stop', decide);
}

const [, invoked] = process.argv;
if (invoked && import.meta.url === pathToFileURL(invoked).href) {
  console.log(JSON.stringify(decide(JSON.parse(readFileSync(process.stdin.fd, 'utf8')))));
}
