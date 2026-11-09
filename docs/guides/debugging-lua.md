# Debugging the game's Lua

The engine has a built-in Lua debugger for the sim VM and the interface VM. It supports breakpoints
(with conditions and hit counts), step over / in / out, pause, the call stack, locals, upvalues, module globals, expandable tables,
hover / watch expressions, and a Debug Console that runs Lua in the stopped function. It speaks the
[Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/) over TCP, so VS Code
talks to the engine directly. A small extension in `dev/vscode-lua-debug/` declares the debug
type. The code is in `src/engine/lua/debugger/`.

## Setup (once)

Build the extension and install it, then reload VS Code (**Developer: Reload Window**):

```sh
dev/vscode-lua-debug/package.sh                                                # macOS / Linux
powershell -ExecutionPolicy Bypass -File dev\vscode-lua-debug\package.ps1   # Windows
```

The script zips the extension into `target/faf-lua-debug.vsix` without needing `vsce` or a
download, then runs `code --install-extension`. If the `code` command isn't found, install the file
with **Extensions → ⋯ → Install from VSIX…**. Run the script again after changing the extension.
Linking or copying the folder into `~/.vscode/extensions` doesn't work: VS Code only loads
extensions listed in its `extensions.json` and marks any others obsolete.

The repo's `.vscode/launch.json` has the configurations. To set breakpoints in FAF's code, open
`../fa` in the same window: **File → Add Folder to Workspace…**, or just open the file.

## Use

- **F5 with "Engine: skirmish (Lua debug)"** builds and runs `cargo run -- --session
  --lua-debug-wait --no-audio`. The engine's output goes to the *FAF Lua engine* output panel. The engine
  runs no Lua until VS Code has attached and sent its breakpoints, so breakpoints in start-up code
  (blueprint loading, `SetupSession`, `BeginSession`, unit `OnCreate`) are hit. Stopping the
  debug session stops the engine.
- **"Engine: solo, no AI (Lua debug)"** is the same with one army, yours (`--armies 1 --player 1`):
  your commander and nothing else, so only what you do runs.
- **"Attach to the engine's Lua"** connects to an engine you started yourself with
  `--lua-debug` (port 4711, or `--lua-debug <port>`). Detaching clears the breakpoints and the
  game carries on.

Each VM appears as a thread: **Sim** and **UI**. Each stops on its own: stopping the sim
freezes the game while the interface keeps drawing. Stopping the UI freezes the window.

### Conditional breakpoints

Right-click a breakpoint → **Edit Condition** (or right-click the gutter → **Add Conditional
Breakpoint…**):

- **Expression**: stop only when it's true, e.g. `self.UnitId == "uel0001"`,
  `amount > 500 and not self.Dead`, `i % 100 == 0`.
- **Hit Count**: stop on the 5th hit (`5` or `== 5`), from the 5th (`>= 5`), every 5th (`% 5`),
  or `>`, `<`, `<=`. With a condition too, only hits where it holds count.

A condition that fails to evaluate (indexing `nil`, comparing a number with a string) stops there,
with the error in the Debug Console, so a broken one isn't silently never hit. One that doesn't
parse (a function call, say) isn't set: VS Code shows the breakpoint grey with the reason.

Each time the line runs the condition is checked, which costs microseconds: fine for a callback
like `OnDamage`, noticeable on a line in a tight loop. Breakpoints without a condition cost nothing
until hit.

### The Debug Console

Type Lua while stopped and it runs in the selected stack frame, as a browser's console does:

```lua
self:GetHealth()              -- 12000
self:SetHealth(self, 500)     -- the unit now has 500 HP
count = count + 1             -- a local of the stopped function: it carries on with the new value
LOG(self.UnitId)              -- print / LOG / WARN / SPEW show in the console too
```

Names are the frame's locals, then upvalues, then the module's globals, and assigning one
changes it. In the **Variables** view, double-click a value to set it. Plain reads (names,
fields, comparisons) run no code, so looking changes nothing.

- **It changes the game**: a sim changed this way won't replay the same (the log says so the
  first time), and in a network game it desyncs you.
- Breakpoints don't stop inside console code, it can't wait (`WaitSeconds`), and it's stopped
  after 5 seconds.
- Luau folds a local set to a constant and never reassigned (`local n = 10`) into the code, so
  assigning it has no effect. Parameters and locals set from calls are fine.

### Expressions

Conditions, the watch list and hovers share one read-only subset of Lua:

- names and fields, `self.Blueprint.General`, `units[1]["EntityId"]`, `t[i]` (`a:b` reads the
  field `b`);
- literals (numbers, strings, `nil`, `true`, `false`);
- `== ~= < <= > >=`, `and or not`, `+ - * / %`, unary `-`, and `#`, with Lua's precedence.

Names are looked up as locals, then upvalues, then the function's module environment. `__index`
tables are followed (so methods and class fields show), but nothing is called: no function calls
(`self:GetHealth()` won't work, `self.MaxHealth` or the like will), no metamethods (`==` compares
tables by identity, `#` is the raw length), no `..`. For more, use the Debug Console. The code is
`src/engine/lua/debugger/expr.rs`.

## Limits

- **Pause, then step**: a pause stops wherever Lua is (at a call or a loop back-edge). A step from
  there starts at the next Lua call into that VM, not the next line. Stepping from a breakpoint
  is exact.
- Only chunks loaded through `doscript` (all VFS scripts: FAF, maps, mods) get breakpoints. Engine
  chunks (`=[engine ...]`) and chunks restored from a keyframe or save game don't.
- No logpoints or "break on error" yet.
- The init VM (`init_*.lua`) and the other short-lived VMs (prefs, mod scanning) aren't attached.
