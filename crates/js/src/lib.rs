//! Code-mode tools: the model writes one JavaScript program, we run it in a
//! fresh QuickJS VM where every tool is an async function.
//!
//! Why code instead of one tool call per action (the idea Tura's `command_run`
//! proved: fewer round trips, fewer replayed tokens), but stronger: `await` is a
//! dependency step, `Promise.all` is a parallel step, variables bind outputs,
//! a thrown error stops the batch, and the script returns only what the model
//! needs to see instead of every raw tool output.
//!
//! The VM has no filesystem, network or process access of its own; the tool
//! functions below are the only way out. Each finished call is reported as a
//! [`Call`] for the UI's tool log.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::prelude::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Function, Promise};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

/// Tool functions the script can call, for the model-facing description.
pub const TOOLS: &str = "\
read(path, {offset?, limit?}) -> string      the whole file; or lines offset..offset+limit (1-based) ending in a
                                             note …[lines a-b of n]; long files come in 2000-line pages
                                             an image file: you see it (attached after this program's result)
                                             and get {type:'image', path, width, height, mime, bytes}
write(path, text, {force?})                  create or overwrite (atomic), makes parent dirs; refuses text holding
                                             a part or cut note, and emptying a file without {force: true}
edit(path, old, new, {all?})                 exact text replace; throws if old is missing or ambiguous
ls(path='.') -> string[]                     sorted names, directories end with /
sh(cmd, {cwd?, timeout?, input?}) -> {code, out, err}  bash -c; never throws on exit code; input: text for its
                                             stdin (else it has none);
                                             out/err lose trailing whitespace, and past 10000 chars their middle
                                             is cut (marked): not a byte-exact copy to write back
console.log(...)                             add a line to the output
Every call starts immediately and runs concurrently: await only where you need the value.
Calls you never await still finish before the program ends; their failures are reported.
Each tool also takes one object, e.g. edit({path, old, new}) or sh({cmd, timeout}).
Every call takes {timeout: ms} in its last options object (default 5000). A call still running then throws (catch it
to go on) and keeps running in the background as a job; the error names its id. Give slow work a timeout up front:
sh('cargo build', {timeout: 300000}).
jobs.wait(id | [ids], {timeout?}) -> {id, call, status, secs, value | error | out, err}  until it ends; timeout ms
                                             (default 300000); status running|done|failed|stopped; while it runs,
                                             an sh job shows its output so far, and waiting: its last line when it
                                             seems stuck at a prompt (wait returns then). Jobs outlive the program
                                             and turn
sh(cmd, {interactive: true}) -> {id, status, out, err}  a session (ssh, a REPL, a prompt): a job from the start
                                             with stdin kept open; answers once its output goes quiet. No terminal:
                                             use ssh -T, python3 -iu, or script -qfc 'cmd' /dev/null for programs that
                                             need one
jobs.send(id, text, {wait?, close?}) -> {id, status, out, err, code?}  write text to its stdin (end lines with \\n),
                                             then the output since the last send/read, once it goes quiet (wait ms,
                                             default 10000); close: true ends its input (EOF)
jobs.read(id, {wait?}) -> same               the output since the last send/read, without sending
jobs.list() -> [{id, call, status, secs}]    this conversation's jobs
jobs.stop(id)                                stop it (an sh job: TERM to its whole process group, KILL after 3s)
A job you are not waiting on messages you when it ends, or when it seems stuck at a prompt (<job-notification>,
between your steps or as a new turn). So never poll or sleep for a job: do other work or end your turn, and
jobs.wait only when you need its result to go on. Don't start a second copy of a job that is still running.";

/// Model-visible output is cut to this many characters (head + tail kept).
pub const MAX_OUTPUT: usize = 10_000;

/// One tool call, for the log: sent once when it starts (`running`) and
/// again with the same `id` when it finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Order of the call within its program.
    pub id: usize,
    pub running: bool,
    /// read, write, edit, ls, sh
    pub verb: String,
    /// Path or command, single line.
    pub target: String,
    /// Short result: "412 lines", "+3 −1", "✓ 1.2s", or the error.
    pub info: String,
    pub ok: bool,
}

/// What the model gets back, plus totals for the log.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Console lines + return value (or the error), truncated to MAX_OUTPUT.
    pub text: String,
    pub error: Option<String>,
    pub calls: usize,
    pub elapsed: Duration,
    /// Image files the program read, absolute, in order, without repeats:
    /// for the embedder to show the model (and the user).
    pub images: Vec<PathBuf>,
}

/// Images one program may show; the rest are named in its output instead.
pub const MAX_SHOWN: usize = 10;

pub struct Limits {
    pub timeout: Duration,
    pub memory: usize,
    /// `write` and `edit` refuse (`sh` still runs: this guards mistakes,
    /// it is not a sandbox).
    pub read_only: bool,
    /// `write` and `edit` refuse paths outside the root folder (an agent
    /// in its own worktree). Same caveat: `sh` is not confined.
    pub confine: bool,
    /// Where calls that outlive their timeout go on (keep one per
    /// conversation so later programs can reach them).
    pub jobs: Arc<Jobs>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(600),
            memory: 256 << 20,
            read_only: false,
            confine: false,
            jobs: Arc::default(),
        }
    }
}

/// What a host function resolves to: the value for the script and a short
/// log line, or an error message (thrown in the script).
pub type HostResult = Result<(Value, String), String>;
pub type HostFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = HostResult> + Send + 'a>>;

