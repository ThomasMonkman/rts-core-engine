// The engine is the debug adapter (`--lua-debug` serves the Debug Adapter
// Protocol over TCP): this extension only declares the `faf-lua` debug type
// and points VS Code at the engine's port. `launch` also starts the engine
// (by default `cargo run -- --session --lua-debug-wait --no-audio`) and waits for the
// port to open; ending the session stops it. See docs/guides/debugging-lua.md.

const vscode = require('vscode');
const net = require('net');
const cp = require('child_process');

function probe(host, port) {
  return new Promise((resolve) => {
    const s = net.connect({ host, port }, () => { s.destroy(); resolve(true); });
    s.on('error', () => resolve(false));
  });
}

async function waitForPort(host, port, timeout, child) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (child && child.exitCode !== null) throw new Error(`the engine exited (code ${child.exitCode}) before it listened on ${port}`);
    if (await probe(host, port)) return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`nothing listening on ${host}:${port} after ${timeout / 1000} s`);
}

function activate(context) {
  const out = vscode.window.createOutputChannel('FAF Lua engine');
  const children = new Map();

  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('faf-lua', {
      async createDebugAdapterDescriptor(session) {
        const c = session.configuration;
        const port = c.port || 4711;
        const host = c.host || '127.0.0.1';
        if (c.request === 'launch') {
          const program = c.program || 'cargo';
          const args = [...(c.args || ['run', '--', '--session', '--lua-debug-wait', '--no-audio'])];
          if (!args.some((a) => a.startsWith('--lua-debug=') || a === '--lua-debug')) {
            if (program === 'cargo' && !args.includes('--')) args.push('--');
            args.push(`--lua-debug=${port}`);
          }
          const cwd = c.cwd || (session.workspaceFolder && session.workspaceFolder.uri.fsPath);
          out.show(true);
          out.appendLine(`> ${program} ${args.join(' ')}`);
          // Its own process group, so stopping it stops cargo's child too.
          const child = cp.spawn(program, args, { cwd, env: process.env, detached: process.platform !== 'win32' });
          child.stdout.on('data', (d) => out.append(d.toString()));
          child.stderr.on('data', (d) => out.append(d.toString()));
          child.on('exit', (code) => out.appendLine(`engine exited (${code})`));
          children.set(session.id, child);
          await waitForPort(host, port, c.timeout || 600000, child);
        }
        return new vscode.DebugAdapterServer(port, host);
      },
    }),
    vscode.debug.onDidTerminateDebugSession((s) => {
      const child = children.get(s.id);
      if (!child) return;
      children.delete(s.id);
      if (child.exitCode !== null) return;
      try {
        // Windows has no process groups: killing cargo would leave the engine
        // running, so end the whole tree.
        if (process.platform === 'win32') cp.spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true });
        else process.kill(-child.pid, 'SIGTERM');
      } catch (_) { /* already gone */ }
    }),
    out,
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
