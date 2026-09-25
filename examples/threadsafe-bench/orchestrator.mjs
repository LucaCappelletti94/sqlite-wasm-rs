// Runs the scaling matrix on any runtime through a small platform adapter.

export const MODALITIES = {
  memvfs_shared: 0,
  memvfs_per_worker: 1,
  memory_per_worker: 2,
  sahpool_per_worker: 3,
  cipher_shared: 4,
  cipher_per_worker: 5,
  sqlcipher_shared: 6,
};

// Operation totals are split evenly across workers, so every total divides by 32.
export const WORKLOADS = {
  point: { code: 0, ops: 256_000, batchable: true },
  range: { code: 1, ops: 4_800, batchable: true },
  scan: { code: 2, ops: 32, batchable: false },
  sort: { code: 3, ops: 3_200, batchable: true },
  fts: { code: 4, ops: 3_200, batchable: true },
  // Each OPFS commit waits for a real flush (20 to 45 ms in Chrome), so sahpool writes fewer.
  insert: { code: 5, ops: 9_600, opfsOps: 256, batchable: false },
  mix: { code: 6, ops: 160_000, opfsOps: 3_200, batchable: false },
  // Only for SQLCipher builds, where a keyed open runs PBKDF2 and every cold page is decrypted.
  // Each keyed open costs most of a second at one worker, so it runs fewer ops and rounds.
  keyed_open: { code: 7, ops: 32, rounds: 3, batchable: false, variants: ['sqlcipher', 'sqlcipher-tcache'] },
  cold_scan: { code: 8, ops: 64, batchable: false, variants: ['sqlcipher', 'sqlcipher-tcache'] },
};

// Every shared modality sets up on one worker first, so only one worker creates and fills the database.
const SHARED = new Set(Object.entries(MODALITIES).filter(([name]) => name.endsWith('_shared')).map(([, code]) => code));
const SERVER_WORKLOADS = ['point', 'range'];