/// Extra async functions provided by the embedder (subagents, ...). Each one
/// is a global in the script and logs like the built-in tools.
pub trait Host: Send + Sync {
    /// Global names this host answers.
    fn names(&self) -> &[&'static str];
    /// How long a call may run before it becomes a background job; None:
    /// the caller's `{timeout}` or the default (5 s). For calls that wait
    /// by design, like `agents.wait`.
    fn timeout(&self, _name: &str, _args: &[Value]) -> Option<Duration> {
        None
    }
    /// Log target for a call (first argument by default).
    fn target(&self, _name: &str, args: &[Value]) -> String {
        args.first()
            .map(|arg| match arg {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    }
    fn call<'a>(&'a self, name: &'a str, args: Vec<Value>) -> HostFuture<'a>;
}

struct Env {
    cwd: PathBuf,
    log: UnboundedSender<Call>,
    read_only: bool,
    confine: bool,
    host: Option<Arc<dyn Host>>,
    jobs: Arc<Jobs>,
    /// Finished calls, for the "what already happened" footer after an error.
    done: std::sync::Mutex<Vec<Call>>,
    /// Images to show the model, with the number of the call that read
    /// them (calls finish in any order; they are shown in call order).
    shown: std::sync::Mutex<Vec<(usize, PathBuf)>>,
}

impl Env {
    /// Mark the image at `file` (relative to cwd), read by call `id`, for
    /// the model.
    fn show(&self, id: usize, file: &str) {
        let Ok(target) = path(self, file) else {
            return;
        };
        if let Ok(mut shown) = self.shown.lock() {
            shown.push((id, target));
        }
    }
}

/// A leading `//` comment, if the script opens with one. Never the first
/// line of code: `const [a, b] = await …` says nothing about intent.
pub fn title(code: &str) -> Option<String> {
    let first = code.lines().map(str::trim).find(|line| !line.is_empty())?;
    let text = first.strip_prefix("//")?.trim();
    (!text.is_empty()).then(|| text.chars().take(80).collect())
}

/// Run `code` as the body of an async function in a fresh VM rooted at `cwd`.
pub async fn run(code: &str, cwd: &Path, limits: Limits, log: UnboundedSender<Call>) -> Outcome {
    run_with(code, cwd, limits, log, None).await
}

/// [`run`] with the host's functions available as globals.
pub async fn run_with(
    code: &str,
    cwd: &Path,
    limits: Limits,
    log: UnboundedSender<Call>,
    host: Option<Arc<dyn Host>>,
) -> Outcome {
    let start = Instant::now();
    let env = Arc::new(Env {
        cwd: cwd.to_path_buf(),
        log,
        read_only: limits.read_only,
        confine: limits.confine,
        host,
        jobs: limits.jobs.clone(),
        done: std::sync::Mutex::new(Vec::new()),
        shown: std::sync::Mutex::new(Vec::new()),
    });
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let secs = limits.timeout.as_secs();
    let result = tokio::time::timeout(
        limits.timeout,
        execute(code, env.clone(), calls.clone(), &limits),
    )
    .await;
    let (out, error) = match result {
        Ok(Ok((out, value))) => (join(&out, value.as_deref()), None),
        Ok(Err((out, err))) => {
            let err = if err.contains("interrupted") {
                format!("stopped after {secs}s: the program never finished (endless loop?)")
            } else {
                err
            };
            (join(&out, Some(&format!("error{err}"))), Some(err))
        }
        Err(_) => {
            let err = format!(": the program ran longer than {secs}s and was stopped");
            (format!("error{err}"), Some(err))
        }
    };
    // After a failure, list what already ran: those effects are real.
    let out = match &error {
        Some(_) => {
            let done = env.done.lock().map(|done| done.clone()).unwrap_or_default();
            if done.is_empty() {
                format!("{out}\nnothing ran before the error")
            } else {
                let mut footer =
                    format!("{out}\ncalls that ran before the error (their effects are kept):");
                for call in done.iter().take(12) {
                    let mark = if call.ok { "✓" } else { "✗" };
                    footer.push_str(&format!(
                        "\n  {mark} {} {} · {}",
                        call.verb, call.target, call.info
                    ));
                }
                if done.len() > 12 {
                    footer.push_str(&format!("\n  … {} more", done.len() - 12));
                }
                footer
            }
        }
        None => out,
    };
    let error = error.map(|err| err.trim_start_matches([':', ' ']).to_string());
    let calls = calls.load(std::sync::atomic::Ordering::Relaxed);
    let mut images: Vec<PathBuf> = Vec::new();
    let mut shown = env
        .shown
        .lock()
        .map(|shown| shown.clone())
        .unwrap_or_default();
    shown.sort_by_key(|(id, _)| *id);
    for (_, image) in shown {
        if !images.contains(&image) {
            images.push(image);
        }
    }
    let mut out = out;
    if images.len() > MAX_SHOWN {
        let rest: Vec<String> = images[MAX_SHOWN..]
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        out.push_str(&format!(
            "\n(only {MAX_SHOWN} images are shown per program; not shown: {})",
            rest.join(", ")
        ));
        images.truncate(MAX_SHOWN);
    }
    let text = if out.trim().is_empty() && images.is_empty() {
        format!("ok · {calls} calls · no output (return or console.log what you need to see)")
    } else {
        clip(&out, MAX_OUTPUT)
    };
    Outcome {
        text,
        error,
        calls,
        elapsed: start.elapsed(),
        images,
    }
}

fn join(out: &str, value: Option<&str>) -> String {
    match value {
        Some(value) if out.is_empty() => value.to_string(),
        Some(value) => format!("{out}\n{value}"),
        None => out.to_string(),
    }
}

/// Glue: one host function `__call(name, argsJson) -> resultJson`; the
/// prelude turns it into throwing async functions and captures console.
const PRELUDE: &str = r#"
const __out = [];
const __pending = [];
// Images are plain objects from read(); they print as one short line.
const __isImage = (x) => !!x && typeof x === 'object' && x.type === 'image' && typeof x.path === 'string';
const __imageText = (x) => '[image ' + x.path + ' ' + x.width + '×' + x.height + ']';
const __fmt = (x) => {
  if (typeof x === 'string') return x;
  if (x && typeof x.then === 'function') return '<' + (x.__label || 'call') + ' not awaited>';
  if (__isImage(x)) return __imageText(x);
  try { return JSON.stringify(x, (k, v) => (__isImage(v) ? __imageText(v) : v)); } catch (e) { return String(x); }
};

globalThis.console = { log: (...a) => { __out.push(a.map(__fmt).join(' ')); } };
console.error = console.warn = console.info = console.debug = console.log;
// One object argument works too: read({path}), edit({path, old, new}), sh({cmd}).
const __norm = (name, a) => {
  if (a.length === 1 && a[0] && typeof a[0] === 'object' && !Array.isArray(a[0])) {
    const o = a[0];
    const pick = (...ks) => { for (const k of ks) if (o[k] !== undefined) return o[k]; };
    if (name === 'read') a = [pick('path', 'file', 'file_path'), { offset: pick('offset', 'start', 'from'), limit: pick('limit', 'count', 'lines'), timeout: pick('timeout', 'timeout_ms') }];
    if (name === 'write') a = [pick('path', 'file', 'file_path'), pick('content', 'text', 'data', 'contents'), { force: pick('force'), timeout: pick('timeout', 'timeout_ms') }];
    if (name === 'edit') a = [pick('path', 'file', 'file_path'), pick('old', 'old_text', 'old_string', 'search', 'find'), pick('new', 'new_text', 'new_string', 'replace', 'replacement'), { all: pick('all', 'replace_all'), timeout: pick('timeout', 'timeout_ms') }];
    if (name === 'ls') a = [pick('path', 'dir', 'directory'), { timeout: pick('timeout', 'timeout_ms') }];
    if (name === 'sh') a = [pick('cmd', 'command', 'script'), { cwd: pick('cwd', 'dir', 'workdir'), timeout: pick('timeout', 'timeout_ms'), input: pick('input', 'stdin'), interactive: pick('interactive') }];
  }
  if (name === 'sh' && Array.isArray(a[0])) {
    a = [a[0].map(x => /^[\w@%+=:,./-]+$/.test(String(x)) ? String(x) : "'" + String(x).replace(/'/g, "'\\''") + "'").join(' '), ...a.slice(1)];
  }
  if (name === 'read' && typeof a[1] === 'number') a = [a[0], { offset: a[1], limit: a[2] }];
  // Objects are written as JSON; a missing text stays missing (an error),
  // never an empty file.
  if (name === 'write' && a[1] !== undefined && a[1] !== null && typeof a[1] !== 'string') a = [a[0], JSON.stringify(a[1], null, 2), ...a.slice(2)];
  return a;
};
// Calls start at once. The returned thenable records whether anyone awaited
// it, so failures nobody looked at are still reported.
const __tool = (name) => (...args) => {
  const norm = __norm(name, args);
  const label = name + '(' + JSON.stringify(norm[0] === undefined ? '' : norm[0]).slice(0, 60) + ')';
  const entry = { label, used: false, error: null };
  entry.inner = __call(name, JSON.stringify(norm)).then((s) => {
    const r = JSON.parse(s);
    if ('error' in r) throw new Error(label + ': ' + r.error);
    return r.value;
  });
  entry.inner.catch((e) => { entry.error = e; });
  __pending.push(entry);
  return {
    __label: label,
    then(ok, ko) { entry.used = true; return entry.inner.then(ok, ko); },
    catch(ko) { entry.used = true; return entry.inner.catch(ko); },
    finally(f) { entry.used = true; return entry.inner.finally(f); },
  };
};
for (const name of ['read', 'write', 'edit', 'ls', 'sh']) globalThis[name] = __tool(name);
globalThis.jobs = { wait: __tool('jobs.wait'), send: __tool('jobs.send'), read: __tool('jobs.read'), list: __tool('jobs.list'), stop: __tool('jobs.stop') };
// A returned value may hold calls that were never awaited: resolve them.
const __settle = async (v, d = 0) => {
  if (v && typeof v.then === 'function') v = await v;
  if (d > 2 || !v || typeof v !== 'object') return v;
  if (Array.isArray(v)) return Promise.all(v.map((x) => __settle(x, d + 1)));
  if (Object.getPrototypeOf(v) === Object.prototype) {
    const o = {};
    for (const k of Object.keys(v)) o[k] = await __settle(v[k], d + 1);
    return o;
  }
  return v;
};
"#;

/// Wraps the model's code; line numbers in errors are shifted back by the
/// lines before the code.
const WRAP_HEAD: &str = "
(async () => {
  let v, err;
  try { v = await __settle(await (async () => {
";
const WRAP_TAIL: &str = r#"
})()); } catch (e) { err = e; }
  await Promise.allSettled(__pending.map((p) => p.inner));
  const lost = __pending.filter((p) => !p.used && p.error).map((p) => 'not awaited and failed: ' + p.error.message);
  const out = __out.concat(lost).join('\n');
  if (err !== undefined) {
    const msg = err && err.message !== undefined ? err.message : String(err);
    return JSON.stringify({ out, error: msg, name: String((err && err.name) || ''), stack: String((err && err.stack) || '') });
  }
  return JSON.stringify({ out, value: v === undefined ? null : __fmt(v) });
})()
"#;

/// "eval_script:LINE" in an error, as `: <name> at line N\n  N | code`.
fn locate(error: &str, name: &str, stack: &str, code: &str) -> String {
    let offset = PRELUDE.matches('\n').count() + WRAP_HEAD.matches('\n').count();
    let mut lines = Vec::new();
    for text in [stack, error] {
        for (at, marker) in text.match_indices("eval_script:") {
            let digits: String = text[at + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(line) = digits.parse::<usize>() {
                lines.push(line);
            }
        }
    }
    let count = code.lines().count();
    let line = lines.into_iter().find_map(|line| {
        line.checked_sub(offset)
            .filter(|line| *line >= 1 && *line <= count)
    });
    let message = error.lines().next().unwrap_or(error).trim();
    let kind = if name.is_empty() || message.starts_with(name) {
        String::new()
    } else {
        format!("{name}: ")
    };
    match line {
        Some(line) => {
            let text = code.lines().nth(line - 1).unwrap_or_default().trim();
            format!(" at line {line}: {kind}{message}\n  {line} | {text}")
        }
        None => format!(": {kind}{message}"),
    }
}

type Lines = (String, Option<String>);

async fn execute(
    code: &str,
    env: Arc<Env>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    limits: &Limits,
) -> Result<Lines, (String, String)> {
    let fail = |err: String| (String::new(), format!(": {err}"));
    let runtime = AsyncRuntime::new().map_err(|err| fail(err.to_string()))?;
    runtime.set_memory_limit(limits.memory).await;
    // CPU loops never yield to the tokio timeout; the interrupt handler stops them.
    let deadline = Instant::now() + limits.timeout;
    runtime
        .set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)))
        .await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|err| fail(err.to_string()))?;
    let extra: String = env
        .host
        .as_ref()
        .map(|host| {
            host.names()
                .iter()
                .map(|name| match name.split_once('.') {
                    // `agents.spawn` → a method on a global namespace object.
                    Some((space, method)) => format!(
                        "(globalThis[{space:?}] ??= {{}})[{method:?}] = __tool({name:?});\n"
                    ),
                    None => format!("globalThis[{name:?}] = __tool({name:?});\n"),
                })
                .collect()
        })
        .unwrap_or_default();
    let script = format!("{PRELUDE}{extra}{WRAP_HEAD}{code}{WRAP_TAIL}");
    let source = code.to_string();
    let raw: Result<String, String> = context
        .async_with(async move |ctx| {
            let call = Function::new(
                ctx.clone(),
                Async(move |name: String, args: String| {
                    let env = env.clone();
                    // Number the call and take its file's place in line now,
                    // in the order the program made the calls.
                    let id = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let args: Vec<Value> = serde_json::from_str(&args).unwrap_or_default();
                    let turn = turn(&env, &name, &args);
                    async move { dispatch(&env, id, &name, args, turn).await.to_string() }
                }),
            );
            let call = match call {
                Ok(call) => call,
                Err(err) => return Err(err.to_string()),
            };
            if let Err(err) = ctx.globals().set("__call", call) {
                return Err(err.to_string());
            }
            let promise: Promise = match ctx.eval(script).catch(&ctx) {
                Ok(promise) => promise,
                Err(err) => return Err(err.to_string()),
            };
            promise
                .into_future::<String>()
                .await
                .catch(&ctx)
                .map_err(|err| err.to_string())
        })
        .await;
    // Syntax errors never start the program: point at the line.
    let raw = raw.map_err(|err| fail(locate(&err, "", "", &source)))?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|err| fail(format!(": {err}")))?;
    let out = parsed["out"].as_str().unwrap_or_default().to_string();
    match parsed.get("error").and_then(Value::as_str) {
        Some(err) => {
            let name = parsed["name"].as_str().unwrap_or_default();
            let stack = parsed["stack"].as_str().unwrap_or_default();
            Err((out, locate(err, name, stack, &source)))
        }
        None => Ok((out, parsed["value"].as_str().map(String::from))),
    }
}

/// Run one tool, log it, and answer `{"value": …}` or `{"error": "…"}`.
async fn dispatch(
    env: &Arc<Env>,
    id: usize,
    name: &str,
    args: Vec<Value>,
    turn: Option<Turn>,
) -> Value {
    let text = |index: usize| {
        args.get(index)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let host = env.host.clone().filter(|host| host.names().contains(&name));
    let target = match name {
        "sh" => one_line(&text(0)),
        "ls" if text(0).is_empty() => ".".into(),
        _ if host.is_some() => one_line(
            &host
                .as_ref()
                .map(|h| h.target(name, &args))
                .unwrap_or_default(),
        ),
        "jobs.send" => format!(
            "{} ← {}",
            text(0),
            args.get(1)
                .and_then(Value::as_str)
                .map(|t| t.escape_debug().to_string())
                .unwrap_or_default()
        ),
        "jobs.wait" => match args.first() {
            Some(Value::Array(ids)) => ids
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => text(0),
        },
        _ => text(0),
    };
    let file = text(0);
    let started = Instant::now();
    let _ = env.log.send(Call {
        id,
        running: true,
        verb: name.to_string(),
        target: target.clone(),
        info: String::new(),
        ok: true,
    });
    let (result, short) = if host.is_none() && name.starts_with("jobs.") {
        (env.jobs.call(name, &args).await, None)
    } else if let Some(refusal) = refused(env, name, &file) {
        (Err(refusal), None)
    } else {
        in_time(env, name, args, host, &target, started, turn).await
    };
    if name == "read"
        && let Ok((value, _)) = &result
        && value["type"] == "image"
    {
        env.show(id, &file);
    }
    let (answer, info, ok) = match result {
        Ok((value, info)) => (json!({ "value": value }), info, true),
        Err(err) => (json!({ "error": err }), short.unwrap_or(err), false),
    };
    let call = Call {
        id,
        running: false,
        verb: name.to_string(),
        target,
        info: one_line(&info),
        ok,
    };
    if let Ok(mut done) = env.done.lock() {
        done.push(call.clone());
    }
    let _ = env.log.send(call);
    answer
}

type Done = Result<(Value, String), String>;

/// Why this agent may not make this call, if it may not.
fn refused(env: &Env, name: &str, file: &str) -> Option<String> {
    match name {
        "write" | "edit" if env.read_only => {
            Some("this agent is read-only: report what to change instead".into())
        }
        "write" | "edit" if env.confine && !inside(env, file) => Some(format!(
            "{file} is outside your worktree {}; change files only inside it",
            env.cwd.display()
        )),
        _ => None,
    }
}

/// A call waits this long by default before it moves to the background.
const CALL_WAIT: Duration = Duration::from_secs(5);
/// `jobs.wait` gives up after this long by default.
const JOB_WAIT: Duration = Duration::from_secs(300);
/// jobs.send/read wait at most this long for the answer to go quiet.
const SEND_WAIT: Duration = Duration::from_secs(10);
/// Jobs kept per conversation; the oldest finished ones go first.
const MAX_JOBS: usize = 100;
/// Quiet this long after a line that looks like a prompt: the job is
/// probably waiting for input.
const STALL: Duration = Duration::from_secs(10);
/// jobs.stop: TERM first, then KILL after this long.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Output tail in an end notification, per stream.
const NOTE_TAIL: usize = 1500;

/// Run the call as its own task and wait for it up to its timeout. Still
/// running then: it goes on as a job and this call fails, naming the job
/// (the second value is the short line for the log).
async fn in_time(
    env: &Arc<Env>,
    name: &str,
    args: Vec<Value>,
    host: Option<Arc<dyn Host>>,
    target: &str,
    started: Instant,
    turn: Option<Turn>,
) -> (Done, Option<String>) {
    let wait = host
        .as_ref()
        .and_then(|host| host.timeout(name, &args))
        .or_else(|| {
            args.last()
                .and_then(|opts| opts.get("timeout"))
                .and_then(Value::as_u64)
                .map(Duration::from_millis)
        })
        .unwrap_or(CALL_WAIT);
    let interactive = name == "sh"
        && args
            .get(1)
            .is_some_and(|o| o["interactive"].as_bool() == Some(true));
    let live = Live::default();
    let mut task = tokio::spawn(perform(
        env.clone(),
        name.to_string(),
        args,
        host,
        live.clone(),
        turn,
    ));
    // The program stopped (cancelled, out of time) while the call ran: so
    // does the call, unless it became a job.
    let mut abort = AbortOnDrop(Some(task.abort_handle()));
    if interactive {
        // Asked for a session: it is a job from the start. Answer with what
        // it printed once it goes quiet (a prompt, a banner), or ended.
        abort.0 = None;
        let id = env
            .jobs
            .adopt(format!("{name} {target}"), started, task, live);
        return match env.jobs.find(&id) {
            Ok(job) => {
                let _watching = Watching::new(vec![job.clone()]);
                job.settle(0, Instant::now() + wait).await;
                let report = job.fresh();
                let short = format!("job {id} · {}", job.status());
                (Ok((report, short)), None)
            }
            Err(err) => (Err(err), None),
        };
    }
    match tokio::time::timeout(wait, &mut task).await {
        Ok(Ok(done)) => (done, None),
        Ok(Err(join)) if join.is_panic() => {
            (Err("the tool crashed (a bug in shroom)".into()), None)
        }
        Ok(Err(_)) => (Err("stopped".into()), None),
        Err(_) => {
            abort.0 = None;
            let id = env
                .jobs
                .adopt(format!("{name} {target}"), started, task, live.clone());
            let short = format!("→ job {id} (still running after {})", span(wait));
            let note = moved(&id, wait, &live, env.jobs.notifies());
            live.mark_read();
            (Err(note), Some(short))
        }
    }
}

/// The work of one call, on its own task.
async fn perform(
    env: Arc<Env>,
    name: String,
    args: Vec<Value>,
    host: Option<Arc<dyn Host>>,
    live: Live,
    turn: Option<Turn>,
) -> Done {
    // Wait for the call before this one on the same file; ours holds the
    // line until it is done (or dropped).
    let _mine = match turn {
        Some((earlier, mine)) => {
            if let Some(mut earlier) = earlier {
                let _ = earlier.changed().await;
            }
            Some(mine)
        }
        None => None,
    };
    if let Some(host) = host {
        return host.call(&name, args).await;
    }
    if name == "sh" {
        let cmd = args.first().and_then(Value::as_str).unwrap_or_default();
        return sh(&env, cmd, args.get(1), &live).await;
    }
    // File calls block: off the async threads, so a slow disk can time out.
    tokio::task::spawn_blocking(move || files(&env, &name, &args))
        .await
        .unwrap_or_else(|_| Err("the file tool crashed (a bug in shroom)".into()))
}

fn files(env: &Env, name: &str, args: &[Value]) -> Done {
    let file = args.first().and_then(Value::as_str).unwrap_or_default();
    match name {
        "read" => read(env, file, args.get(1)),
        "write" => match args.get(1).and_then(Value::as_str) {
            Some(body) => write(env, file, body, args.get(2)),
            None => Err(
                "write needs the text as a string: write(path, text). Nothing was written".into(),
            ),
        },
        "edit" => match (
            args.get(1).and_then(Value::as_str),
            args.get(2).and_then(Value::as_str),
        ) {
            (Some(old), Some(new)) => edit(env, file, old, new, args.get(3)),
            _ => Err("edit needs old and new text as strings: edit(path, old, new); to delete text pass '' as new. Nothing was changed".into()),
        },
        "ls" => ls(env, if file.is_empty() { "." } else { file }),
        other => Err(format!("unknown tool {other}")),
    }
}

/// A place in one file's line: the call before (to wait for) and ours (the
/// next one waits until it is dropped).
type Turn = (
    Option<tokio::sync::watch::Receiver<()>>,
    tokio::sync::watch::Sender<()>,
);

/// Calls on one file run in the order they were made, by any agent in this
/// process, even when an earlier one became a job.
fn turn(env: &Env, name: &str, args: &[Value]) -> Option<Turn> {
    if env
        .host
        .as_ref()
        .is_some_and(|host| host.names().contains(&name))
    {
        return None;
    }
    match name {
        "read" | "write" | "edit" => args
            .first()
            .and_then(Value::as_str)
            .and_then(|file| path(env, file).ok())
            .map(|target| queue(&target)),
        _ => None,
    }
}

fn queue(target: &Path) -> Turn {
    type Lines =
        std::sync::Mutex<std::collections::HashMap<PathBuf, tokio::sync::watch::Receiver<()>>>;
    static LINES: std::sync::LazyLock<Lines> = std::sync::LazyLock::new(Lines::default);
    // One key per file however it is named: through symlinks, also for a
    // file not made yet.
    let key = std::fs::canonicalize(target)
        .ok()
        .or_else(|| {
            let parent = std::fs::canonicalize(target.parent()?).ok()?;
            Some(parent.join(target.file_name()?))
        })
        .unwrap_or_else(|| target.to_path_buf());
    let (mine, next) = tokio::sync::watch::channel(());
    let earlier = LINES.lock().ok().and_then(|mut lines| {
        lines.retain(|_, last| last.has_changed().is_ok());
        lines.insert(key, next)
    });
    (earlier, mine)
}

struct AbortOnDrop(Option<tokio::task::AbortHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

fn span(time: Duration) -> String {
    if time < Duration::from_secs(1) {
        format!("{}ms", time.as_millis())
    } else if time.subsec_millis() == 0 {
        format!("{}s", time.as_secs())
    } else {
        format!("{:.1}s", time.as_secs_f64())
    }
}

/// The error of a call that became a job: what to do next, and what an sh
/// command printed so far.
fn moved(id: &str, wait: Duration, live: &Live, notified: bool) -> String {
    let mut note = format!(
        "still running after {}, so this call fails here; it goes on in the background as job {id}. ",
        span(wait)
    );
    if notified {
        note.push_str(&format!("You get a message when it ends (or seems stuck at a prompt), so don't poll or sleep for it: do other work or end your turn; jobs.wait('{id}') only if you need its result to go on. "));
    } else {
        note.push_str(&format!("jobs.wait('{id}') for its result. "));
    }
    note.push_str(&format!("Don't start it again while it runs; jobs.stop('{id}') if it is no longer needed. Next time give slow work {{timeout: ms}} up front"));
    if live.buffers().is_some() && live.size() == 0 {
        note.push_str("\nIt has printed nothing yet. A program that buffers output into a pipe (python without -u, a pipe into tail or grep) shows nothing until it ends.");
    }
    if let Some((out, err)) = live.tails(2000) {
        for (name, text) in [("stdout", out), ("stderr", err)] {
            if !text.is_empty() {
                note.push_str(&format!("\n{name} so far (end):\n{text}"));
            }
        }
    }
    note
}

/// A running sh as its job sees it: what it printed so far, its stdin when
/// that was kept open, and how much of the output was already handed out.
#[derive(Clone, Default)]
struct Live(Arc<Pipes>);

#[derive(Default)]
struct Pipes {
    out: std::sync::Mutex<Option<(Buffer, Buffer)>>,
    stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    /// Bytes of stdout and stderr already returned by send/read.
    seen: std::sync::Mutex<(usize, usize)>,
    /// Its process group (bash's pid).
    pid: std::sync::Mutex<Option<u32>>,
}

impl Live {
    fn set(&self, out: &Buffer, err: &Buffer) {
        if let Ok(mut live) = self.0.out.lock() {
            *live = Some((out.clone(), err.clone()));
        }
    }

    fn buffers(&self) -> Option<(Buffer, Buffer)> {
        self.0.out.lock().ok()?.clone()
    }

    /// The last BTcharsBT of stdout and stderr so far.
    fn tails(&self, chars: usize) -> Option<(String, String)> {
        let (out, err) = self.buffers()?;
        Some((out.tail(chars), err.tail(chars)))
    }

    /// Bytes printed so far (stdout + stderr).
    fn size(&self) -> usize {
        self.buffers().map_or(0, |(out, err)| out.len() + err.len())
    }

    /// Output not handed out yet; marks it handed out.
    fn unread(&self) -> (String, String) {
        let Some((out, err)) = self.buffers() else {
            return Default::default();
        };
        let Ok(mut seen) = self.0.seen.lock() else {
            return Default::default();
        };
        let new = (out.since(seen.0), err.since(seen.1));
        *seen = (out.len(), err.len());
        new
    }

    fn pid(&self) -> Option<u32> {
        *self.0.pid.lock().ok()?
    }

    /// Its stdin is still open (an interactive session).
    fn open(&self) -> bool {
        self.0
            .stdin
            .try_lock()
            .map_or(true, |stdin| stdin.is_some())
    }

    /// Output the model has not been handed yet.
    fn unseen(&self) -> bool {
        match (self.buffers(), self.0.seen.lock()) {
            (Some((out, err)), Ok(seen)) => (out.len(), err.len()) != *seen,
            _ => false,
        }
    }

    /// The last line of stdout or stderr, if it looks like a prompt.
    fn prompt(&self) -> Option<String> {
        let (out, err) = self.tails(1024)?;
        [err, out].into_iter().find_map(|text| {
            let line = text.rsplit(['\n', '\r']).next().unwrap_or_default().trim();
            asks(line).then(|| line.to_string())
        })
    }

    /// What was printed until now counts as seen (the model got its end).
    fn mark_read(&self) {
        if let (Some((out, err)), Ok(mut seen)) = (self.buffers(), self.0.seen.lock()) {
            *seen = (out.len(), err.len());
        }
    }

    /// Write to the command's stdin (kept open by {interactive: true}).
    async fn send(&self, text: &str, close: bool) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut stdin = self.0.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else {
            return Err(
                "its input is closed: only sh(cmd, {interactive: true}) keeps it open".into(),
            );
        };
        if !text.is_empty() {
            let write = async {
                pipe.write_all(text.as_bytes()).await?;
                pipe.flush().await
            };
            match tokio::time::timeout(Duration::from_secs(5), write).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    *stdin = None;
                    return Err(format!("cannot write to it: {err} (it closed its input)"));
                }
                Err(_) => {
                    return Err("it is not reading its input (5s); nothing more was sent".into());
                }
            }
        }
        if close {
            *stdin = None;
        }
        Ok(())
    }
}

/// Calls that outlived their timeout. They keep running as jobs, which the
/// model checks with `jobs.wait` from later programs and turns too: the
/// embedder keeps one per conversation (a fresh one per run by default).
pub struct Jobs {
    next: std::sync::atomic::AtomicUsize,
    list: std::sync::Mutex<Vec<Arc<Job>>>,
    notes: Arc<Notes>,
    /// STALL in ms (tests shorten it).
    stall: std::sync::atomic::AtomicU64,
}

/// Notifications not delivered yet, and who to wake for them.
#[derive(Default)]
struct Notes {
    queue: std::sync::Mutex<Vec<Note>>,
    wake: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

enum Note {
    Ended(String),
    /// The job and the prompt line it seems stuck at.
    Waiting(String, String),
}

impl Notes {
    fn push(&self, note: Note) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push(note);
        }
        let wake = self.wake.lock().ok().and_then(|wake| wake.clone());
        if let Some(wake) = wake {
            wake();
        }
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            next: Default::default(),
            list: Default::default(),
            notes: Default::default(),
            stall: (STALL.as_millis() as u64).into(),
        }
    }
}

