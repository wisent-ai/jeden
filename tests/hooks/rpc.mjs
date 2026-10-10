// The actual Jeden subprocess transport shared by lifecycle and refusal journeys.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createInterface } from 'node:readline';

export function startRpc({ binary, cwd, env, report }) {
  const pending = new Map();
  let processFailure;
  const command = { program: binary, args: ['rpc'], cwd, stderr: '' };
  report.commands.push(command);
  const child = spawn(binary, command.args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'] });
  function fail(error) {
    processFailure = error;
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  }
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (text) => { command.stderr += text; });
  child.stdin.on('error', fail);
  child.on('error', (error) => {
    command.error = error.message;
    fail(error);
  });
  const exited = new Promise((resolveExit) => child.on('close', (status, signal) => {
    Object.assign(command, { status, signal });
    fail(new Error(`RPC process ended: status=${status}, signal=${signal}; ${command.stderr}`));
    resolveExit();
  }));
  createInterface({ input: child.stdout }).on('line', (line) => {
    try {
      const frame = JSON.parse(line);
      report.frames.push({ direction: 'response', frame });
      const request = pending.get(frame.id);
      if (!request) return;
      pending.delete(frame.id);
      if (frame.error) request.reject(new Error(`${frame.error.code}: ${frame.error.message}`));
      else request.resolve(frame.result);
    } catch (error) { fail(error); }
  });
  function rpc(method, params) {
    if (processFailure) return Promise.reject(processFailure);
    const id = randomUUID();
    const frame = { id, method, params };
    report.frames.push({ direction: 'request', frame });
    return new Promise((resolveRequest, reject) => {
      pending.set(id, { resolve: resolveRequest, reject });
      child.stdin.write(`${JSON.stringify(frame)}\n`, (error) => {
        if (error) fail(error);
      });
    });
  }
  async function end() {
    if (child.exitCode === null && child.signalCode === null) child.stdin.end();
    await exited;
    return command;
  }
  return { rpc, end, command };
}
