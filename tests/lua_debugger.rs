//! The Lua debugger end to end (`engine::lua::debugger`): a DAP client over
//! TCP sets breakpoints in a script and in its hook, and the VM stops there,
//! shows its stack and locals, evaluates, steps over and out, stops at
//! conditional breakpoints only where they hold, and carries on
//! to the right result. Its own test binary: `--lua-debug` changes how every
//! script in the process compiles.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, RwLock};

use mlua::Lua;
use rts_core_engine::engine::api::dofile_or_script;
use rts_core_engine::engine::lua::{debugger, script_cache::{self, ScriptCache}};
use rts_core_engine::engine::vfs::{MountPoint, Vfs};
use serde_json::{Value, json};

struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
    seq: i64,
    events: Vec<Value>,
}

impl Client {
    fn connect(port: u16) -> Self {
        let w = TcpStream::connect(("127.0.0.1", port)).unwrap();
        w.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
        let r = BufReader::new(w.try_clone().unwrap());
        Self { w, r, seq: 1, events: Vec::new() }
    }

    fn read(&mut self) -> Value {
        let mut len = 0;
        loop {
            let mut line = String::new();
            assert!(self.r.read_line(&mut line).unwrap() > 0, "debugger closed the connection");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        self.r.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn request(&mut self, command: &str, arguments: Value) -> Value {
        let seq = self.seq;
        self.seq += 1;
        let body = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }).to_string();
        self.w.write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        loop {
            let msg = self.read();
            if msg["type"] == "response" && msg["request_seq"] == seq {
                assert_eq!(msg["success"], true, "{command} failed: {msg}");
                return msg["body"].clone();
            }
            self.events.push(msg);
        }
    }

    fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.events.iter().position(|e| e["event"] == name) {
            return self.events.remove(i)["body"].clone();
        }
        loop {
            let msg = self.read();
            if msg["event"] == name {
                return msg["body"].clone();
            }
            self.events.push(msg);
        }
    }

    /// The top frame of a stopped thread: (function name, file, line, frame id).
    fn top(&mut self, thread: i64) -> (String, String, i64, i64) {
        let st = self.request("stackTrace", json!({ "threadId": thread }));
        let f = &st["stackFrames"][0];
        (f["name"].as_str().unwrap().into(), f["source"]["path"].as_str().unwrap_or("").into(), f["line"].as_i64().unwrap(), f["id"].as_i64().unwrap())
    }

    fn locals(&mut self, frame: i64) -> Vec<(String, String)> {
        let scopes = self.request("scopes", json!({ "frameId": frame }));
        let r = scopes["scopes"][0]["variablesReference"].clone();
        let vars = self.request("variables", json!({ "variablesReference": r }));
        vars["variables"].as_array().unwrap().iter().map(|v| (v["name"].as_str().unwrap().into(), v["value"].as_str().unwrap().into())).collect()
    }
}

const SCRIPT: &str = "local function add(a, b)
    local sum = a + b
    return sum
