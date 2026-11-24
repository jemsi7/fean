//! `fean`: command line of the Rust engine (docs/engine.md section 2; Map unit I-09-e).
//!
//!     fean compile model.fean [-o model.feanc]    model text -> image file
//!     fean run     model.fean | model.feanc       print the query results
//!     fean dump    model.feanc                    readable listing of an image
//!     fean check   model.fean                     parse and check only
//!     fean inspect model.fean                     everything the IDE shows, as JSON
//!     fean batch                                  jobs on standard input (used by the tests)
//!
//! Exit codes, as in the Python engine: 0 success, 1 compile error (syntax, static rule, bad image),
//! 2 model error while running, 3 unreadable input or wrong usage. A source of `-` is standard input.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fean_core::image::{self, Image, NONE};
use fean_core::session::Session;
use fean_core::vm::{Step, Vm, DEFAULT_BUDGET};
use fean_core::{compile, frontend, with_stack, FeanError, Kind};
use serde_json::{json, Value};

const USAGE: &str = "usage: fean {compile,run,dump,check,inspect,batch} ...";

const HELP: &str = "usage: fean {compile,run,dump,check,inspect,batch} ...

FEAN 엔진: 모델 글을 이미지로 컴파일하고 실행한다.

  compile SOURCE [-o OUTPUT] [--no-prelude]         모델 글 → 이미지 파일(.feanc)
  run PATH [--json] [--budget N] [--no-prelude]     모델 글이나 이미지를 실행해 질의 결과를 출력
  dump PATH                                         이미지의 목록을 출력
  check SOURCE [--no-prelude]                       모델 글의 구문과 규칙만 검사
  inspect PATH [--views] [--budget N] [--no-prelude]  IDE가 보여 주는 자료를 JSON으로 출력
  batch                                             표준입력의 작업 목록(JSON)을 수행

  --no-prelude   표준 계약을 앞에 붙이지 않는다
  --budget N     관찰 지점 수의 한도 (기본 200000)
  --views        inspect: 걸음마다 각 주체의 지식도 함께 출력
  SOURCE나 PATH가 - 이면 표준입력을 읽는다.

종료 코드: 0 성공, 1 컴파일 오류, 2 실행 중 모델 오류, 3 읽을 수 없는 입력이나 잘못된 사용법.";

/// What can stop a command: an engine error, an unreadable input, or wrong usage.
enum Stop {
    Fean(FeanError),
    Io(String),
    NotUtf8,
    Usage(String),
}

impl From<FeanError> for Stop {
    fn from(e: FeanError) -> Stop {
        Stop::Fean(e)
    }
}

impl From<std::io::Error> for Stop {
    fn from(e: std::io::Error) -> Stop {
        // the operating system's words, as Python's `strerror` gives them
        let text = e.to_string();
        let text = match text.rfind(" (os error ") {
            Some(at) => text[..at].to_string(),
            None => text,
        };
        Stop::Io(text)
    }
}

#[derive(Default)]
struct Args {
    path: Option<String>,
    output: Option<String>,
    json: bool,
    views: bool,
    no_prelude: bool,
    budget: Option<usize>,
}

