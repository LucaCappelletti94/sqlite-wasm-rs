// Serves the page cross-origin isolated, runs one browser headless and collects its rows:
// `node run-browser.mjs chrome|firefox|webkit [--quick] [--variants=threadsafe,cipher,single]`.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const [browser, ...rest] = process.argv.slice(2);
const args = new Map(rest.map((arg) => arg.replace(/^--/, '').split('=')));
const quick = args.has('quick');
const variants = args.get('variants') ?? 'threadsafe,cipher,single';
const TYPES = { '.html': 'text/html', '.mjs': 'text/javascript', '.js': 'text/javascript', '.wasm': 'application/wasm' };
const ISOLATION = { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp' };
const LIMIT_MS = Number(args.get('limit-ms') ?? 6 * 3600 * 1000);

fs.mkdirSync(`${here}results`, { recursive: true });
const out = `${here}results/${browser}${args.has('variants') ? `-${variants.replaceAll(',', '-')}` : ''}${args.has('tag') ? `-${args.get('tag')}` : ''}${quick ? '-quick' : ''}.jsonl`;
fs.writeFileSync(out, '');

let finish;
const finished = new Promise((resolve) => { finish = resolve; });
const server = http.createServer((request, response) => {
  if (request.method === 'POST') {
    let body = '';
    request.on('data', (chunk) => { body += chunk; });
    request.on('end', () => {
      response.writeHead(200, ISOLATION).end();
      const data = JSON.parse(body);
      // Load from other processes is stamped per point, since it varies within a run.
      if (request.url === '/row') fs.appendFileSync(out, JSON.stringify({ ...data, loadavg1: os.loadavg()[0] }) + '\n');
      else if (request.url === '/environment') fs.appendFileSync(out, JSON.stringify({ environment: { ...data, cpu: os.cpus()[0].model, loadavg: os.loadavg(), date: new Date().toISOString() } }) + '\n');
      else if (request.url === '/log') console.log(data.line);
      else if (request.url === '/done') finish(0);
      else if (request.url === '/fail') { console.error(data.error); finish(1); }
    });
    return;
  }
  const file = path.join(here, decodeURIComponent(new URL(request.url, 'http://x').pathname));
  if (!file.startsWith(here) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    response.writeHead(404, ISOLATION).end();
    return;
  }
  response.writeHead(200, { ...ISOLATION, 'Content-Type': TYPES[path.extname(file)] ?? 'application/octet-stream' });
  fs.createReadStream(file).pipe(response);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/index.html?runtime=${browser}&variants=${variants}`
  + (quick ? '&quick' : '') + (args.has('plan') ? `&plan=${args.get('plan')}` : '')
  + (args.has('workloads') ? `&workloads=${args.get('workloads')}` : '')
  + (args.has('stress') ? `&stress=${args.get('stress')}` : '')
  + ['counts', 'rounds', 'run-ms', 'reserve-mb'].filter((key) => args.has(key)).map((key) => `&${key}=${args.get(key)}`).join('');

// Snap-packaged Firefox can only read profiles under its own home directory.
const profileRoot = fs.existsSync(`${os.homedir()}/snap/firefox/common`) ? `${os.homedir()}/snap/firefox/common` : os.tmpdir();
const profile = fs.mkdtempSync(path.join(browser === 'firefox' ? profileRoot : os.tmpdir(), 'threadsafe-bench-'));
const webkit = () => {
  const root = `${os.homedir()}/.cache/ms-playwright`;
  const build = fs.readdirSync(root).filter((name) => name.startsWith('webkit-')).sort().pop();
  return `${root}/${build}/pw_run.sh`;
};
const commands = {
  chrome: ['google-chrome', ['--headless=new', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check',
    '--disable-background-timer-throttling', '--disable-renderer-backgrounding', url]],
  firefox: ['firefox', ['--headless', '--no-remote', '--profile', profile, url]],
  webkit: [process.env.WEBKIT_RUN ?? webkit(), ['--headless', url]],
};
const [command, commandArgs] = commands[browser];
const child = spawn(command, commandArgs, { stdio: 'ignore', detached: true });
const timer = setTimeout(() => { console.error(`no result within ${LIMIT_MS} ms`); finish(2); }, LIMIT_MS);

const code = await finished;
clearTimeout(timer);
try { process.kill(-child.pid, 'SIGTERM'); } catch {}
server.close();
// The browser may still be writing its profile while it exits.
try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 }); } catch {}
console.log(`results in ${out}`);
process.exit(code);