class Remote {
  constructor(handle, index) {
    this.handle = handle;
    this.index = index;
    this.pending = new Map();
    this.next = 0;
    handle.onMessage((message) => {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.ok) pending.resolve(message.result);
      else pending.reject(new Error(`worker ${index} ${pending.cmd}: ${message.error}`));
    });
    // A crashed worker never replies, so its pending calls fail now instead of at their timeout.
    handle.onError?.((error) => {
      for (const [id, pending] of this.pending) {
        clearTimeout(pending.timer);
        pending.reject(new Error(`worker ${index} ${pending.cmd} failed: ${error}`));
        this.pending.delete(id);
      }
    });
  }

  // Every request carries an id the reply quotes, and every wait ends by itself.
  call(cmd, args, timeoutMs, transfer = []) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`worker ${this.index} ${cmd} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, cmd });
      this.handle.post({ id, cmd, args }, transfer);
    });
  }
}

// The slowest point (one worker, encrypted autocommit lookups) takes about 6 s, so a stall shows within minutes.
const SETUP_MS = 600_000;
const RUN_MS = 240_000;

// Hammers one SQLite mutex from many workers, failing on a lost increment or a stuck worker.
export async function runStress(platform, options) {
  const glue = await platform.importGlue('threadsafe');
  const module = await platform.compile('threadsafe');
  await glue.default({ module_or_path: module });
  const memory = glue.shared_memory();
  const workers = [];
  for (let index = 0; index < 32; index++) workers.push(new Remote(platform.spawn(), index));
  await Promise.all(workers.map((worker) => worker.call('init', { glue: platform.glueUrl('threadsafe'), module, memory }, SETUP_MS)));
  let point = 60_000;
  let expected = 0;
  for (let round = 0; round < options.rounds; round++) {
    for (const count of [2, 8, 16, 32]) {
      point++;
      const iterations = 200_000;
      const started = Date.now();
      const results = await Promise.all(workers.slice(0, count).map((worker) =>
        worker.call('stress', [point, count, iterations], 120_000)));
      expected += count * iterations;
      const seen = Math.max(...results);
      await options.log(`${platform.runtime} stress round ${round} ${count} workers: ${Date.now() - started} ms, counter ${seen} of ${expected}`);
      if (seen !== expected) throw new Error(`lost ${expected - seen} increments`);
    }
  }
  for (const worker of workers) platform.terminate(worker.handle);
}

export async function runBench(platform, options) {
  const quick = options.quick ?? false;
  const counts = quick ? [1, 2] : options.counts ?? [1, 2, 4, 8, 16, 32];
  const rounds = quick ? 1 : options.rounds ?? 7;
  const warmups = quick ? 0 : 1;
  const scale = quick ? 1 / 16 : 1;
  const maxWorkers = Math.max(...counts);
  let point = 0;

  for (const variant of options.variants) {
    const glue = await platform.importGlue(variant);
    const module = await platform.compile(variant);
    await glue.default({ module_or_path: module });
    const memory = glue.shared_memory();
    if (options.reserveMb) glue.reserve_heap(options.reserveMb);
    const memstatus = options.memstatus ?? true;
    if (!memstatus) glue.disable_memstatus();
    platform.memstatus = memstatus;
    const sqliteThreadsafe = glue.sqlite_threadsafe();
    const single = sqliteThreadsafe === 0;
    const poolSize = single ? 1 : maxWorkers + 1;
    await options.log(`${platform.runtime} ${variant}: SQLITE_THREADSAFE=${sqliteThreadsafe}, starting ${poolSize} workers`);

    const workers = [];
    for (let index = 0; index < poolSize; index++) workers.push(new Remote(platform.spawn(), index));
    await Promise.all(workers.map((worker) =>
      worker.call('init', { glue: platform.glueUrl(variant), module, memory }, SETUP_MS)));

    const plan = modalityPlan(variant, platform, options);
    for (const { modality, nomutex } of plan) {
      const name = Object.keys(MODALITIES).find((key) => MODALITIES[key] === modality);
      const involved = single ? workers.slice(0, 1) : workers.slice(0, maxWorkers);
      const setup = (worker) => worker.call('setup', { modality, worker: worker.index, nomutex }, SETUP_MS);
      if (SHARED.has(modality)) {
        await setup(involved[0]);
        await Promise.all(involved.slice(1).map(setup));
      } else {
        await Promise.all(involved.map(setup));
      }
      await options.log(`${platform.runtime} ${variant} ${name}${nomutex ? ' nomutex' : ''}: set up`);

      for (const [workload, spec] of Object.entries(WORKLOADS)) {
        if (options.workloads && !options.workloads.includes(workload)) continue;
        if (!(spec.variants ?? ['threadsafe', 'cipher', 'single', 'tcache']).includes(variant)) continue;
        for (const batched of spec.batchable ? [false, true] : [false]) {
          for (const count of single ? [1] : counts) {
            for (let round = 0; round < warmups + (spec.rounds ?? rounds); round++) {
              point++;
              const total = modality === MODALITIES.sahpool_per_worker ? spec.opfsOps ?? spec.ops : spec.ops;
              const ops = Math.max(1, Math.round(total * scale / count));
              const spans = await Promise.all(involved.slice(0, count).map((worker) =>
                worker.call('run', [point, count, modality, nomutex, spec.code, batched, worker.index, ops], options.runMs ?? RUN_MS)))
                .catch(async (error) => {
                  // Two snapshots show which workers still make progress and which are stuck inside an operation.
                  const words = () => Array.from(new Uint32Array(memory.buffer, glue.progress_address(), 192));
                  const before = words();
                  await new Promise((resolve) => setTimeout(resolve, 5_000));
                  const after = words();
                  const state = involved.slice(0, count).map((worker) =>
                    `w${worker.index}:${before[worker.index]}->${after[worker.index]}${after[64 + worker.index] ? ' in run' : ''} stage ${after[128 + worker.index]}`);
                  // A worker that answers a ping after timing out lost its reply, one that does not is stuck.
                  const pings = await Promise.all(involved.slice(0, count).map((worker) =>
                    worker.call('ping', [], 10_000).then(() => `w${worker.index} answers`, () => `w${worker.index} silent`)));
                  throw new Error(`${error.message}; ops ${ops} per worker; ${state.join(', ')}; ${pings.join(', ')}`);
                });
              await options.emit(row(platform, variant, sqliteThreadsafe, name, nomutex, workload, batched, 'own_connection',
                count, round - warmups, spans, ops * count));
            }
          }
          await options.log(`${platform.runtime} ${variant} ${name}${nomutex ? ' nomutex' : ''} ${workload}${batched ? ' batched' : ''}: done`);
        }
      }

      if (variant === 'threadsafe' && modality === MODALITIES.memvfs_shared && !nomutex && !options.workloads) {
        point = await postMessageBaseline(platform, variant, sqliteThreadsafe, workers, counts, rounds, warmups, scale, point, options);
      }
      await Promise.all(involved.map((worker) =>
        worker.call('teardown', { modality, worker: worker.index, nomutex }, SETUP_MS)));
    }
    for (const worker of workers) platform.terminate(worker.handle);
  }
}

function modalityPlan(variant, platform, options) {
  const plan = variant.startsWith('sqlcipher')
    ? [MODALITIES.sqlcipher_shared, MODALITIES.memvfs_shared].map((modality) => ({ modality, nomutex: false }))
    : variant === 'cipher'
    ? [MODALITIES.cipher_shared, MODALITIES.cipher_per_worker].map((modality) => ({ modality, nomutex: false }))
    : [MODALITIES.memvfs_shared, MODALITIES.memvfs_per_worker, MODALITIES.memory_per_worker]
      .concat(platform.opfs ? [MODALITIES.sahpool_per_worker] : [])
      .map((modality) => ({ modality, nomutex: false }))
      .concat(variant === 'threadsafe'
        ? [MODALITIES.memvfs_shared, MODALITIES.memory_per_worker].map((modality) => ({ modality, nomutex: true }))
        : []);
  // Entries like `3` or `0n`, where `n` selects the NOMUTEX connections of that modality.
  const only = options.plan;
  return only ? plan.filter(({ modality, nomutex }) => only.includes(`${modality}${nomutex ? 'n' : ''}`)) : plan;
}

// One worker owns the connection and answers every client over its own MessagePort.
async function postMessageBaseline(platform, variant, sqliteThreadsafe, workers, counts, rounds, warmups, scale, point, options) {
  const server = workers[workers.length - 1];
  await server.call('setup', { modality: MODALITIES.memvfs_shared, worker: server.index, nomutex: false }, SETUP_MS);
  for (const workload of SERVER_WORKLOADS) {
    const spec = WORKLOADS[workload];
    for (const count of counts) {
      for (let round = 0; round < warmups + rounds; round++) {
        point++;
        const ops = Math.max(1, Math.round(spec.ops * scale / count));
        const spans = await Promise.all(workers.slice(0, count).map(async (client) => {
          const channel = platform.channel();
          await server.call('serve', { port: channel.port2 }, SETUP_MS, [channel.port2]);
          return client.call('client', { port: channel.port1, point, parties: count, workload: spec.code, ops },
            RUN_MS, [channel.port1]);
        }));
        await options.emit(row(platform, variant, sqliteThreadsafe, 'memvfs_shared', false, workload, false, 'one_server_worker',
          count, round - warmups, spans, ops * count));
      }
    }
    await options.log(`${platform.runtime} ${variant} postMessage baseline ${workload}: done`);
  }
  await server.call('teardown', { modality: MODALITIES.memvfs_shared, worker: server.index, nomutex: false }, SETUP_MS);
  return point;
}

function row(platform, variant, sqliteThreadsafe, modality, nomutex, workload, batched, topology, workers, round, spans, ops) {
  const start = Math.min(...spans.map((span) => span[0]));
  const end = Math.max(...spans.map((span) => span[1]));
  return {
    runtime: platform.runtime,
    runtime_version: platform.version,
    variant: platform.memstatus === false ? `${variant}-nomemstatus` : variant,
    sqlite_threadsafe: sqliteThreadsafe,
    modality,
    nomutex,
    workload,
    batched,
    topology,
    workers,
    round,
    ms: end - start,
    ops,
  };
}
