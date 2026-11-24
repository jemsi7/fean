//! FEAN engine in Rust: compiler, image and machine (docs/engine.md; Map units I-09-a to I-10-a).
//!
//! The language is defined in docs/financial-language.md. The Python engine in `fean/` is the
//! reference this crate is checked against: same verdicts, same images byte for byte, same results.

pub mod check;
pub mod image;
pub mod lower;
pub mod session;
pub mod solve;
pub mod sym;
pub mod syntax;
pub mod trace;
pub mod vm;

pub use lower::SourceMap;
pub use syntax::{FeanError, Kind, Loc, Origin, R};

/// The standard contracts, shared with the Python engine (one file, DR-30).
pub const STD_PRELUDE: &str = include_str!("../../../fean/prelude.fean");

/// Run `f` on a thread with a large stack. The front end recurses with the nesting of the text,
/// which is limited to `syntax::DEPTH_LIMIT`; this stack is sized for that limit.
pub fn with_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(f)
        .expect("cannot start a worker thread")
        .join()
        .expect("the worker thread panicked")
}

/// Parse, expand and check a model text. `prelude = None` uses the standard contracts, `Some("")` none.
pub fn frontend(src: &str, prelude: Option<&str>) -> R<check::Spec> {
    let (name, decls) = syntax::parse_model(src, Origin::Model)?;
    let before = prelude.unwrap_or(STD_PRELUDE);
    let mut all = if before.is_empty() { Vec::new() } else { syntax::parse_decls(before, Origin::Prelude)? };
    all.extend(decls);
    check::build(&name, all)
}

/// A compiled model: the verified image, where its parts were written, and the checked model.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub image: serde_json::Value,
    pub map: SourceMap,
    pub spec: check::Spec,
}

/// Ahead-of-time compilation: model text -> verified image (and its source map).
pub fn compile(src: &str, prelude: Option<&str>) -> R<Compiled> {
    let spec = frontend(src, prelude)?;
    let (image, map) = lower::lower(&spec);
    image::verify(&image)?;
    Ok(Compiled { image, map, spec })
}