/// Calls waiting on jobs right now (jobs.wait, send, read): they hand the
/// model its end, so no notification is needed. Dropped: an ended job's end
/// was seen.
struct Watching(Vec<Arc<Job>>);

impl Watching {
    fn new(jobs: Vec<Arc<Job>>) -> Self {
        for job in &jobs {
            job.watchers
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Self(jobs)
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        for job in &self.0 {
            if job.status() != "running" {
                job.claimed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            job.watchers
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

struct Job {
    id: String,
    /// "sh cargo build", for the list.
    call: String,
    started: Instant,
    state: std::sync::Mutex<JobState>,
    ended: tokio::sync::watch::Sender<bool>,
    /// Set once the task is gone (its processes killed), after a stop too.
    gone: tokio::sync::watch::Sender<bool>,
    abort: tokio::task::AbortHandle,
    live: Live,
    /// Calls waiting on it now (see Watching).
    watchers: std::sync::atomic::AtomicUsize,
    /// The model got its end (a wait, send, read or stop answered).
    claimed: std::sync::atomic::AtomicBool,
    /// Its last line while it seems stuck at that prompt.
    asking: tokio::sync::watch::Sender<Option<String>>,
    notes: Arc<Notes>,
}

enum JobState {
    Running,
    /// `{"value": …}` or `{"error": …}`, and how long it ran.
    Ended(Value, Duration),
    Stopped(Duration),
}

impl Job {
    /// The notification for its end.
    fn ending(&self) -> String {
        let (id, call) = (&self.id, one_line(&self.call));
        let status = self.status();
        let Ok(state) = self.state.lock() else {
            return String::new();
        };
        let JobState::Ended(answer, took) = &*state else {
            return String::new();
        };
        let mut body = format!(
            "<job-notification>\n<id>{id}</id>\n<call>{call}</call>\n<status>{status}</status>\n"
        );
        let after = span(Duration::from_secs(took.as_secs().max(1)));
        let value = &answer["value"];
        let what = match (
            answer.get("error"),
            value.get("code").and_then(Value::as_i64),
        ) {
            (Some(error), _) => format!(
                "failed after {after}: {}",
                clip(&one_line(error.as_str().unwrap_or_default()), 300)
            ),
            (None, Some(0)) => format!("finished after {after} (exit 0)"),
            (None, Some(code)) => format!("exited with code {code} after {after}"),
            (None, None) => format!("finished after {after}"),
        };
        if let Some(code) = value.get("code") {
            body.push_str(&format!("<code>{code}</code>\n"));
        }
        body.push_str(&format!("<summary>Job {id} \"{call}\" {what}</summary>\n"));
        for (name, text) in [("out", &value["out"]), ("err", &value["err"])] {
            let text = text.as_str().unwrap_or_default();
            if !text.trim().is_empty() {
                body.push_str(&format!(
                    "<{name}-tail>\n{}\n</{name}-tail>\n",
                    last_chars(text, NOTE_TAIL)
                ));
            }
        }
        body.push_str(&format!(
            "</job-notification>\nSent because job {id} ended while nothing waited on it. jobs.wait('{id}') returns its whole result if you need more; don't call it just to confirm."
        ));
        body
    }

    /// The notification for a job that seems stuck at the prompt `line`.
    fn waiting(&self, line: &str, quiet: Duration) -> String {
        let (id, call) = (&self.id, one_line(&self.call));
        let hint = if self.live.open() {
            format!(
                "Answer it with jobs.send('{id}', 'y\\n') (the text it needs, ending with \\n), or jobs.stop('{id}')."
            )
        } else {
            format!(
                "Its input is closed, so it cannot be answered: jobs.stop('{id}'), then run it again with {{input: 'y\\n'}} or a non-interactive flag (-y, --yes)."
            )
        };
        format!(
            "<job-notification>\n<id>{id}</id>\n<call>{call}</call>\n<status>waiting</status>\n<summary>Job {id} \"{call}\" seems to be waiting for input: nothing for {} after: {}</summary>\n</job-notification>\n{hint}",
            span(quiet),
            clip(line, 300)
        )
    }

    /// Watch for a prompt it waits at: quiet for `stall` after a line that
    /// looks like one. Wakes jobs.wait (through `asking`), and tells the
    /// model when nothing waits on it and it has not seen that line.
    async fn watch_prompts(&self, stall: Duration) {
        let mut ended = self.ended.subscribe();
        let (mut last, mut since, mut checked) = (0, Instant::now(), 0);
        loop {
            let tick = tokio::time::timeout(Duration::from_millis(250), ended.wait_for(|e| *e));
            if tick.await.is_ok() {
                return;
            }
            let size = self.live.size();
            if size != last {
                (last, since) = (size, Instant::now());
                self.asking
                    .send_if_modified(|asking| asking.take().is_some());
                continue;
            }
            if size == 0 || checked == size || since.elapsed() < stall {
                continue;
            }
            checked = size;
            let Some(line) = self.live.prompt() else {
                continue;
            };
            self.asking.send_replace(Some(line.clone()));
            if self.watchers.load(std::sync::atomic::Ordering::SeqCst) == 0 && self.live.unseen() {
                self.notes.push(Note::Waiting(self.id.clone(), line));
            }
        }
    }

    /// Wait for output past `size` bytes to arrive and go quiet, for the job
    /// to end, or for `deadline`.
    async fn settle(&self, size: usize, deadline: Instant) {
        const QUIET: Duration = Duration::from_millis(300);
        let mut ended = self.ended.subscribe();
        let (mut last, mut since) = (self.live.size(), Instant::now());
        while Instant::now() < deadline && !*ended.borrow() {
            let step =
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now()));
            let _ = tokio::time::timeout(step, ended.changed()).await;
            let now = self.live.size();
            if now != last {
                (last, since) = (now, Instant::now());
            } else if now > size && since.elapsed() >= QUIET {
                return;
            }
        }
    }

    /// Status and the output not handed out yet (and the exit code once it
    /// ended): what send and read answer.
    fn fresh(&self) -> Value {
        let (out, err) = self.live.unread();
        let mut report = json!({
            "id": self.id,
            "status": self.status(),
            "out": clip(out.trim_end(), MAX_OUTPUT),
            "err": clip(err.trim_end(), MAX_OUTPUT),
        });
        if let Ok(state) = self.state.lock()
            && let JobState::Ended(answer, _) = &*state
        {
            if let Some(code) = answer["value"].get("code") {
                report["code"] = code.clone();
            }
            if let Some(error) = answer.get("error") {
                report["error"] = error.clone();
            }
        }
        report
    }

    /// The first end counts (a stop, then the cancelled task, is one stop).
    fn end(&self, state: JobState) {
        if let Ok(mut now) = self.state.lock()
            && matches!(*now, JobState::Running)
        {
            *now = state;
        }
        self.ended.send_replace(true);
    }

    fn status(&self) -> &'static str {
        match self.state.lock().as_deref() {
            Ok(JobState::Running) => "running",
            Ok(JobState::Ended(answer, _)) if answer.get("value").is_some() => "done",
            Ok(JobState::Stopped(_)) => "stopped",
            _ => "failed",
        }
    }

    fn report(&self) -> Value {
        let mut report = json!({ "id": self.id, "call": self.call, "status": self.status() });
        let secs = |time: Duration| json!((time.as_secs_f64() * 10.0).round() / 10.0);
        let Ok(state) = self.state.lock() else {
            return report;
        };
        match &*state {
            JobState::Running => {
                report["secs"] = secs(self.started.elapsed());
                if let Some((out, err)) = self.live.tails(4000) {
                    report["out"] = out.into();
                    report["err"] = err.into();
                }
                if let Some(line) = &*self.asking.borrow() {
                    report["waiting"] = line.clone().into();
                }
            }
            JobState::Ended(answer, took) => {
                report["secs"] = secs(*took);
                match answer.get("value") {
                    Some(value) => report["value"] = value.clone(),
                    None => report["error"] = answer["error"].clone(),
                }
            }
            JobState::Stopped(took) => report["secs"] = secs(*took),
        }
        report
    }
}

/// Jobs nobody can reach any more are stopped, not orphaned.
impl Drop for Jobs {
    fn drop(&mut self) {
        if let Ok(list) = self.list.lock() {
            for job in list.iter() {
                job.abort.abort();
            }
        }
    }
}

impl Jobs {
    /// Keep `task` running as a new job; its id.
    fn adopt(
        &self,
        call: String,
        started: Instant,
        task: tokio::task::JoinHandle<Done>,
        live: Live,
    ) -> String {
        let id = format!(
            "j{}",
            self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
        );
        let job = Arc::new(Job {
            id: id.clone(),
            call,
            started,
            state: std::sync::Mutex::new(JobState::Running),
            ended: tokio::sync::watch::channel(false).0,
            gone: tokio::sync::watch::channel(false).0,
            abort: task.abort_handle(),
            live,
            watchers: Default::default(),
            claimed: Default::default(),
            asking: tokio::sync::watch::channel(None).0,
            notes: self.notes.clone(),
        });
        let stall = Duration::from_millis(self.stall.load(std::sync::atomic::Ordering::Relaxed));
        let watcher = job.clone();
        tokio::spawn(async move { watcher.watch_prompts(stall).await });
        if let Ok(mut list) = self.list.lock() {
            if list.len() >= MAX_JOBS
                && let Some(old) = list.iter().position(|job| job.status() != "running")
            {
                list.remove(old);
            }
            list.push(job.clone());
        }
        tokio::spawn(async move {
            let took = job.started.elapsed();
            let state = match task.await {
                Ok(Ok((value, _))) => JobState::Ended(json!({ "value": value }), took),
                Ok(Err(err)) => JobState::Ended(json!({ "error": err }), took),
                Err(join) if join.is_cancelled() => JobState::Stopped(took),
                Err(_) => JobState::Ended(
                    json!({ "error": "the tool crashed (a bug in shroom)" }),
                    took,
                ),
            };
            let took = job.started.elapsed();
            job.end(match state {
                JobState::Ended(answer, _) => JobState::Ended(answer, took),
                _ => JobState::Stopped(took),
            });
            job.gone.send_replace(true);
            // Nothing handed the model its end: tell it (unless something
            // does before the note is delivered).
            if !job.claimed.load(std::sync::atomic::Ordering::SeqCst)
                && job.watchers.load(std::sync::atomic::Ordering::SeqCst) == 0
                && job.status() != "stopped"
            {
                job.notes.push(Note::Ended(job.id.clone()));
            }
        });
        id
    }

    /// Call `wake` when a notification is queued (see [`Jobs::pending`]),
    /// so an idle conversation can start a turn for it.
    pub fn notify(&self, wake: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.notes.wake.lock() {
            *slot = Some(Arc::new(wake));
        }
    }

    /// Whether an idle conversation is woken for notifications.
    fn notifies(&self) -> bool {
        self.notes.wake.lock().is_ok_and(|wake| wake.is_some())
    }

    /// Notifications for the model, taken from the queue: a job that ended
    /// while nothing waited on it, one that seems stuck at a prompt. Ones
    /// that went stale (a jobs.wait got the end, new output came) are
    /// dropped.
    pub fn pending(&self) -> Vec<String> {
        let queue = self
            .notes
            .queue
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        let mut texts: Vec<String> = Vec::new();
        for note in queue {
            if let Some(text) = self.deliver(&note)
                && !texts.contains(&text)
            {
                texts.push(text);
            }
        }
        texts
    }

    /// Whether [`Jobs::pending`] has anything (without taking it).
    pub fn has_pending(&self) -> bool {
        self.notes
            .queue
            .lock()
            .is_ok_and(|queue| queue.iter().any(|note| self.live_note(note)))
    }

    fn live_note(&self, note: &Note) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        match note {
            Note::Ended(id) => self
                .find(id)
                .is_ok_and(|job| !job.claimed.load(SeqCst) && job.watchers.load(SeqCst) == 0),
            Note::Waiting(id, line) => self.find(id).is_ok_and(|job| {
                job.status() == "running"
                    && job.watchers.load(SeqCst) == 0
                    && job.asking.borrow().as_deref() == Some(line.as_str())
            }),
        }
    }

    fn deliver(&self, note: &Note) -> Option<String> {
        if !self.live_note(note) {
            return None;
        }
        match note {
            Note::Ended(id) => {
                let job = self.find(id).ok()?;
                job.claimed.store(true, std::sync::atomic::Ordering::SeqCst);
                Some(job.ending())
            }
            Note::Waiting(id, line) => {
                let job = self.find(id).ok()?;
                Some(job.waiting(
                    line,
                    Duration::from_millis(self.stall.load(std::sync::atomic::Ordering::Relaxed)),
                ))
            }
        }
    }

    #[cfg(test)]
    fn stall(&self, stall: Duration) {
        self.stall.store(
            stall.as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    fn find(&self, id: &str) -> Result<Arc<Job>, String> {
        let list = self
            .list
            .lock()
            .map_err(|_| "jobs unavailable".to_string())?;
        list.iter().find(|job| job.id == id).cloned().ok_or_else(|| {
            let known: Vec<&str> = list.iter().map(|job| job.id.as_str()).collect();
            if known.is_empty() {
                format!("no job {id:?}: this conversation has no jobs (a1-style ids are subagents: agents.wait)")
            } else {
                format!("no job {id:?}; jobs here: {}", known.join(", "))
            }
        })
    }

    async fn call(&self, name: &str, args: &[Value]) -> Done {
        let id = args.first().and_then(Value::as_str).unwrap_or_default();
        match name {
            "jobs.wait" => {
                let timeout = args
                    .get(1)
                    .and_then(|opts| opts["timeout"].as_u64())
                    .map_or(JOB_WAIT, Duration::from_millis);
                let deadline = Instant::now() + timeout;
                // Until it ends, or seems stuck at a prompt.
                let wait = move |job: Arc<Job>| async move {
                    let mut ended = job.ended.subscribe();
                    let mut asking = job.asking.subscribe();
                    let left = deadline.saturating_duration_since(Instant::now());
                    let either = async {
                        tokio::select! {
                            _ = async { let _ = ended.wait_for(|ended| *ended).await; } => {}
                            _ = async { let _ = asking.wait_for(Option::is_some).await; } => {}
                        }
                    };
                    let _ = tokio::time::timeout(left, either).await;
                    job
                };
                let state = |job: &Job| {
                    if job.asking.borrow().is_some() && job.status() == "running" {
                        format!("{} waiting for input", job.id)
                    } else {
                        format!("{} {}", job.id, job.status())
                    }
                };
                match args.first() {
                    Some(Value::Array(ids)) => {
                        let jobs = ids
                            .iter()
                            .filter_map(Value::as_str)
                            .map(|id| self.find(id))
                            .collect::<Result<Vec<_>, _>>()?;
                        let _watching = Watching::new(jobs.clone());
                        let (mut reports, mut line) = (Vec::new(), Vec::new());
                        for job in jobs {
                            let job = wait(job).await;
                            line.push(state(&job));
                            reports.push(job.report());
                        }
                        Ok((Value::Array(reports), line.join(" · ")))
                    }
                    _ => {
                        let job = self.find(id)?;
                        let _watching = Watching::new(vec![job.clone()]);
                        let job = wait(job).await;
                        Ok((job.report(), state(&job)))
                    }
                }
            }
            "jobs.list" => {
                let list = self
                    .list
                    .lock()
                    .map(|list| list.clone())
                    .unwrap_or_default();
                let items = list
                    .iter()
                    .map(|job| {
                        let report = job.report();
                        json!({ "id": report["id"], "call": report["call"], "status": report["status"], "secs": report["secs"] })
                    })
                    .collect();
                Ok((Value::Array(items), format!("{} jobs", list.len())))
            }
            "jobs.send" | "jobs.read" => {
                let job = self.find(id)?;
                let _watching = Watching::new(vec![job.clone()]);
                let opts = args.get(if name == "jobs.send" { 2 } else { 1 });
                let wait = opts
                    .and_then(|opts| opts["wait"].as_u64())
                    .map_or(SEND_WAIT, Duration::from_millis);
                let size = job.live.size();
                if name == "jobs.send" {
                    let Some(text) = args.get(1).and_then(Value::as_str) else {
                        return Err(
                            "jobs.send needs the text: jobs.send(id, 'ls\\n') (end lines with \\n)"
                                .into(),
                        );
                    };
                    let close = opts.is_some_and(|o| o["close"].as_bool() == Some(true));
                    if job.status() != "running" {
                        return Err(format!(
                            "job {id} has ended ({}); nothing was sent",
                            job.status()
                        ));
                    }
                    job.live
                        .send(text, close)
                        .await
                        .map_err(|err| format!("job {id}: {err}"))?;
                }
                job.settle(size, Instant::now() + wait).await;
                let report = job.fresh();
                let line = format!(
                    "{} · {} chars",
                    job.status(),
                    report["out"].as_str().map_or(0, str::len)
                        + report["err"].as_str().map_or(0, str::len)
                );
                Ok((report, line))
            }
            "jobs.stop" => {
                let job = self.find(id)?;
                job.claimed.store(true, std::sync::atomic::Ordering::SeqCst);
                let running = job.status() == "running";
                job.end(JobState::Stopped(job.started.elapsed()));
                let mut gone = job.gone.subscribe();
                let pid = job.live.pid().filter(|_| running);
                // TERM first, so it can clean up (a lock, a temp file).
                if let Some(pid) = pid {
                    signal_group(pid, "TERM");
                    let _ = tokio::time::timeout(STOP_GRACE, gone.wait_for(|gone| *gone)).await;
                }
                job.abort.abort();
                // Answer once it is really gone, so "stopped" means its
                // processes are dead.
                let _ =
                    tokio::time::timeout(Duration::from_secs(5), gone.wait_for(|gone| *gone)).await;
                // What it left behind in its group (bash exited on TERM).
                if let Some(pid) = pid {
                    signal_group(pid, "KILL");
                }
                Ok((job.report(), job.status().to_string()))
            }
            other => Err(format!("unknown function {other}")),
        }
    }
}

fn path(env: &Env, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("missing path".into());
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(env.cwd.join(path))
}

/// `file` resolves inside the root folder (`..` and `.` folded away, no
/// file system access: a file that does not exist yet counts too).
fn inside(env: &Env, file: &str) -> bool {
    let Ok(full) = path(env, file) else {
        return false;
    };
    let mut folded = PathBuf::new();
    for part in full.components() {
        match part {
            std::path::Component::ParentDir => {
                folded.pop();
            }
            std::path::Component::CurDir => {}
            other => folded.push(other),
        }
    }
    // Then through the file system: the deepest part that exists, with its
    // symlinks resolved, must still be inside the (resolved) root, so a link
    // in the worktree cannot carry a write out of it.
    let root = std::fs::canonicalize(&env.cwd).unwrap_or_else(|_| env.cwd.clone());
    let mut probe = folded.as_path();
    let real = loop {
        if let Ok(real) = std::fs::canonicalize(probe) {
            break real;
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => break folded.clone(),
        }
    };
    folded.starts_with(&env.cwd) && real.starts_with(&root)
}

/// "no such file" plus up to five look-alike names from the same directory.
fn missing(target: &Path, file: &str) -> String {
    let Some(dir) = target.parent() else {
        return format!("no such file: {file}");
    };
    if !dir.is_dir() {
        return format!("no such directory: {}", dir.display());
    }
    let wanted = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mut similar: Vec<(usize, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .map(|name| (distance(&wanted, &name.to_lowercase()), name))
        .filter(|(score, name)| *score <= (name.len() / 3).max(2))
        .collect();
    similar.sort();
    let similar: Vec<String> = similar.into_iter().take(5).map(|(_, name)| name).collect();
    if similar.is_empty() {
        format!("no such file: {file}")
    } else {
        format!(
            "no such file: {file} (similar here: {})",
            similar.join(", ")
        )
    }
}

/// Lines per read when the caller gave no limit.
const PAGE: usize = 2000;

/// An image file as a value: what it is, never its bytes. Header only, so
/// reading many images to compare their sizes is cheap.
fn image_value(target: &Path, file: &str, bytes: u64) -> Option<Value> {
    let reader = ::image::ImageReader::open(target)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    let mime = match format {
        ::image::ImageFormat::Png => "image/png",
        ::image::ImageFormat::Jpeg => "image/jpeg",
        ::image::ImageFormat::Gif => "image/gif",
        ::image::ImageFormat::WebP => "image/webp",
        _ => return None,
    };
    let (width, height) = reader.into_dimensions().ok()?;
    Some(
        json!({ "type": "image", "path": file, "width": width, "height": height, "mime": mime, "bytes": bytes }),
    )
}

/// A filesystem error as the model should read it: "no such file" (with
/// look-alikes) only when the file is really missing, else the real cause,
/// so a permission or encoding problem is never mistaken for an absent file
/// (and "recreated" with write).
fn io_error(target: &Path, file: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        missing(target, file)
    } else {
        format!("cannot open {file}: {err}")
    }
}

/// Text that is not UTF-8 would come back with its bad bytes replaced, and
/// writing that back would change them: refused, with where to go instead.
fn utf8(bytes: Vec<u8>, file: &str) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|err| {
        format!(
            "{file} is not UTF-8 text (bad byte at offset {}); reading it as text would change its bytes. Inspect or change it with sh (iconv, sed, python)",
            err.utf8_error().valid_up_to()
        )
    })
}

fn read(env: &Env, file: &str, opts: Option<&Value>) -> Done {
    let target = path(env, file)?;
    let meta = std::fs::metadata(&target).map_err(|err| io_error(&target, file, &err))?;
    if meta.is_dir() {
        return Err(format!("{file} is a directory; use ls('{file}')"));
    }
    // Reading an image is looking at it: the model sees it, the user too.
    // (dispatch marks it to show: it knows the call's number.)
    if let Some(value) = image_value(&target, file, meta.len()) {
        let info = format!("▣ {}×{}", value["width"], value["height"]);
        return Ok((value, info));
    }
    let bytes = std::fs::read(&target).map_err(|err| io_error(&target, file, &err))?;
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return Err(format!(
            "{file} is binary ({} bytes); inspect it with sh, e.g. sh('file {file}')",
            bytes.len()
        ));
    }
    let text = utf8(bytes, file)?;
    let total = text.lines().count();
    let offset = opts.and_then(|o| o["offset"].as_u64()).unwrap_or(1).max(1) as usize;
    let limit = opts.and_then(|o| o["limit"].as_u64()).map(|n| n as usize);
    let take = limit.unwrap_or(PAGE);
    // The whole file: exactly its text, line ends and last newline kept.
    if offset == 1 && take >= total {
        return Ok((json!(text), format!("{total} lines")));
    }
    if offset > total.max(1) {
        return Err(format!(
            "{file} has only {total} lines; offset {offset} is past the end"
        ));
    }
    // Part of it: the lines as they are (CRLF kept), then a note saying
    // which part. The note marks the text as partial, so it can never be
    // written back over the file by mistake (see cut_note).
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .skip(offset - 1)
        .take(take)
        .collect();
    let end = offset - 1 + lines.len();
    let mut page = lines.concat();
    if !page.ends_with('\n') {
        page.push('\n');
    }
    let more = if end < total {
        format!("; read('{file}', {{offset: {}}}) for more", end + 1)
    } else {
        String::new()
    };
    page.push_str(&format!("…[lines {offset}-{end} of {total}{more}]"));
    Ok((json!(page), format!("L{offset}-{end} of {total}")))
}

