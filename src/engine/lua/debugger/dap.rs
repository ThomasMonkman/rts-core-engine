//! The Debug Adapter Protocol side: one client at a time over TCP, its
//! requests answered here or passed to the stopped VM they're about.
//! <https://microsoft.github.io/debug-adapter-protocol/specification>

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

use super::inspect::{Request, split_id};
use super::{Bp, Cmd, DEBUGGER, Debugger, StepKind, Vm, WORK, expr};

pub(super) fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(dbg) = DEBUGGER.get() else { return };
        let _ = stream.set_nodelay(true);
        let Ok(writer) = stream.try_clone() else { continue };
        tracing::info!(peer = ?stream.peer_addr().ok(), "Lua debugger: client connected");
        *dbg.out.lock().unwrap() = Some(writer);
        let mut reader = BufReader::new(stream);
        while let Some(msg) = read_message(&mut reader) {
            if msg["type"] == "request" && !dbg.request(&msg) {
                break;
            }
        }
        dbg.detach();
        *dbg.out.lock().unwrap() = None;
        tracing::info!("Lua debugger: client disconnected");
    }
}

/// One `Content-Length`-framed JSON message; `None` when the stream ends.
fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

impl Debugger {
    fn respond(&self, req: &Value, result: Result<Value, String>) {
        let mut msg = json!({ "type": "response", "request_seq": req["seq"], "command": req["command"] });
        match result {
            Ok(body) => {
                msg["success"] = json!(true);
                msg["body"] = body;
            }
            Err(message) => {
                msg["success"] = json!(false);
                msg["message"] = json!(message);
            }
        }
        self.send(msg);
    }

