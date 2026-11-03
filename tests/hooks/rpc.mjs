// The actual Jeden subprocess transport shared by lifecycle and refusal journeys.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createInterface } from 'node:readline';

export function startRpc({ binary, cwd, env, report, redactFrame }) {
  const pending = new Map();
  const observers = new Set();
  let processFailure;
  const command = { program: binary, args: ['rpc'], cwd, stderr: '', protocolErrors: [] };
  report.commands.push(command);
  // Redact retained evidence without changing the frame sent to Jeden.
  const recordFrame = (direction, frame) => {
    report.frames.push({ direction, frame: redactFrame ? redactFrame(frame) : frame });
  };
  const child = spawn(binary, command.args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'] });
  // The process a journey reads from the kernel (its open files) while it runs.
  command.pid = child.pid;
  function fail(error) {
    processFailure = error;
    for (const request of pending.values()) request.reject(error);
    pending.clear();
    for (const observer of observers) observer.reject(error);
    observers.clear();
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
      recordFrame('response', frame);
      for (const observer of observers) {
        if (!observer.predicate(frame)) continue;
        observers.delete(observer);
        observer.resolve(frame);
      }
      const request = pending.get(frame.id);
      if (!request) return;
      pending.delete(frame.id);
      if (frame.error) {
        const error = new Error(`${frame.error.code}: ${frame.error.message}`);
        error.code = frame.error.code;
        error.data = frame.error.data;
        request.reject(error);
      } else request.resolve(frame.result);
    } catch (error) {
      command.protocolErrors.push({ line, cause: String(error) });
      fail(error);
    }
  });
  // One request, with the id it was sent under, so a journey can name it in a
  // later request (context/cancel takes the id of the request it cancels).
  function request(method, params) {
    if (processFailure) return { id: undefined, response: Promise.reject(processFailure) };
    const id = randomUUID();
    const frame = { id, method, params };
    recordFrame('request', frame);
    const response = new Promise((resolveRequest, reject) => {
      pending.set(id, { resolve: resolveRequest, reject });
      child.stdin.write(`${JSON.stringify(frame)}\n`, (error) => {
        if (error) fail(error);
      });
    });
    return { id, response };
  }
  function rpc(method, params) {
    return request(method, params).response;
  }
  // Observe actual nonterminal work, with the owned request's terminal result
  // preventing a failed operation from leaving the observer waiting forever.
  function observeFrame(predicate, response) {
    return new Promise((resolve, reject) => {
      const observer = { predicate, resolve, reject };
      observers.add(observer);
      response.then(
        (result) => {
          if (observers.delete(observer)) reject(new Error(
            'The RPC request completed before its expected event was observed', { cause: result }));
        },
        (error) => { if (observers.delete(observer)) reject(error); },
      );
      if (processFailure && observers.delete(observer)) reject(processFailure);
    });
  }
  async function end() {
    if (child.exitCode === null && child.signalCode === null) child.stdin.end();
    await exited;
    if (command.protocolErrors.length) {
      throw new Error(`RPC protocol failed: ${command.protocolErrors.map(({ cause }) => cause).join('; ')}`);
    }
    return command;
  }
  return { rpc, request, observeFrame, end, command };
}
