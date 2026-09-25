// Runs the matrix under Node (worker_threads) or Bun (Web Workers): `node run-native.mjs [--quick]`.

import fs from 'node:fs';
import os from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { runBench } from './orchestrator.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const args = new Map(process.argv.slice(2).map((arg) => arg.replace(/^--/, '').split('=')));
const bun = globalThis.Bun !== undefined;
const runtime = bun ? 'bun' : 'node';
const WorkerThread = bun ? null : (await import('node:worker_threads')).Worker;

const platform = {
  runtime,
  version: bun ? Bun.version : process.version,
  opfs: false,
  importGlue: (variant) => import(pathToFileURL(`${here}pkg/${variant}/threadsafe_bench.js`).href),
  compile: (variant) => WebAssembly.compile(fs.readFileSync(`${here}pkg/${variant}/threadsafe_bench_bg.wasm`)),
  glueUrl: (variant) => pathToFileURL(`${here}pkg/${variant}/threadsafe_bench.js`).href,
  spawn() {
    const url = new URL('./worker.mjs', import.meta.url);
    if (bun) {
      const worker = new Worker(url.href, { type: 'module' });
      return { worker, post: (m, t) => worker.postMessage(m, t), onMessage: (f) => { worker.onmessage = (e) => f(e.data); } };
    }
    const worker = new WorkerThread(url);
    return { worker, post: (m, t) => worker.postMessage(m, t), onMessage: (f) => worker.on('message', f) };
  },
  terminate: (handle) => handle.worker.terminate(),
  channel: () => new MessageChannel(),
};

const variants = (args.get('variants') ?? 'threadsafe,cipher,single').split(',');
const plan = args.has('plan') ? args.get('plan').split(',') : undefined;
const workloads = args.has('workloads') ? args.get('workloads').split(',') : undefined;
fs.mkdirSync(`${here}results`, { recursive: true });
const out = `${here}results/${runtime}${args.has('variants') ? `-${args.get('variants').replaceAll(',', '-')}` : ''}${args.has('no-memstatus') ? '-nomemstatus' : ''}${args.has('tag') ? `-${args.get('tag')}` : ''}${args.has('quick') ? '-quick' : ''}.jsonl`;
fs.writeFileSync(out, JSON.stringify({ environment: { runtime, version: platform.version, cpus: os.cpus().length, cpu: os.cpus()[0].model, loadavg: os.loadavg(), date: new Date().toISOString() } }) + '\n');

await runBench(platform, {
  variants,
  plan,
  workloads,
  quick: args.has('quick'),
  memstatus: !args.has('no-memstatus'),
  emit: (row) => fs.appendFileSync(out, JSON.stringify({ ...row, loadavg1: os.loadavg()[0] }) + '\n'),
  log: (line) => console.log(line),
});
console.log(`results in ${out}`);
process.exit(0);