fn parse(command: &str, rest: &[String]) -> Result<Args, Stop> {
    let allowed: &[&str] = match command {
        "compile" => &["output", "no-prelude"],
        "run" => &["json", "budget", "no-prelude"],
        "check" => &["no-prelude"],
        "inspect" => &["budget", "no-prelude", "views"],
        _ => &[],
    };
    let mut args = Args::default();
    let mut plain = false;
    let mut i = 0;
    let allow = |name: &str| -> Result<(), Stop> {
        if allowed.contains(&name) {
            Ok(())
        } else {
            Err(Stop::Usage(format!("fean {command}: unrecognized arguments: --{name}")))
        }
    };
    let budget = |text: &str| -> Result<usize, Stop> {
        match text.parse::<i64>() {
            Ok(n) => Ok(n.max(0) as usize),
            Err(_) => Err(Stop::Usage(format!("fean {command}: argument --budget: invalid int value: '{text}'"))),
        }
    };
    while i < rest.len() {
        let a = rest[i].as_str();
        i += 1;
        let mut value = |name: &str| -> Result<String, Stop> {
            if i < rest.len() {
                i += 1;
                Ok(rest[i - 1].clone())
            } else {
                Err(Stop::Usage(format!("fean {command}: argument {name}: expected one argument")))
            }
        };
        if plain || a == "-" || !a.starts_with('-') {
            if args.path.is_some() {
                return Err(Stop::Usage(format!("fean {command}: unrecognized arguments: {a}")));
            }
            args.path = Some(a.to_string());
        } else if a == "--" {
            plain = true;
        } else if a == "-h" || a == "--help" {
            return Err(Stop::Usage(String::new()));
        } else if a == "--no-prelude" {
            allow("no-prelude")?;
            args.no_prelude = true;
        } else if a == "--json" {
            allow("json")?;
            args.json = true;
        } else if a == "--views" {
            allow("views")?;
            args.views = true;
        } else if a == "--budget" {
            allow("budget")?;
            args.budget = Some(budget(&value("--budget")?)?);
        } else if let Some(text) = a.strip_prefix("--budget=") {
            allow("budget")?;
            args.budget = Some(budget(text)?);
        } else if a == "-o" || a == "--output" {
            allow("output")?;
            args.output = Some(value("-o/--output")?);
        } else if let Some(text) = a.strip_prefix("--output=") {
            allow("output")?;
            args.output = Some(text.to_string());
        } else if a.starts_with("-o") && !a.starts_with("--") {
            allow("output")?;
            args.output = Some(a[2..].to_string());
        } else {
            return Err(Stop::Usage(format!("fean {command}: unrecognized arguments: {a}")));
        }
    }
    if args.path.is_none() {
        return Err(Stop::Usage(format!("fean {command}: the following arguments are required: path")));
    }
    Ok(args)
}

/// Read a text the way Python's text mode does: UTF-8, with every line ending turned into `\n`.
fn read(path: &str) -> Result<String, Stop> {
    let bytes = if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        std::fs::read(path)?
    };
    let text = String::from_utf8(bytes).map_err(|_| Stop::NotUtf8)?;
    Ok(if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text })
}

fn shown(path: &str) -> &str {
    if path == "-" {
        "<stdin>"
    } else {
        path
    }
}

