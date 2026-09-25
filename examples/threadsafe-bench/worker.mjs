// One benchmark worker: loads the shared module, then answers numbered commands.

const node = !globalThis.Bun && globalThis.process?.versions?.node !== undefined;
const parent = node ? (await import('node:worker_threads')).parentPort : null;
const post = (message) => (node ? parent.postMessage(message) : self.postMessage(message));
const listen = (port, handler) => {
  if (typeof port.on === 'function') port.on('message', handler);
  else port.onmessage = (event) => handler(event.data);
};
const now = () => performance.timeOrigin + performance.now();

let bench;
let stage;
const markStage = (worker, value) => { if (stage && worker !== undefined) Atomics.store(stage, 128 + worker, value); };

async function client({ port, point, parties, workload, ops }) {
  const waiting = new Map();
  listen(port, (message) => {
    waiting.get(message.i)?.(message.v);
    waiting.delete(message.i);
  });
  bench.start_line(point, parties);
  const start = now();
  for (let i = 0; i < ops; i++) {
    await new Promise((resolve) => {
      waiting.set(i, resolve);
      port.postMessage({ i, w: workload, s: point * 1_000_003 + i });
    });
  }
  const end = now();
  port.postMessage({ close: true });
  port.close();
  return [start, end, 0];
}

function serve({ port }) {
  listen(port, (message) => {
    if (message.close) {
      port.close();
      return;
    }
    port.postMessage({ i: message.i, v: bench.serve(message.w, message.s) });
  });
}

async function handle({ id, cmd, args }) {
  try {
    let result = null;
    switch (cmd) {
      case 'init':
        bench = await import(args.glue);
        await bench.default({ module_or_path: args.module, memory: args.memory });
        stage = new Uint32Array(args.memory.buffer, bench.progress_address(), 192);
        break;
      case 'setup':
        if (args.modality === 3) await bench.setup_sahpool(args.worker);
        else bench.setup(args.modality, args.worker, args.nomutex);
        break;
      case 'teardown':
        bench.teardown(args.modality, args.worker, args.nomutex);
        break;
      case 'run':
        result = Array.from(bench.run(...args));
        markStage(args[6], 2);
        post({ id, ok: true, result });
        markStage(args[6], 3);
        return;
      case 'ping':
        break;
      case 'stress':
        result = bench.mutex_stress(...args);
        break;
      case 'serve':
        serve(args);
        break;
      case 'client':
        result = await client(args);
        break;
      default:
        throw new Error(`unknown command ${cmd}`);
    }
    post({ id, ok: true, result });
  } catch (error) {
    post({ id, ok: false, error: String(error?.stack ?? error) });
  }
}

if (node) parent.on('message', handle);
else self.onmessage = (event) => handle(event.data);