    /// Handles one request; false when the client is done.
    fn request(&self, req: &Value) -> bool {
        let args = &req["arguments"];
        let command = req["command"].as_str().unwrap_or("");
        let result = match command {
            "initialize" => {
                self.respond(
                    req,
                    Ok(json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsConditionalBreakpoints": true,
                        "supportsHitConditionalBreakpoints": true,
                        "supportsEvaluateForHovers": true,
                        "supportsSetVariable": false,
                        "supportsStepBack": false,
                    })),
                );
                self.event("initialized", json!({}));
                return true;
            }
            "launch" | "attach" => Ok(json!({})),
            "setBreakpoints" => Ok(self.set_breakpoints(args)),
            "setExceptionBreakpoints" | "setFunctionBreakpoints" => Ok(json!({ "breakpoints": [] })),
            "configurationDone" => {
                self.state.lock().unwrap().configured = true;
                self.configured.notify_all();
                Ok(json!({}))
            }
            "threads" => {
                let st = self.state.lock().unwrap();
                Ok(json!({ "threads": st.vms.iter().map(|v| json!({ "id": v.id, "name": v.name })).collect::<Vec<_>>() }))
            }
            "stackTrace" => self.stack_trace(args),
            "scopes" => {
                let (vm, level) = split_id(args["frameId"].as_i64().unwrap_or(0));
                self.ask(vm, Request::Scopes(level as i32))
            }
            "variables" => {
                let (vm, i) = split_id(args["variablesReference"].as_i64().unwrap_or(0));
                self.ask(vm, Request::Variables(i as usize))
            }
            "evaluate" => self.evaluate(args),
            "continue" => self.resume(args, None).map(|_| json!({ "allThreadsContinued": false })),
            "next" => self.resume(args, Some(StepKind::Over)),
            "stepIn" => self.resume(args, Some(StepKind::In)),
            "stepOut" => self.resume(args, Some(StepKind::Out)),
            "pause" => match self.vm(args["threadId"].as_i64().unwrap_or(0)) {
                Some(vm) => {
                    vm.pause.store(true, Ordering::Relaxed);
                    WORK.fetch_add(1, Ordering::Relaxed);
                    Ok(json!({}))
                }
                None => Err("no such VM".into()),
            },
            "source" => self.source(args),
            "disconnect" | "terminate" => {
                self.detach();
                self.respond(req, Ok(json!({})));
                return false;
            }
            other => Err(format!("{other} isn't supported")),
        };
        self.respond(req, result);
        true
    }

    fn set_breakpoints(&self, args: &Value) -> Value {
        let path = args["source"]["path"].as_str().unwrap_or("").to_string();
        // Each breakpoint, or the error that keeps it from being set.
        let parsed: Vec<(u32, Result<Arc<Bp>, String>)> = match args["breakpoints"].as_array() {
            Some(bps) => bps.iter().filter_map(|b| Some((b["line"].as_u64()? as u32, b))).map(|(line, b)| (line, parse_bp(line, b))).collect(),
            None => args["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_u64()).map(|l| (l as u32, parse_bp(l as u32, &Value::Null))).collect(),
        };
        let set: Vec<Arc<Bp>> = parsed.iter().filter_map(|(_, b)| b.as_ref().ok().cloned()).collect();
        let mut st = self.state.lock().unwrap();
        if set.is_empty() {
            st.by_disk.remove(&path);
        } else {
            st.by_disk.insert(path.clone(), set);
        }
        self.resolve(&mut st);
        // Verified once a VM can map the file into its VFS (before any VM
        // attaches, they're kept and mapped then).
        let mapped = st.vms.first().is_none_or(|vm| {
            let vfs = vm.vfs.read().unwrap();
            vfs.contains(&crate::engine::vfs::normalise(&vfs.disk_to_local(&path)))
        });
        let bps: Vec<Value> = parsed
            .iter()
            .map(|(l, b)| match b {
                Err(e) => json!({ "verified": false, "line": l, "message": e }),
                Ok(_) if mapped => json!({ "verified": true, "line": l }),
                Ok(_) => json!({ "verified": false, "line": l, "message": "This file isn't in the game's VFS (not a mounted folder)." }),
            })
            .collect();
        json!({ "breakpoints": bps })
    }

    /// Passes `req` to a stopped VM and waits for its answer.
    fn ask(&self, vm: i64, req: Request) -> Result<Value, String> {
        let vm = self.vm(vm).ok_or("no such VM")?;
        if !vm.stopped.load(Ordering::SeqCst) {
            return Err(format!("{} is running", vm.name));
        }
        let (tx, rx) = mpsc::channel();
        let seq = vm.stop_seq.load(Ordering::SeqCst);
        vm.tx.lock().unwrap().send(Cmd::Inspect { seq, req, reply: tx }).map_err(|_| "VM gone")?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "no answer from the VM".to_string())?
    }

    fn stack_trace(&self, args: &Value) -> Result<Value, String> {
        let id = args["threadId"].as_i64().unwrap_or(0);
        let Some(vm) = self.vm(id) else { return Err("no such VM".into()) };
        if !vm.stopped.load(Ordering::SeqCst) {
            return Ok(json!({ "stackFrames": [], "totalFrames": 0 }));
        }
        let mut all = self.ask(id, Request::StackTrace)?;
        let start = args["startFrame"].as_u64().unwrap_or(0) as usize;
        let levels = args["levels"].as_u64().filter(|&n| n > 0).map_or(usize::MAX, |n| n as usize);
        if let Some(frames) = all["stackFrames"].as_array_mut() {
            let kept: Vec<Value> = frames.drain(..).skip(start).take(levels).collect();
            *frames = kept;
        }
        Ok(all)
    }

    fn evaluate(&self, args: &Value) -> Result<Value, String> {
        let expr = args["expression"].as_str().unwrap_or("").to_string();
        let (vm, level) = match args["frameId"].as_i64() {
            Some(f) => split_id(f),
            None => {
                let st = self.state.lock().unwrap();
                let vm = st.vms.iter().find(|v| v.stopped.load(Ordering::SeqCst)).ok_or("no VM is stopped")?;
                (vm.id, 0)
            }
        };
        self.ask(vm, Request::Evaluate(level as i32, expr))
    }

    fn resume(&self, args: &Value, step: Option<StepKind>) -> Result<Value, String> {
        let vm = self.vm(args["threadId"].as_i64().unwrap_or(0)).ok_or("no such VM")?;
        resume(&vm, step);
        Ok(json!({}))
    }

    fn source(&self, args: &Value) -> Result<Value, String> {
        let r = args["sourceReference"].as_u64().or_else(|| args["source"]["sourceReference"].as_u64()).unwrap_or(0) as usize;
        let st = self.state.lock().unwrap();
        let key = st.sources.get(r.wrapping_sub(1)).ok_or("unknown source")?;
        let vm = st.vms.first().ok_or("no VM")?;
        let bytes = vm.vfs.read().unwrap().read(key).map_err(|e| e.to_string())?;
        Ok(json!({ "content": String::from_utf8_lossy(&bytes), "mimeType": "text/x-lua" }))
    }

    /// The client left: breakpoints off, stopped VMs go on.
    fn detach(&self) {
        let vms = {
            let mut st = self.state.lock().unwrap();
            st.by_disk.clear();
            self.resolve(&mut st);
            st.vms.clone()
        };
        for vm in vms {
            vm.pause.store(false, Ordering::Relaxed);
            super::set_step(&vm, None);
            resume(&vm, None);
        }
    }
}

fn resume(vm: &Vm, step: Option<StepKind>) {
    if vm.stopped.load(Ordering::SeqCst) {
        let seq = vm.stop_seq.load(Ordering::SeqCst);
        let _ = vm.tx.lock().unwrap().send(Cmd::Resume { seq, step });
    }
}

/// A `SourceBreakpoint`: its line, `condition` and `hitCondition`.
fn parse_bp(line: u32, b: &Value) -> Result<Arc<Bp>, String> {
    let condition = match b["condition"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => Some((c.to_string(), expr::parse(c).map_err(|e| format!("Condition: {e}"))?)),
        None => None,
    };
    let hit = match b["hitCondition"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        Some(h) => Some(expr::HitCount::parse(h)?),
        None => None,
    };
    Ok(Arc::new(Bp { line, condition, hit, hits: AtomicU64::new(0) }))
}