/// A path written the way Python's `pathlib` prints it (`./a//b` -> `a/b`).
fn tidy(path: &Path) -> String {
    let text = path.to_string_lossy();
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let joined = parts.join("/");
    if text.starts_with('/') {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

fn prelude(args: &Args) -> Option<&'static str> {
    if args.no_prelude {
        Some("")
    } else {
        None
    }
}

fn cmd_compile(args: &Args) -> Result<(), Stop> {
    let source = args.path.as_deref().unwrap();
    if source == "-" && args.output.is_none() {
        eprintln!("fean compile: 표준입력을 읽을 때에는 -o 로 출력 파일을 정해야 합니다");
        return Err(Stop::Usage("-".into()));
    }
    let compiled = compile(&read(source)?, prelude(args))?;
    let out: PathBuf = match &args.output {
        Some(o) => PathBuf::from(o),
        None => Path::new(source).with_extension("feanc"),
    };
    let mut tmp_name = out.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = out.with_file_name(tmp_name);
    std::fs::write(&tmp, image::dumps(&compiled.image))?;
    std::fs::rename(&tmp, &out)?; // the image appears whole or not at all
    let im = &compiled.image;
    let count = |k: &str| im[k].as_array().map(|a| a.len()).unwrap_or(0);
    println!(
        "{} → {}  (셀 {}, 사건 {}, 코드 블록 {}, 질의 {})",
        shown(source),
        tidy(&out),
        count("cells"),
        count("events"),
        count("codes"),
        count("queries")
    );
    Ok(())
}

fn load(args: &Args) -> Result<Value, Stop> {
    let path = args.path.as_deref().unwrap();
    let text = read(path)?;
    Ok(if path.ends_with(".feanc") { image::loads(&text)? } else { compile(&text, prelude(args))?.image })
}

fn quoted(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn cmd_run(args: &Args) -> Result<(), Stop> {
    let image = load(args)?;
    let results = Session::new(&image, args.budget.unwrap_or(DEFAULT_BUDGET))?.results()?;
    let mut out = String::new();
    if args.json {
        if results.is_empty() {
            out.push_str("{}\n");
        } else {
            let rows: Vec<String> = results.iter().map(|(k, v)| format!("  {}: {}", quoted(k), quoted(v))).collect();
            out.push_str(&format!("{{\n{}\n}}\n", rows.join(",\n")));
        }
    } else {
        let width = results.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
        for (name, value) in &results {
            out.push_str(&format!("{name:width$} = {value}\n"));
        }
    }
    std::io::stdout().write_all(out.as_bytes())?;
    Ok(())
}

fn cmd_dump(args: &Args) -> Result<(), Stop> {
    let text = read(args.path.as_deref().unwrap())?;
    let listing = image::listing(&image::loads(&text)?);
    std::io::stdout().write_all(listing.as_bytes())?;
    Ok(())
}

fn cmd_check(args: &Args) -> Result<(), Stop> {
    let source = args.path.as_deref().unwrap();
    let spec = frontend(&read(source)?, prelude(args))?;
    println!("{}: 검사 통과 (셀 {}, 사건 {}, 질의 {})", shown(source), spec.cells.len(), spec.events.len(), spec.queries.len());
    Ok(())
}

fn cmd_inspect(args: &Args) -> Result<(), Stop> {
    let source = args.path.as_deref().unwrap();
    let text = read(source)?;
    let report = fean_core::trace::inspect(&text, prelude(args), args.budget.unwrap_or(DEFAULT_BUDGET), args.views);
    println!("{report}");
    Ok(())
}

// ---- batch: many jobs in one process, for the cross-checks against the Python engine -----------

fn error(e: &FeanError) -> Value {
    json!({"ok": false, "kind": e.kind.name(), "code": e.code, "line": e.line, "message": e.message})
}

fn plain(v: i64) -> Value {
    if v == NONE {
        Value::Null
    } else {
        json!(v)
    }
}

fn step_json(im: &Image, step: &Step) -> Value {
    match step {
        Step::Init(w) => json!(["init", w]),
        Step::End => json!(["end"]),
        Step::Tick(t) => json!(["tick", t]),
        Step::Commit(u) => json!(["commit", im.units[*u].name]),
        Step::Reject(u, why) => json!(["reject", im.units[*u].name, why]),
        Step::Fail(u, status, why) => json!(["fail", im.units[*u].name, status.name(), why]),
        Step::Branch(j) => json!(["branch", j]),
        Step::Show(who, menu) => json!(["show", who, menu]),
        Step::Pick(who, menu, action) => json!(["pick", who, menu, action]),
        Step::Mark(m) => json!(["mark", m]),
    }
}

fn history(vm: &Vm) -> Value {
    let steps: Vec<Value> = vm.nodes.iter().map(|n| step_json(&vm.im, &n.step)).collect();
    let leaves: Vec<Value> = vm
        .leaves
        .iter()
        .map(|&l| {
            let n = &vm.nodes[l as usize];
            let cells: Vec<Value> = n.state.cells.iter().map(|&v| plain(v)).collect();
            let mem: Vec<Value> = n
                .state
                .mem
                .iter()
                .map(|slot| match slot {
                    Some((v, asof, recv)) => json!([plain(*v), plain(*asof), recv]),
                    None => Value::Null,
                })
                .collect();
            json!([n.world, n.failed().map(|f| f.name()), n.state.t, cells, mem])
        })
        .collect();
    json!({"worlds": vm.worlds.len(), "steps": steps, "leaves": leaves})
}

fn histories(image: &Value, budget: usize) -> Result<Value, FeanError> {
    let mut session = Session::new(image, budget)?;
    let mut rows = Vec::new();
    for pv in session.assignments() {
        let row = match session.vm(&pv).and_then(|vm| vm.explore().map(|()| history(vm))) {
            Ok(mut h) => {
                h["pvals"] = json!(pv);
                h
            }
            Err(e) => json!({"pvals": pv, "error": error(&e)}),
        };
        rows.push(row);
    }
    let results = match session.results() {
        Ok(r) => json!(r),
        Err(e) => error(&e),
    };
    Ok(json!({"ok": true, "assignments": rows, "results": results}))
}

/// Every solve of an image under every assignment: its answer, its regions, and the verdict of
/// the solver at each point the job asks about (`points`: {solve name: [[pvals, [point, ...]], ...]}).
fn solves(image: &Value, budget: usize, runs: usize, points: &Value) -> Result<Value, FeanError> {
    let session = Session::new(image, budget)?;
    let im = session.image.clone();
    let mut out = Vec::new();
    for (index, sv) in im.solves.iter().enumerate() {
        let mut rows = Vec::new();
        for pv in session.assignments() {
            let solved = fean_core::solve::solve(&im, index, &pv, budget, runs);
            let asked: Vec<Vec<i64>> = points[&sv.name]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row[0] == json!(pv)))
                .and_then(|row| row[1].as_array())
                .map(|list| list.iter().map(|p| p.as_array().map(|c| c.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()).collect())
                .unwrap_or_default();
            let verdicts: Vec<Value> = asked
                .iter()
                .map(|p| match solved.member(p) {
                    fean_core::solve::Verdict::Invalid(code) => json!(["invalid", code]),
                    other => json!([other.word()]),
                })
                .collect();
            let regions: Vec<Value> = solved
                .leaves
                .iter()
                .map(|l| json!({"verdict": l.verdict.word(), "text": solved.region_text(&l.region), "sample": l.sample[..solved.vars.len().min(l.sample.len())]}))
                .collect();
            rows.push(json!({
                "pvals": pv, "text": solved.text, "runs": solved.runs, "complete": solved.complete, "unknown": solved.unknown,
                "best": solved.best, "value": solved.value.map(|v| v.to_string()), "counter": solved.counter, "error": solved.error,
                "vars": solved.vars.iter().map(|v| v.name.clone()).collect::<Vec<_>>(), "verdicts": verdicts, "regions": regions,
            }));
        }
        out.push(json!({"name": sv.name, "form": sv.form, "rows": rows}));
    }
    Ok(json!({"ok": true, "solves": out}))
}

fn batch() -> Result<(), Stop> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    let jobs: Value = serde_json::from_str(&text).map_err(|e| Stop::Usage(format!("fean batch: the jobs must be JSON ({e})")))?;
    let mut out = Vec::new();
    for job in jobs["jobs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let src = job["src"].as_str().unwrap_or("");
        let prelude = job["prelude"].as_str();
        let budget = job["budget"].as_u64().map(|b| b as usize).unwrap_or(DEFAULT_BUDGET);
        let done: Result<Value, FeanError> = match job["op"].as_str().unwrap_or("") {
            "check" => frontend(src, prelude).map(|_| json!({"ok": true})),
            "compile" => compile(src, prelude)
                .map(|c| json!({"ok": true, "image": image::dumps(&c.image), "listing": image::listing(&c.image)})),
            "verify" => image::verify(&job["image"]).map(|()| json!({"ok": true})),
            "loads" => image::loads(job["text"].as_str().unwrap_or("")).map(|_| json!({"ok": true})),
            "run" => compile(src, prelude)
                .and_then(|c| Session::new(&c.image, budget)?.results())
                .map(|r| json!({"ok": true, "results": r})),
            "histories" => compile(src, prelude).and_then(|c| histories(&c.image, budget)),
            "solve" => {
                let runs = job["runs"].as_u64().map(|n| n as usize).unwrap_or(fean_core::solve::RUN_LIMIT);
                compile(src, prelude).and_then(|c| solves(&c.image, budget, runs, &job["points"]))
            }
            "inspect" => {
                let views = job["views"].as_bool().unwrap_or(false);
                Ok(json!({"ok": true, "report": fean_core::trace::inspect(src, prelude, budget, views)}))
            }
            // the history views after a candidate of a solve was picked to be looked at
            "case" => compile(src, prelude).and_then(|c| {
                let mut analysis = fean_core::trace::Analysis::new(&c, budget)?;
                let point: Vec<i64> = job["point"].as_array().map(|p| p.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                let (solve, row) = (job["solve"].as_u64().unwrap_or(0) as usize, job["row"].as_u64().unwrap_or(0) as usize);
                let case = analysis.open_case(solve, row, &point)?;
                Ok(json!({"ok": true, "case": case, "report": analysis.to_json(false)}))
            }),
            other => Ok(json!({"ok": false, "kind": "usage", "code": "usage", "message": format!("unknown op {other}")})),
        };
        out.push(done.unwrap_or_else(|e| error(&e)));
    }
    println!("{}", Value::Array(out));
    Ok(())
}

fn run(args: Vec<String>) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}\nfean: error: the following arguments are required: command");
        return 3;
    };
    if command == "-h" || command == "--help" {
        println!("{HELP}");
        return 0;
    }
    if !["compile", "run", "dump", "check", "inspect", "batch"].contains(&command) {
        eprintln!("{USAGE}\nfean: error: argument command: invalid choice: '{command}'");
        return 3;
    }
    let outcome = if command == "batch" {
        batch().map(|()| String::new())
    } else {
        parse(command, &args[1..]).and_then(|parsed| {
            let path = shown(parsed.path.as_deref().unwrap()).to_string();
            let done = match command {
                "compile" => cmd_compile(&parsed),
                "run" => cmd_run(&parsed),
                "dump" => cmd_dump(&parsed),
                "check" => cmd_check(&parsed),
                _ => cmd_inspect(&parsed),
            };
            // name the input in whatever went wrong
            done.map(|()| String::new()).map_err(|stop| match stop {
                Stop::Fean(e) => {
                    let place = match e.line {
                        Some(line) if line > 0 && e.kind != Kind::Model => format!("{path}:{line}"),
                        _ => path.clone(),
                    };
                    eprintln!("{place}: [{}] {}", e.code, e.message);
                    Stop::Fean(e)
                }
                Stop::Io(text) => {
                    eprintln!("{path}: {text}");
                    Stop::Io(String::new())
                }
                Stop::NotUtf8 => {
                    eprintln!("{path}: UTF-8 글이 아닙니다");
                    Stop::NotUtf8
                }
                other => other,
            })
        })
    };
    match outcome {
        Ok(_) => 0,
        Err(Stop::Fean(e)) => {
            if e.kind == Kind::Model {
                2
            } else {
                1
            }
        }
        Err(Stop::Io(text)) => {
            if !text.is_empty() {
                eprintln!("fean: {text}");
            }
            3
        }
        Err(Stop::NotUtf8) => 3,
        Err(Stop::Usage(text)) => {
            if text.is_empty() {
                println!("{HELP}");
                return 0;
            }
            if text != "-" {
                eprintln!("{USAGE}\n{text}");
            }
            3
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // the front end recurses with the nesting of the text: give it a deep stack
    let code = with_stack(move || run(args));
    std::process::exit(code);
}