end
local t = { name = \"x\", n = 3 }
local r = add(1, 2)
r = r + t.n
Result = r
for i = 1, 5 do
    local sq = i * i
    sq = sq + 0
end
";

const HOOK: &str = "local h = 10
Result = Result + h
";

#[test]
fn breakpoints_stack_variables_and_stepping() {
    let dir = std::env::temp_dir().join(format!("lua_debugger_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lua")).unwrap();
    std::fs::create_dir_all(dir.join("schook/lua")).unwrap();
    std::fs::write(dir.join("lua/dbgtest.lua"), SCRIPT).unwrap();
    std::fs::write(dir.join("schook/lua/dbgtest.lua"), HOOK).unwrap();
    let script = std::fs::canonicalize(dir.join("lua/dbgtest.lua")).unwrap();
    let hook = std::fs::canonicalize(dir.join("schook/lua/dbgtest.lua")).unwrap();

    debugger::start(0, false).unwrap();
    let mut c = Client::connect(debugger::port().unwrap());
    c.request("initialize", json!({ "adapterID": "test" }));
    c.event("initialized");
    c.request("attach", json!({}));
    let set = c.request(
        "setBreakpoints",
        json!({ "source": { "path": script }, "breakpoints": [
            { "line": 2 },
            // The 2nd time i is even (i = 4).
            { "line": 10, "condition": "i % 2 == 0 and t.name == 'x'", "hitCondition": "2" },
            { "line": 11, "condition": "sq > 20" },
            // Calls can't be evaluated: not set.
            { "line": 7, "condition": "f()" },
        ] }),
    );
    let verified: Vec<bool> = set["breakpoints"].as_array().unwrap().iter().map(|b| b["verified"].as_bool().unwrap()).collect();
    assert_eq!(verified, [true, true, true, false], "{set}");
    c.request("setBreakpoints", json!({ "source": { "path": hook }, "breakpoints": [{ "line": 2 }] }));
    c.request("configurationDone", json!({}));

    let vm_dir = dir.clone();
    let vm = std::thread::spawn(move || {
        let vfs = Vfs::new(vec![MountPoint { source: vm_dir, mountpoint: "/".into() }], vec!["/schook".into()]).unwrap();
        let vfs = Arc::new(RwLock::new(vfs));
        let lua = Lua::new();
        lua.set_compiler(script_cache::compiler());
        lua.set_app_data(vfs.clone());
        lua.set_app_data(Arc::new(ScriptCache::new(vfs)));
        dofile_or_script::register(&lua).unwrap();
        debugger::attach(&lua, "Test");
        lua.load("doscript('/lua/dbgtest.lua') return Result").eval::<i64>().unwrap()
    });

    let started = c.event("thread");
    let thread = started["threadId"].as_i64().unwrap();

    // The breakpoint in `add`.
    let stop = c.event("stopped");
    assert_eq!(stop["reason"], "breakpoint");
    let (name, file, line, frame) = c.top(thread);
    assert_eq!((name.as_str(), line), ("add", 2));
    assert_eq!(std::path::Path::new(&file), script);
    let locals = c.locals(frame);
    assert!(locals.contains(&("a".into(), "1".into())) && locals.contains(&("b".into(), "2".into())), "{locals:?}");
    let ev = c.request("evaluate", json!({ "expression": "b", "frameId": frame, "context": "hover" }));
    assert_eq!(ev["result"], "2");
    // Raw field lookup through the caller's locals.
    let caller = c.request("stackTrace", json!({ "threadId": thread }))["stackFrames"][1]["id"].as_i64().unwrap();
    assert_eq!(c.request("evaluate", json!({ "expression": "t.name", "frameId": caller }))["result"], "\"x\"");

    // Step over: the next line of `add`.
    c.request("next", json!({ "threadId": thread }));
    assert_eq!(c.event("stopped")["reason"], "step");
    let (_, _, line, frame) = c.top(thread);
    assert_eq!(line, 3);
    assert!(c.locals(frame).contains(&("sum".into(), "3".into())));

    // Step out: back in the main chunk, on the line after the call (the
    // call's result went straight into `r`: line 6 has nothing left to run).
    c.request("stepOut", json!({ "threadId": thread }));
    assert_eq!(c.event("stopped")["reason"], "step");
    let (name, _, line, _) = c.top(thread);
    assert_eq!((name.as_str(), line), ("(main chunk)", 7));

    // Continue: the conditional breakpoints, each where its condition holds.
    c.request("continue", json!({ "threadId": thread }));
    assert_eq!(c.event("stopped")["reason"], "breakpoint");
    let (_, _, line, frame) = c.top(thread);
    assert_eq!(line, 10);
    assert_eq!(c.request("evaluate", json!({ "expression": "i", "frameId": frame }))["result"], "4");
    assert_eq!(c.request("evaluate", json!({ "expression": "i * 2 + t.n - #t", "frameId": frame }))["result"], "11");
    assert_eq!(c.request("evaluate", json!({ "expression": "t.name == \"x\" and not missing", "frameId": frame }))["result"], "true");
    c.request("continue", json!({ "threadId": thread }));
    assert_eq!(c.event("stopped")["reason"], "breakpoint");
    let (_, _, line, frame) = c.top(thread);
    assert_eq!(line, 11);
    assert_eq!(c.request("evaluate", json!({ "expression": "sq", "frameId": frame }))["result"], "25");

    // Continue: the breakpoint in the hook, shown in the hook's own file.
    c.request("continue", json!({ "threadId": thread }));
    assert_eq!(c.event("stopped")["reason"], "breakpoint");
    let (_, file, line, frame) = c.top(thread);
    assert_eq!(std::path::Path::new(&file), hook);
    assert_eq!(line, 2);
    assert!(c.locals(frame).contains(&("h".into(), "10".into())));

    c.request("continue", json!({ "threadId": thread }));
    assert_eq!(vm.join().unwrap(), 16);
    c.request("disconnect", json!({}));
    std::fs::remove_dir_all(dir).ok();
}