/// The note a cut-short result carries in place of what was left out. Text
/// holding one is a partial copy; writing it back would drop the rest of
/// the file, so write and edit refuse it. Every note the tools and the
/// backend make, each with real numbers (the source that formats them has
/// `{placeholders}` there and passes):
/// - read()'s part note: `…[lines A-B of N]`, maybe with "; read(..) for more";
/// - clip: `…N characters truncated…`;
/// - the backend's long-result note: `…[full output (N characters) in …]`;
/// - compaction and the backend's fallback: `…[N characters cut]…`.
fn cut_note(text: &str) -> Option<&'static str> {
    // A run of digits at the start of `s`, and what follows it.
    fn number(s: &str) -> Option<&str> {
        let rest = s.trim_start_matches(|c: char| c.is_ascii_digit());
        (rest.len() < s.len()).then_some(rest)
    }
    const PART: &str = "read()'s part note (only some lines of the file were read)";
    const CUT: &str = "a cut in long output (its middle was left out)";
    for (at, _) in text.match_indices('…') {
        let rest = &text[at + '…'.len_utf8()..];
        if let Some(lines) = rest.strip_prefix("[lines ")
            && let Some(rest) = number(lines).and_then(|r| r.strip_prefix('-'))
            && let Some(rest) = number(rest).and_then(|r| r.strip_prefix(" of "))
            && let Some(rest) = number(rest)
            && (rest.starts_with(']') || rest.starts_with(';'))
        {
            return Some(PART);
        }
        if let Some(rest) = number(rest)
            && rest.starts_with(" characters truncated…")
        {
            return Some(CUT);
        }
        if let Some(saved) = rest.strip_prefix("[full output (")
            && number(saved).is_some_and(|r| r.starts_with(" characters)"))
        {
            return Some(CUT);
        }
        if let Some(cut) = rest.strip_prefix('[')
            && number(cut).is_some_and(|r| r.starts_with(" characters cut]"))
        {
            return Some(CUT);
        }
    }
    None
}

fn refuse_cut(file: &str, text: &str) -> Result<(), String> {
    match cut_note(text) {
        Some(what) => Err(format!(
            "not written: the text holds {what}, so it is not the whole of what it came \
             from and writing it would cut {file} short. Change files with edit(), or read \
             the whole file (every page) before a write"
        )),
        None => Ok(()),
    }
}

/// Replace `target`'s contents in one step: written to a temporary file
/// beside it, synced, then renamed over it, so a full disk or a crash leaves
/// the old file whole. A symlink is written through (the file it points at
/// changes, the link stays) and the old file's permissions are kept.
fn save(target: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    static TEMPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let target = match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(target)
            .map_err(|err| format!("{} is a broken symlink ({err})", target.display()))?,
        _ => target.to_path_buf(),
    };
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(format!("{} is not a file path", target.display()));
    };
    let temp = dir.join(format!(
        ".{}.shroom-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, &target)
    })();
    written.map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        format!("not written ({err}); {} is unchanged", target.display())
    })
}

fn write(env: &Env, file: &str, text: &str, opts: Option<&Value>) -> Done {
    refuse_cut(file, text)?;
    let target = path(env, file)?;
    if target.is_dir() {
        return Err(format!("{file} is a directory"));
    }
    let before = std::fs::read(&target).ok();
    let force = opts.and_then(|o| o["force"].as_bool()).unwrap_or(false);
    let lines = |bytes: &[u8]| String::from_utf8_lossy(bytes).lines().count();
    if let Some(before) = &before
        && !before.is_empty()
        && text.is_empty()
        && !force
    {
        return Err(format!(
            "not written: this would empty {file} ({} lines). To empty it on purpose: write('{file}', '', {{force: true}})",
            lines(before)
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    save(&target, text.as_bytes())?;
    let now = text.lines().count();
    let info = match before {
        Some(before) => format!("{} → {now} lines", lines(&before)),
        None => format!("new · {now} lines"),
    };
    Ok((Value::Null, info))
}

fn edit(env: &Env, file: &str, old: &str, new: &str, opts: Option<&Value>) -> Done {
    let target = path(env, file)?;
    let bytes = std::fs::read(&target).map_err(|err| io_error(&target, file, &err))?;
    let text = utf8(bytes, file)?;
    if old.is_empty() {
        return Err("old text is empty; to create or replace a whole file use write".into());
    }
    if old == new {
        return Err("old and new text are identical; nothing to change".into());
    }
    refuse_cut(file, new)?;
    // A CRLF file: text written with \n is meant with the file's own line
    // ends, both what to find and what replaces it.
    let (old, new) = match text.contains("\r\n") && !old.contains('\r') {
        true => {
            let crlf = |s: &str| s.replace("\r\n", "\n").replace('\n', "\r\n");
            match text.contains(&crlf(old)) {
                true => (crlf(old), crlf(new)),
                false => (old.to_string(), new.to_string()),
            }
        }
        false => (old.to_string(), new.to_string()),
    };
    let count = text.matches(&old).count();
    let all = opts.and_then(|o| o["all"].as_bool()).unwrap_or(false);
    match count {
        0 => return Err(not_found(&text, &old, file)),
        n if n > 1 && !all => {
            return Err(format!(
                "old text matches {n} times in {file}; add context or pass {{all:true}}"
            ));
        }
        _ => {}
    }
    save(&target, text.replace(&old, &new).as_bytes())?;
    let lines = |s: &str| s.lines().count().max(1) * count;
    Ok((Value::Null, format!("+{} −{}", lines(&new), lines(&old))))
}

fn ls(env: &Env, dir: &str) -> Done {
    let mut names: Vec<String> = std::fs::read_dir(path(env, dir)?)
        .map_err(|err| err.to_string())?
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    let info = format!("{} entries", names.len());
    Ok((json!(names), info))
}

async fn sh(env: &Env, cmd: &str, opts: Option<&Value>, live: &Live) -> Done {
    if cmd.trim().is_empty() {
        return Err("empty command".into());
    }
    let cwd = match opts.and_then(|o| o["cwd"].as_str()) {
        Some(dir) => env.cwd.join(dir),
        None => env.cwd.clone(),
    };
    let input = opts.and_then(|o| o["input"].as_str()).map(String::from);
    let interactive = opts.is_some_and(|o| o["interactive"].as_bool() == Some(true));
    let start = Instant::now();
    let mut child = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        // No stdin unless asked: rg/grep without a path read a piped one.
        .stdin(if input.is_some() || interactive {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Own process group: a timeout or a stopped run kills everything the
        // command started, not just bash.
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| err.to_string())?;
    let group = Group(child.id());
    if let Ok(mut pid) = live.0.pid.lock() {
        *pid = child.id();
    }
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err("no pipes".into());
    };
    let (out_buf, err_buf) = (Buffer::default(), Buffer::default());
    live.set(&out_buf, &err_buf);
    if let Some(stdin) = child.stdin.take() {
        *live.0.stdin.lock().await = Some(stdin);
        // Written on its own task: a command that reads slowly must not
        // stop its output from being drained.
        let live = live.clone();
        tokio::spawn(async move {
            let _ = live
                .send(input.as_deref().unwrap_or_default(), !interactive)
                .await;
        });
    }
    let out_task = tokio::spawn(drain(stdout, out_buf.clone()));
    let err_task = tokio::spawn(drain(stderr, err_buf.clone()));
    // No limit here: a call that outlives its timeout goes on as a job, and
    // jobs.stop (or the run being dropped) kills the group.
    let status = child.wait().await.map_err(|err| err.to_string())?;
    // `cmd &` leaves children holding the pipes open, so EOF may never come.
    // Once bash itself has exited, give the pipes a moment, then take what
    // arrived and stop reading.
    let grace = Duration::from_millis(200);
    let finished = tokio::time::timeout(grace, async {
        let _ = out_task.await;
        let _ = err_task.await;
    })
    .await
    .is_ok();
    // Notes about the output go to err, so out stays exactly what the
    // command printed (scripts parse it).
    let (out, mut err) = (out_buf.text(), err_buf.text());
    if !finished {
        err.push_str(
            "\n(a background process still holds the output; what it prints later is not here)",
        );
    }
    if out_buf.overflowed() || err_buf.overflowed() {
        err.push_str(&format!(
            "\n(output past {} MB was dropped; redirect big output to a file)",
            PIPE_CAP >> 20
        ));
    }
    std::mem::forget(group);
    *live.0.stdin.lock().await = None;
    let secs = start.elapsed().as_secs_f64();
    let code = status.code().unwrap_or(-1);
    let info = if code == 0 {
        format!("✓ {secs:.1}s · {} lines", out.lines().count())
    } else {
        format!("exit {code} · {secs:.1}s")
    };
    let value = json!({
        "code": code,
        "out": clip(out.trim_end(), MAX_OUTPUT),
        "err": clip(err.trim_end(), MAX_OUTPUT),
    });
    // A failing command is still a successful call: the script decides.
    Ok((value, info))
}

/// Most bytes kept from one pipe: the model only ever sees MAX_OUTPUT of
/// it, so the rest only costs memory.
const PIPE_CAP: usize = 16 << 20;

/// Bytes read so far from a pipe, readable while the reader still runs, and
/// whether any were dropped at the cap.
#[derive(Clone, Default)]
struct Buffer(
    Arc<std::sync::Mutex<Vec<u8>>>,
    Arc<std::sync::atomic::AtomicBool>,
);

impl Buffer {
    fn len(&self) -> usize {
        self.0.lock().map_or(0, |bytes| bytes.len())
    }

    /// Everything after byte `from` (cut back to a character boundary).
    fn since(&self, from: usize) -> String {
        let bytes = self
            .0
            .lock()
            .map(|bytes| bytes.get(from..).unwrap_or_default().to_vec())
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The last `chars` so far, leaving them in place.
    fn tail(&self, chars: usize) -> String {
        let bytes = self
            .0
            .lock()
            .map(|bytes| bytes[bytes.len().saturating_sub(chars * 4)..].to_vec())
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_end();
        let start = text
            .char_indices()
            .rev()
            .nth(chars.saturating_sub(1))
            .map_or(0, |(at, _)| at);
        text[start..].to_string()
    }

    fn overflowed(&self) -> bool {
        self.1.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// All of it; kept, so a job can still hand out what was not read.
    fn text(&self) -> String {
        let bytes = self.0.lock().map(|bytes| bytes.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Copy a pipe into `buffer` chunk by chunk until EOF.
async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin, buffer: Buffer) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 8192];
    while let Ok(read) = pipe.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        if let Ok(mut bytes) = buffer.0.lock() {
            let room = PIPE_CAP.saturating_sub(bytes.len());
            if read > room {
                buffer.1.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            bytes.extend_from_slice(&chunk[..read.min(room)]);
        }
    }
}

/// Kills a command's whole process group when dropped: on timeout, on a
/// stopped run (the future is dropped), on error. Forgotten on normal exit so
/// intentional background jobs (`server &`) keep running.
struct Group(Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            signal_group(pid, "KILL");
        }
    }
}

/// Send SIG`signal` to process group `pgid`: plain `kill -- -PGID`, no libc.
fn signal_group(pgid: u32, signal: &str) {
    let _ = std::process::Command::new("kill")
        .args([&format!("-{signal}"), "--", &format!("-{pgid}")])
        .stderr(std::process::Stdio::null())
        .status();
}

/// The last `chars` characters of `text`.
fn last_chars(text: &str, chars: usize) -> &str {
    let start = text
        .char_indices()
        .rev()
        .nth(chars.saturating_sub(1))
        .map_or(0, |(at, _)| at);
    &text[start..]
}

/// `line` looks like a prompt waiting for an answer (Claude Code's list,
/// plus passwords).
fn asks(line: &str) -> bool {
    let line = line.trim().to_lowercase();
    if line.is_empty() || line.len() > 400 {
        return false;
    }
    const MARKS: [&str; 9] = [
        "(y/n)",
        "[y/n]",
        "yes/no",
        "(y/n/",
        "press any key",
        "press enter",
        "continue?",
        "overwrite?",
        "proceed?",
    ];
    if MARKS.iter().any(|mark| line.contains(mark))
        || ((line.contains("password") || line.contains("passphrase")) && line.ends_with(':'))
    {
        return true;
    }
    let word = |phrase: &str| {
        line.match_indices(phrase).any(|(at, _)| {
            !line[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                && !line[at + phrase.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
        })
    };
    line.ends_with('?')
        && ["do you", "would you", "shall i", "are you sure", "ready to"]
            .into_iter()
            .any(word)
}

/// Edit distance, for "did you mean" (names are short; O(n·m) is fine).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(prev + usize::from(ca != *cb));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// Why `old` didn't match, and where to look.
fn not_found(text: &str, old: &str, file: &str) -> String {
    if old.contains('\n') && text.contains("\r\n") != old.contains("\r\n") {
        let ends = |crlf: bool| if crlf { "CRLF (\\r\\n)" } else { "LF (\\n)" };
        return format!(
            "old text not found in {file}: the file's lines end in {} and old's in {}; match the file's line ends",
            ends(text.contains("\r\n")),
            ends(old.contains("\r\n"))
        );
    }
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if squash(text).contains(&squash(old)) {
        return format!(
            "old text not found in {file} as written, but it matches when whitespace is ignored: copy the exact indentation from read('{file}')"
        );
    }
    let first = old
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // Only a first line found once says where to look (a lone `}` would
    // point anywhere).
    let at: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !first.is_empty() && line.contains(first))
        .map(|(at, _)| at)
        .collect();
    match (at.len() == 1).then(|| at[0]) {
        Some(at) => format!(
            "old text not found in {file}; its first line is at line {}, but what follows differs. read('{file}', {{offset: {}, limit: 30}}) and copy exactly",
            at + 1,
            at.saturating_sub(2) + 1
        ),
        None => format!("old text not found in {file}; read the file first, it may have changed"),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep head and tail of long output, Tura-style: the model sees how much
/// was there and both ends, where errors and summaries live.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep = max / 2;
    let head: String = text.chars().take(keep).collect();
    let tail: String = {
        let mut tail: Vec<char> = text.chars().rev().take(keep).collect();
        tail.reverse();
        tail.into_iter().collect()
    };
    let cut = text.chars().count() - 2 * keep;
    format!(
        "Total output lines: {}\n\n{head}…{cut} characters truncated…{tail}",
        text.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn script(code: &str, dir: &Path) -> (Outcome, Vec<Call>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let outcome = run(
            code,
            dir,
            Limits {
                timeout: Duration::from_secs(5),
                ..Limits::default()
            },
            tx,
        )
        .await;
        let mut calls = Vec::new();
        while let Ok(call) = rx.try_recv() {
            if !call.running {
                calls.push(call);
            }
        }
        (outcome, calls)
    }

    struct Echo;

    impl Host for Echo {
        fn names(&self) -> &[&'static str] {
            &["agents.spawn", "ping"]
        }

        fn call<'a>(&'a self, name: &'a str, args: Vec<Value>) -> HostFuture<'a> {
            Box::pin(async move {
                match name {
                    "ping" => Err("pong refused".to_string()),
                    _ => Ok((json!({"id": "a1", "task": args[0]}), "a1".to_string())),
                }
            })
        }
    }

    fn png(dir: &Path, name: &str, width: u32, height: u32) {
        let mut bytes = Vec::new();
        ::image::DynamicImage::ImageRgb8(::image::RgbImage::new(width, height))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[tokio::test]
    async fn reading_an_image_shows_it() {
        let dir = temp();
        png(&dir, "a.png", 30, 20);
        png(&dir, "b.png", 8, 8);
        std::fs::write(dir.join("notes.txt"), "plain text").unwrap();

        let (outcome, calls) = script(
            "const [a, b] = await Promise.all([read('a.png'), read('b.png')]); await read('a.png'); return [a.width, a.height, b.mime, a.type]",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, r#"[30,20,"image/png","image"]"#);
        assert_eq!(
            outcome.images,
            [dir.join("a.png"), dir.join("b.png")],
            "each once, in order"
        );
        assert!(calls.iter().all(|c| c.info.starts_with('▣')), "{calls:?}");

        // An image value prints as one line, never a pile of fields.
        let (outcome, _) = script("return { a: await read('a.png') }", &dir).await;
        assert_eq!(outcome.text, r#"{"a":"[image a.png 30×20]"}"#);

        let (outcome, _) = script("return await read('notes.txt')", &dir).await;
        assert_eq!(outcome.text, "plain text");
        assert!(outcome.images.is_empty(), "text is not an image");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn at_most_max_shown_images_per_program() {
        let dir = temp();
        for i in 0..(MAX_SHOWN + 2) {
            png(&dir, &format!("{i}.png"), 4, 4);
        }
        let code = format!(
            "for (let i = 0; i < {}; i++) await read(i + '.png')",
            MAX_SHOWN + 2
        );
        let (outcome, _) = script(&code, &dir).await;
        assert_eq!(outcome.images.len(), MAX_SHOWN);
        assert!(
            outcome.text.contains("not shown:") && outcome.text.contains("11.png"),
            "{}",
            outcome.text
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn confined_writes_stay_inside_the_root() {
        let dir = temp();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let outcome = run_with(
            "await write('in/a.txt', 'x'); \
             let up; try { await write('../escape.txt', 'x') } catch (x) { up = x.message } \
             let abs; try { await write('/tmp/shroom-escape.txt', 'x') } catch (x) { abs = x.message } \
             await write('in/../b.txt', 'y'); return [up, abs]",
            &dir,
            Limits {
                timeout: Duration::from_secs(5),
                confine: true,
                ..Limits::default()
            },
            tx,
            None,
        )
        .await;
        assert!(outcome.error.is_none(), "{}", outcome.text);
        assert!(dir.join("in/a.txt").exists() && dir.join("b.txt").exists());
        assert_eq!(
            outcome.text.matches("outside your worktree").count(),
            2,
            "{}",
            outcome.text
        );
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn host_functions_and_read_only() {
        let dir = temp();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let outcome = run_with(
            "const a = await agents.spawn('look'); let e; try { await ping() } catch (x) { e = x.message } \
             let w; try { await write('f.txt', 'x') } catch (x) { w = x.message } \
             return [a.id, a.task, e, w, (await ls('.')).length]",
            &dir,
            Limits {
                timeout: Duration::from_secs(5),
                read_only: true,
                ..Limits::default()
            },
            tx,
            Some(Arc::new(Echo)),
        )
        .await;
        assert!(outcome.error.is_none(), "{}", outcome.text);
        assert!(outcome.text.contains(r#""a1","look""#), "{}", outcome.text);
        assert!(outcome.text.contains("pong refused"));
        assert!(outcome.text.contains("read-only"));
        assert!(!dir.join("f.txt").exists(), "write refused");
        let mut verbs = Vec::new();
        while let Ok(call) = rx.try_recv() {
            if !call.running {
                verbs.push((call.verb, call.target, call.ok));
            }
        }
        assert!(
            verbs.contains(&("agents.spawn".into(), "look".into(), true)),
            "{verbs:?}"
        );
        assert!(verbs.contains(&("ping".into(), String::new(), false)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shroom-js-{}-{}", std::process::id(), rand()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rand() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[tokio::test]
    async fn tools_round_trip() {
        let dir = temp();
        let (outcome, calls) = script(
            r#"// edit a file
            await write('a/b.txt', 'one\ntwo\nthree');
            await edit('a/b.txt', 'two', 'TWO');
            const [names, part] = await Promise.all([ls('a'), read('a/b.txt', { offset: 2, limit: 1 })]);
            const r = await sh('echo hi && exit 3');
            console.log(names, part);
            return { code: r.code, out: r.out };"#,
            &dir,
        )
        .await;
        assert_eq!(outcome.error, None, "{}", outcome.text);
        // Part of a file: its lines, then which part it is.
        assert_eq!(
            outcome.text,
            "[\"b.txt\"] TWO\n…[lines 2-2 of 3; read('a/b.txt', {offset: 3}) for more]\n{\"code\":3,\"out\":\"hi\"}"
        );
        assert_eq!(outcome.calls, 5);
        let verbs: Vec<_> = calls.iter().map(|c| (c.verb.as_str(), c.ok)).collect();
        assert_eq!(verbs[..2], [("write", true), ("edit", true)]);
        assert!(
            calls
                .iter()
                .any(|c| c.verb == "sh" && c.info.starts_with("exit 3"))
        );
        assert_eq!(title("  // edit a file\nx").as_deref(), Some("edit a file"));
        assert_eq!(
            title("const [c, t] = await Promise.all(["),
            None,
            "code is not a title"
        );
        assert_eq!(title("//\nx"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn errors_stop_the_script_and_keep_output() {
        let dir = temp();
        let (outcome, calls) = script(
            "console.log('before'); await read('missing.txt'); console.log('after');",
            &dir,
        )
        .await;
        assert!(outcome.error.is_some());
        assert!(
            outcome.text.starts_with("before\nerror: "),
            "{}",
            outcome.text
        );
        assert!(!outcome.text.contains("after"));
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].ok);
        let (edit, _) = script("await write('x', 'a a'); await edit('x', 'a', 'b');", &dir).await;
        assert!(edit.text.contains("matches 2 times"), "{}", edit.text);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn sandbox_and_limits() {
        let dir = temp();
        let (outcome, _) = script(
            "return [typeof require, typeof process, typeof std, typeof os].join(',')",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, "undefined,undefined,undefined,undefined");
        let (syntax, _) = script("return (", &dir).await;
        assert!(syntax.error.is_some());
        let (looping, _) = script_with("while (true) {}", &dir, Duration::from_millis(200)).await;
        assert!(looping.error.is_some(), "{}", looping.text);
        let (empty, _) = script("await ls('.')", &dir).await;
        assert!(empty.text.contains("no output"), "{}", empty.text);
        std::fs::remove_dir_all(dir).unwrap();
    }

    async fn script_with(code: &str, dir: &Path, timeout: Duration) -> (Outcome, Vec<Call>) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        (
            run(
                code,
                dir,
                Limits {
                    timeout,
                    ..Limits::default()
                },
                tx,
            )
            .await,
            Vec::new(),
        )
    }

    #[tokio::test]
    async fn parallel_calls_overlap() {
        let dir = temp();
        let started = Instant::now();
        let (outcome, _) = script(
            "await Promise.all([sh('sleep 0.3'), sh('sleep 0.3'), sh('sleep 0.3')]); return 'ok'",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, "ok");
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "{:?}",
            started.elapsed()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn background_jobs_dont_hang_and_timeouts_kill_the_group() {
        let dir = temp();
        let started = Instant::now();
        let (outcome, _) = script("return (await sh('sleep 30 & echo started')).out", &dir).await;
        assert_eq!(outcome.text, "started", "{}", outcome.text);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let _ = std::process::Command::new("pkill")
            .args(["-f", "sleep 30"])
            .status();

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn calls_are_async_by_default() {
        let dir = temp();
        // Never awaited: still runs, still finishes before the program ends.
        let (outcome, calls) = script(
            "write('a.txt', 'x'); write('b.txt', 'y'); return 'started'",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, "started");
        assert_eq!(calls.len(), 2, "unawaited calls finished");
        assert!(dir.join("b.txt").exists());
        // Returning calls without await resolves them, inside arrays/objects too.
        let (outcome, _) = script(
            "return { a: read('a.txt'), both: [read('a.txt'), read('b.txt')] }",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, r#"{"a":"x","both":["x","y"]}"#);
        // A failing call nobody awaited is reported, not lost.
        let (outcome, _) = script("read('nope.txt'); return 'done'", &dir).await;
        assert!(
            outcome
                .text
                .contains("not awaited and failed: read(\"nope.txt\")"),
            "{}",
            outcome.text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn errors_point_at_the_line_and_list_what_ran() {
        let dir = temp();
        let (syntax, _) = script("const a = 1;\nconst b = (;\nreturn a", &dir).await;
        assert!(
            syntax.text.contains("at line 2") && syntax.text.contains("2 | const b = (;"),
            "{}",
            syntax.text
        );
        let (thrown, _) = script(
            "await write('w.txt', 'hi');\nconst x = null;\nreturn x.length",
            &dir,
        )
        .await;
        assert!(
            thrown.text.contains("at line 3: TypeError"),
            "{}",
            thrown.text
        );
        assert!(
            thrown.text.contains("✓ write w.txt"),
            "footer lists finished calls: {}",
            thrown.text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn tool_misuse_gets_useful_answers() {
        let dir = temp();
        std::fs::write(dir.join("config.toml"), "a = 1\n  b = 2\n").unwrap();
        std::fs::write(dir.join("blob"), [0u8, 1, 2, 3]).unwrap();
        let long: String = (1..=2500).map(|n| format!("{n}\n")).collect();
        std::fs::write(dir.join("long.txt"), long).unwrap();
        let err = |code: &'static str| {
            let dir = dir.clone();
            async move { script(code, &dir).await.0.text }
        };
        assert!(
            err("return await read('confg.toml')")
                .await
                .contains("similar here: config.toml")
        );
        assert!(
            err("return await read('.')")
                .await
                .contains("is a directory; use ls")
        );
        assert!(err("return await read('blob')").await.contains("is binary"));
        assert!(
            err("await edit('config.toml', 'a = 1\\nb = 2', 'x')")
                .await
                .contains("whitespace is ignored")
        );
        assert!(
            err("await edit('config.toml', 'a = 1\\nzzz', 'x')")
                .await
                .contains("first line is at line 1")
        );
        assert!(
            err("await edit('config.toml', 'a', 'a')")
                .await
                .contains("identical")
        );
        let page = err("return await read('long.txt')").await;
        assert!(
            page.contains("lines 1-2000 of 2500") && page.contains("offset: 2001"),
            "{}",
            &page[page.len() - 120..]
        );
        // A paged read written back would cut the file short: refused, and
        // the file is left whole.
        let cut = err(
            "const t = await read('long.txt'); await write('long.txt', t.replace('1\\n', 'one\\n'))",
        )
        .await;
        assert!(
            cut.contains("not written") && cut.contains("part note"),
            "{cut}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("long.txt"))
                .unwrap()
                .lines()
                .count(),
            2500
        );
        let clipped = clip(&"x\n".repeat(20_000), MAX_OUTPUT);
        assert!(cut_note(&clipped).is_some());
        // Built here so this file never holds a whole note itself.
        let saved = format!("…[full output ({}0 characters) in /tmp/x.txt]…", 1200);
        assert!(cut_note(&saved).is_some());
        // The code that formats the notes, and ordinary ellipses, pass.
        assert_eq!(
            cut_note("\"…[lines {offset}-{end} of {total}; read('{file}') for more]\""),
            None
        );
        assert_eq!(cut_note("wait… 3 more…"), None);
        // Object-style arguments and argv arrays.
        assert_eq!(
            err("return await read({path: 'config.toml', offset: 2, limit: 1})").await,
            "  b = 2\n…[lines 2-2 of 2]"
        );
        assert_eq!(err("return (await sh(['echo', 'a b'])).out").await, "a b");
        assert!(
            err("await sh('sleep 5', {timeout: 200})")
                .await
                .contains("jobs.wait('j1')")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn file_tools_never_lose_data() {
        let dir = temp();
        let run = |code: String| {
            let dir = dir.clone();
            async move { script(&code, &dir).await.0.text }
        };
        let long: String = (1..=300).map(|n| format!("{n}\n")).collect();
        std::fs::write(dir.join("long.txt"), &long).unwrap();

        // A ranged read is marked as part of the file, so it cannot be
        // written back over it; the whole file comes back exactly.
        let part = run(
            "const t = await read('long.txt', {offset: 1, limit: 100}); await write('long.txt', t)"
                .into(),
        )
        .await;
        assert!(
            part.contains("not written") && part.contains("part note"),
            "{part}"
        );
        assert_eq!(std::fs::read_to_string(dir.join("long.txt")).unwrap(), long);
        assert_eq!(
            run("return await read('long.txt', {limit: 5000})".into()).await,
            long
        );

        // Missing text is an error, never an empty file or a deletion.
        for code in [
            "await write('long.txt')",
            "await write({path: 'long.txt', code: 'x'})",
            "await edit('long.txt', '1\\n')",
        ] {
            let out = run(code.into()).await;
            assert!(out.contains("Nothing was"), "{code}: {out}");
        }
        let empty = run("await write('long.txt', '')".into()).await;
        assert!(empty.contains("would empty"), "{empty}");
        assert_eq!(std::fs::read_to_string(dir.join("long.txt")).unwrap(), long);
        run("await write('long.txt', '', {force: true})".into()).await;
        assert_eq!(std::fs::read_to_string(dir.join("long.txt")).unwrap(), "");

        // Not UTF-8: refused, not read with its bytes replaced.
        std::fs::write(dir.join("latin1.txt"), b"caf\xe9\n").unwrap();
        let latin = run("return await read('latin1.txt')".into()).await;
        assert!(latin.contains("not UTF-8"), "{latin}");
        let latin = run("await edit('latin1.txt', 'caf', 'cafe')".into()).await;
        assert!(
            latin.contains("not UTF-8") && !latin.contains("no such file"),
            "{latin}"
        );
        assert_eq!(std::fs::read(dir.join("latin1.txt")).unwrap(), b"caf\xe9\n");

        // CRLF: a ranged read keeps the line ends; an edit written with \n
        // lands with the file's own.
        std::fs::write(dir.join("crlf.txt"), "a\r\nb\r\nc\r\n").unwrap();
        let part =
            run("return JSON.stringify(await read('crlf.txt', {offset: 2, limit: 1}))".into())
                .await;
        assert!(part.starts_with("\"b\\r\\n…[lines 2-2 of 3"), "{part}");
        run("await edit('crlf.txt', 'a\\nb', 'a\\nB\\nb')".into()).await;
        assert_eq!(
            std::fs::read_to_string(dir.join("crlf.txt")).unwrap(),
            "a\r\nB\r\nb\r\nc\r\n"
        );

        // Writes keep the mode and write through a symlink.
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(dir.join("run.sh"), "echo a\n").unwrap();
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::os::unix::fs::symlink("run.sh", dir.join("link.sh")).unwrap();
        run("await edit('link.sh', 'echo a', 'echo b')".into()).await;
        assert!(
            std::fs::symlink_metadata(dir.join("link.sh"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("run.sh")).unwrap(),
            "echo b\n"
        );
        assert_eq!(
            std::fs::metadata(dir.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        // No temporary files are left behind.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains(".shroom-")), "{names:?}");

        // A permission error says so instead of "no such file".
        std::fs::write(dir.join("locked.txt"), "x").unwrap();
        std::fs::set_permissions(
            dir.join("locked.txt"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        if std::fs::read(dir.join("locked.txt")).is_err() {
            let locked = run("return await read('locked.txt')".into()).await;
            assert!(
                locked.contains("cannot open") && !locked.contains("no such"),
                "{locked}"
            );
        }
        std::fs::set_permissions(
            dir.join("locked.txt"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        // Every note that marks cut text is caught.
        let cut = format!("a\n…[{} characters cut]…\nb", 120);
        assert!(cut_note(&cut).is_some());
        let part = format!("x\n…[lines {}-{} of {}]", 2, 3, 9);
        assert!(cut_note(&part).is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_symlink_cannot_carry_a_confined_write_out() {
        let dir = temp();
        let outside = temp();
        std::os::unix::fs::symlink(&outside, dir.join("out")).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let outcome = run_with(
            "let e; try { await write('out/x.txt', 'x') } catch (x) { e = x.message } return e",
            &dir,
            Limits {
                timeout: Duration::from_secs(5),
                confine: true,
                ..Limits::default()
            },
            tx,
            None,
        )
        .await;
        assert!(
            outcome.text.contains("outside your worktree"),
            "{}",
            outcome.text
        );
        assert!(!outside.join("x.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn slow_calls_become_jobs() {
        let dir = temp();
        // Past its timeout the call fails (naming the job, with the output so
        // far) and keeps running; jobs.wait gets its result.
        let (outcome, calls) = script(
            "let e; try { await sh('echo started; sleep 1; echo end', {timeout: 400}) } catch (x) { e = x.message }
            const now = await jobs.list();
            const r = await jobs.wait('j1');
            return [e, now, r]",
            &dir,
        )
        .await;
        let text = &outcome.text;
        assert_eq!(outcome.error, None, "{text}");
        assert!(
            text.contains("job j1") && text.contains("started"),
            "{text}"
        );
        assert!(text.contains(r#""status":"running""#), "{text}");
        assert!(
            text.contains(r#""status":"done""#) && text.contains(r#""out":"started\nend""#),
            "{text}"
        );
        assert!(
            calls
                .iter()
                .any(|c| !c.running && !c.ok && c.info.contains("job j1"))
        );

        // Jobs outlive the program that started them (one Jobs per
        // conversation), and stop kills the command's whole group.
        let jobs = Arc::new(Jobs::default());
        let marker = format!("shroom-job-{}", std::process::id());
        let limits = || Limits {
            timeout: Duration::from_secs(5),
            jobs: jobs.clone(),
            ..Limits::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let first = run_with(
            &format!(
                "await sh('bash -c \"sleep 40; echo {marker}\" & sleep 40', {{timeout: 300}})"
            ),
            &dir,
            limits(),
            tx.clone(),
            None,
        )
        .await;
        assert!(first.text.contains("job j1"), "{}", first.text);
        let alive = || {
            !std::process::Command::new("pgrep")
                .args(["-f", &marker])
                .output()
                .unwrap()
                .stdout
                .is_empty()
        };
        assert!(alive(), "a job keeps running after its program ends");
        let second = run_with(
            "const s = await jobs.stop('j1'); return [s.status, (await jobs.wait('j1')).status]",
            &dir,
            limits(),
            tx.clone(),
            None,
        )
        .await;
        assert_eq!(second.text, r#"["stopped","stopped"]"#);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(), "stopping a job kills its process group");

        // A wrong id says which exist; a quick call is untouched.
        let third = run_with(
            "let e; try { await jobs.wait('j9') } catch (x) { e = x.message } return e",
            &dir,
            limits(),
            tx,
            None,
        )
        .await;
        assert!(third.text.contains("j1"), "{}", third.text);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn jobs_left_alone_send_notes() {
        let dir = temp();
        let jobs = Arc::new(Jobs::default());
        jobs.stall(Duration::from_millis(400));
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = woken.clone();
        jobs.notify(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let limits = || Limits {
            timeout: Duration::from_secs(5),
            jobs: jobs.clone(),
            ..Limits::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let go = |code: &'static str| run_with(code, &dir, limits(), tx.clone(), None);

        // Left alone, its end becomes a note (once), with the exit code and
        // the end of its output; the timeout error says a note will come.
        let first = go("try { await sh('sleep 0.6; echo built; exit 3', {timeout: 100}) } catch (e) { return e.message }").await;
        assert!(
            first.text.contains("You get a message when it ends"),
            "{}",
            first.text
        );
        assert!(first.text.contains("printed nothing yet"), "{}", first.text);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        assert!(jobs.has_pending());
        let notes = jobs.pending();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let note = &notes[0];
        for part in [
            "<id>j1</id>",
            "<status>done</status>",
            "<code>3</code>",
            "exited with code 3",
            "built",
        ] {
            assert!(note.contains(part), "{part}: {note}");
        }
        assert!(jobs.pending().is_empty(), "a note is delivered once");

        // A job the model waits on, or stops, sends none.
        let waited = go("try { await sh('sleep 0.3', {timeout: 50}) } catch {} return (await jobs.wait('j2')).status").await;
        assert_eq!(waited.text, "done");
        let stopped = go("try { await sh('sleep 30', {timeout: 50}) } catch {} return (await jobs.stop('j3')).status").await;
        assert_eq!(stopped.text, "stopped");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(jobs.pending().is_empty(), "{:?}", jobs.pending());

        // A prompt it waits at: jobs.wait returns early with the line, and
        // left alone it is a note that says how to answer.
        let asked = go("try { await sh('printf \"Overwrite file? [y/N] \"; read a; echo got $a', {interactive: true, timeout: 100}) } catch {}
            const w = await jobs.wait('j4', {timeout: 5000});
            return [w.status, w.waiting]").await;
        assert_eq!(asked.text, r#"["running","Overwrite file? [y/N]"]"#);
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(
            jobs.pending().is_empty(),
            "the model saw that prompt in its wait"
        );
        let left = go("try { await sh('sleep 0.2; echo -n \"Are you sure you want to go on? \"; read a', {interactive: true, timeout: 50}) } catch {} return 1").await;
        assert_eq!(left.error, None, "{}", left.text);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let notes = jobs.pending();
        assert!(
            notes.len() == 1
                && notes[0].contains("<status>waiting</status>")
                && notes[0].contains("jobs.send('j5'"),
            "{notes:?}"
        );
        // Answered, both end; the answers are seen, so no notes.
        let done = go("await jobs.send('j4', 'y\\n'); await jobs.send('j5', 'y\\n'); return [(await jobs.wait('j4')).value.out, (await jobs.wait('j5')).status]").await;
        assert_eq!(done.text, r#"["Overwrite file? [y/N] got y","done"]"#);
        assert!(jobs.pending().is_empty(), "{:?}", jobs.pending());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prompts_are_recognized() {
        for line in [
            "Overwrite existing file? [y/N]",
            "Do you want to continue? (Y/n)",
            "Are you sure you want to delete it?",
            "Press Enter to continue",
            "[sudo] password for fox:",
            "Enter passphrase for key '/home/fox/.ssh/id_ed25519':",
            "Proceed? ",
        ] {
            assert!(asks(line), "{line}");
        }
        for line in [
            "Compiling shroom v0.1.0",
            "is it done?",
            "Undo youth?",
            "password: hunter2",
            "",
        ] {
            assert!(!asks(line), "{line}");
        }
    }

    #[tokio::test]
    async fn commands_take_input_and_sessions_take_more() {
        let dir = temp();
        // Input once, on stdin; without it stdin is empty (rg-style reads of
        // a pipe end at once instead of hanging).
        let (outcome, _) = script(
            "return [(await sh('tr a-z A-Z', {input: 'hi\\n'})).out, (await sh('cat')).out]",
            &dir,
        )
        .await;
        assert_eq!(outcome.text, r#"["HI",""]"#, "{}", outcome.text);

        // A session: a job at once, answers with its banner; each send gets
        // what came back since; closing its input ends it.
        let (outcome, calls) = script(
            "const s = await sh('echo ready; while read l; do echo got:$l; done; echo bye', {interactive: true});
            const a = await jobs.send(s.id, 'one\\n');
            const b = await jobs.send(s.id, 'two\\nthree\\n');
            const q = await jobs.read(s.id, {wait: 300});
            const c = await jobs.send(s.id, '', {close: true});
            const w = await jobs.wait(s.id);
            let late; try { await jobs.send(s.id, 'x\\n') } catch (e) { late = e.message }
            return [s.id, s.status, s.out, a.out, b.out, q.out, c.out, w.status, w.value.code, late]",
            &dir,
        )
        .await;
        assert_eq!(outcome.error, None, "{}", outcome.text);
        let got: Value = serde_json::from_str(&outcome.text).unwrap();
        assert_eq!(got[0], "j1");
        assert_eq!(got[1], "running");
        assert_eq!(got[2], "ready");
        assert_eq!(got[3], "got:one");
        assert_eq!(got[4], "got:two\ngot:three");
        assert_eq!(got[5], "");
        assert_eq!(got[6], "bye");
        assert_eq!(got[7], "done");
        assert_eq!(got[8], 0);
        assert!(got[9].as_str().unwrap().contains("has ended"), "{}", got[9]);
        assert!(
            calls
                .iter()
                .any(|c| c.verb == "jobs.send" && c.target == "j1 ← one\\n")
        );

        // A call that became a job by timing out has no open input.
        let (outcome, _) = script(
            "try { await sh('sleep 2', {timeout: 200}) } catch {} let e; try { await jobs.send('j1', 'x') } catch (x) { e = x.message } await jobs.stop('j1'); return e",
            &dir,
        )
        .await;
        assert!(
            outcome.text.contains("interactive: true"),
            "{}",
            outcome.text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn calls_on_one_file_keep_their_order() {
        let dir = temp();
        std::fs::write(
            dir.join("f.txt"),
            "a
",
        )
        .unwrap();
        // Never awaited, made in this order: they land in this order.
        let (outcome, _) = script(
            "edit('f.txt', 'a', 'b'); edit('f.txt', 'b', 'c'); const t = read('f.txt'); edit('f.txt', 'c', 'd'); return await t",
            &dir,
        )
        .await;
        assert_eq!(
            outcome.text,
            "c
",
            "{}",
            outcome.text
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "d
"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clip_keeps_both_ends() {
        let text = format!("{}END", "x".repeat(50));
        let clipped = clip(&text, 10);
        assert!(clipped.starts_with("Total output lines: 1"));
        assert!(clipped.ends_with("xxEND"));
        assert!(clipped.contains("43 characters truncated"));
    }
}
