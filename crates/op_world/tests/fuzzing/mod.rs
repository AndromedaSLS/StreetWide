//! A small, deterministic mutation fuzzer for StreetWide's strict file
//! readers (roadmap M1, "Before announcing"; SECURITY.md puts crafted
//! tiles, manifests and replays in scope).
//!
//! Stable Rust and no extra dependencies: a SplitMix64 generator drives
//! byte, JSON-text and JSON-value mutations of a corpus (the committed
//! fixtures plus hand-made edge cases), and every input goes through the
//! reader inside `catch_unwind`. A reader must return an error, never
//! panic, hang, overflow or allocate without bound.
//!
//! This file is shared: `op_world/tests/fuzz_readers.rs` declares it as a
//! module and `op_replay/tests/fuzz_replay.rs` includes it with `#[path]`,
//! so both suites use one engine without a dependency between the crates'
//! tests (ADR-0002).
//!
//! Each input is a pure function of the seed, the target name and the
//! iteration number, so every failure reproduces. Environment variables:
//!
//! - `STREETWIDE_FUZZ_ITERS`: scales every target's iteration count, as for
//!   the Python fuzz tests: a target whose default (sized for a debug build
//!   in CI) is d runs `STREETWIDE_FUZZ_ITERS` x d / 2000 iterations, so
//!   40000 runs every target 20 times longer.
//! - `STREETWIDE_FUZZ_SEED`: base seed (decimal, or hex with `0x`); the
//!   default is fixed.
//! - `STREETWIDE_FUZZ_HANG_SECS`: how long a target may make no progress
//!   before it is reported as hung (default 120 seconds).
//! - `STREETWIDE_FUZZ_TRACE=1`: print every iteration number before it
//!   runs, to find an input that aborts the process (a stack overflow
//!   cannot be caught).
//! - `STREETWIDE_FUZZ_ISOLATE`: `1` runs each target in a child process of
//!   its own, `0` in the test process. The default is a child process
//!   where the memory check below is available.
//!
//! Memory: where the kernel reports the process's peak size (Linux), each
//! target runs in a child process of its own, so that no other test shares
//! its address space, and every input that raises the peak may take at most
//! the target's allowance beyond what the process held before it (by
//! default 64 bytes per input byte plus 16 MiB). That catches a reader
//! reserving memory for what a length field claims. Rust cannot count
//! allocations here instead: a counting global allocator needs `unsafe`,
//! which the workspace forbids.
//!
//! On a panic the input is shrunk (keeping the same panic location), part by
//! part for targets whose inputs carry lengths, printed as a Rust
//! byte-string literal for a regression test, and saved under Cargo's
//! temporary directory for tests (`target/tmp/fuzz-failures`).

#![allow(dead_code)] // each test binary uses a different subset

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::io::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Map, Value};

/// Base seed when `STREETWIDE_FUZZ_SEED` is not set.
pub const DEFAULT_SEED: u64 = 0x5354_5245_4554_5744;

// ------------------------------------------------------------- generator --

/// SplitMix64 (Steele, Lea and Flood, 2014): tiny, fast and well mixed;
/// plenty for choosing mutations.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`; the modulo bias does not matter here).
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    /// Uniform in `lo..=hi`.
    pub fn range_i(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as usize) as i64
    }

    /// True with probability `percent` / 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// A non-empty range `a..b` of `0..len` (`len > 0`), at most `max` long.
    pub fn span(&mut self, len: usize, max: usize) -> (usize, usize) {
        let a = self.below(len);
        let n = self.range(1, (len - a).min(max.max(1)));
        (a, a + n)
    }

    /// How many mutations to stack: 1 most of the time, up to `max`.
    pub fn count(&mut self, max: usize) -> usize {
        1 + (self.next_u64().trailing_zeros() as usize).min(max - 1)
    }
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The generator for iteration `i` of `target`.
pub fn iteration_rng(seed: u64, target: &str, i: u64) -> Rng {
    let mut rng = Rng::new(seed ^ fnv1a(target) ^ i.wrapping_mul(0xD1B5_4A32_D192_ED03));
    rng.next_u64();
    rng
}

// ---------------------------------------------------------- configuration --

fn env_u64(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let text = raw.trim().replace('_', "");
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    Some(parsed.unwrap_or_else(|_| panic!("{name}={raw:?} is not a number")))
}

/// The unit of `STREETWIDE_FUZZ_ITERS`, in Rust and Python alike: the
/// default iteration count of the cheapest Python fuzz test.
pub const REFERENCE_ITERS: u64 = 2000;

/// Iterations for a target whose CI budget is `default`: `default`, or with
/// `STREETWIDE_FUZZ_ITERS=n`, `n` x `default` / 2000 (at least 1).
pub fn iterations(default: u64) -> u64 {
    env_u64("STREETWIDE_FUZZ_ITERS").map_or(default, |n| {
        (n.saturating_mul(default) / REFERENCE_ITERS).max(1)
    })
}

pub fn seed() -> u64 {
    env_u64("STREETWIDE_FUZZ_SEED").unwrap_or(DEFAULT_SEED)
}

/// How a target's inputs are made of parts (a bundle's files, a glb's
/// chunks, a replay's header and body), so that shrinking can delete bytes
/// inside each part and keep the lengths that hold them together right.
#[derive(Clone, Copy)]
pub struct Parts {
    pub split: fn(&[u8]) -> Option<Vec<Vec<u8>>>,
    pub join: fn(&[Vec<u8>]) -> Vec<u8>,
}

/// A fuzz target: its name, how many iterations it runs and how much memory
/// one input may take.
#[derive(Clone, Copy)]
pub struct Target {
    name: &'static str,
    iters: u64,
    per_byte: u64,
    fixed: u64,
    parts: Option<Parts>,
    in_process: bool,
}

impl Target {
    /// A target that runs `default` iterations unless `STREETWIDE_FUZZ_ITERS`
    /// scales them (see [`iterations`]). One input may take 64 bytes per
    /// input byte plus 16 MiB.
    pub fn new(name: &'static str, default: u64) -> Self {
        Target {
            name,
            iters: iterations(default),
            per_byte: 64,
            fixed: 16 << 20,
            parts: None,
            in_process: false,
        }
    }

    /// Runs in the test process even where a child process would measure
    /// memory better (for tests of this engine).
    pub fn in_process(mut self) -> Self {
        self.in_process = true;
        self
    }

    /// Exactly `iters` iterations, whatever `STREETWIDE_FUZZ_ITERS` says.
    pub fn exactly(mut self, iters: u64) -> Self {
        self.iters = iters;
        self
    }

    /// The memory one input may take beyond what the process held before
    /// it: `per_byte` per input byte plus `fixed` bytes.
    pub fn memory(mut self, per_byte: u64, fixed: u64) -> Self {
        (self.per_byte, self.fixed) = (per_byte, fixed);
        self
    }

    /// Inputs made of parts (see [`Parts`]).
    pub fn parts(
        mut self,
        split: fn(&[u8]) -> Option<Vec<Vec<u8>>>,
        join: fn(&[Vec<u8>]) -> Vec<u8>,
    ) -> Self {
        self.parts = Some(Parts { split, join });
        self
    }

    fn allowance(&self, input_len: usize) -> u64 {
        self.per_byte
            .saturating_mul(input_len as u64)
            .saturating_add(self.fixed)
    }
}

// ----------------------------------------------------------------- memory --

/// The process's current and peak address-space size in bytes, as the
/// kernel reports them (`VmSize` and `VmPeak` in `/proc/self/status`), or
/// `None` where it does not (Windows, macOS).
pub fn address_space() -> Option<(u64, u64)> {
    status_sizes(&std::fs::read_to_string("/proc/self/status").ok()?)
}

/// `VmSize` and `VmPeak`, in bytes, from the text of `/proc/<pid>/status`.
pub fn status_sizes(status: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let kib = status.lines().find_map(|l| l.strip_prefix(name))?;
        kib.trim()
            .strip_suffix("kB")?
            .trim()
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    };
    Some((field("VmSize:")?, field("VmPeak:")?))
}

/// If the process's peak size rose since `before` (its size and peak then),
/// how far above the size then it went. The peak never comes down, so an
/// input that stays below an earlier peak is not measured; one that
/// reserves much more than the inputs before it is.
fn excess_since(before: Option<(u64, u64)>) -> Option<u64> {
    let (size, peak) = before?;
    let (_, now) = address_space()?;
    (now > peak).then(|| now - size)
}

/// Set in the child process that runs one fuzz test alone.
const CHILD: &str = "STREETWIDE_FUZZ_CHILD";

/// Runs the calling test again, alone, in a child process (see the module
/// documentation), and checks that it passed. `false` if the target should
/// run in this process instead.
fn ran_in_child() -> bool {
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let isolate = match std::env::var("STREETWIDE_FUZZ_ISOLATE") {
        Ok(v) => v.trim() != "0",
        Err(_) => address_space().is_some(),
    };
    // libtest runs each test on a thread named after it.
    let current = std::thread::current();
    let Some(test) = current.name().filter(|n| isolate && *n != "main") else {
        return false;
    };
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args([test, "--exact", "--test-threads=1", "--nocapture"])
        .env(CHILD, "1")
        // glibc: one heap that grows with brk, and every block of 128 KiB
        // or more mapped on its own (no adaptive threshold), so that the
        // process size follows what the reader allocates and frees.
        .env("MALLOC_ARENA_MAX", "1")
        .env("MALLOC_MMAP_THRESHOLD_", "131072")
        .output()
        .expect("run the fuzz test in a child process");
    let out = String::from_utf8_lossy(&output.stdout);
    print!("{out}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success() && out.contains("test result: ok. 1 passed"),
        "fuzz test {test:?} failed in its child process ({}); its output is above",
        output.status
    );
    true
}

// ----------------------------------------------------------------- runner --

thread_local! {
    /// While set, the panic hook stays quiet on this thread (shrinking).
    static SILENT: Cell<bool> = const { Cell::new(false) };
    /// Where the last panic on this thread happened.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

static HOOK: Once = Once::new();

/// Records where panics happen (per thread) and passes them on to the
/// previous hook unless the thread is shrinking an input.
fn install_hook() {
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let at = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            LAST_PANIC.with(|p| *p.borrow_mut() = at);
            if !SILENT.with(Cell::get) {
                previous(info);
            }
        }));
    });
}

/// Where `check(input)` panics, or `None` if it does not.
fn panic_location(input: &[u8], check: &(dyn Fn(&[u8]) + Sync)) -> Option<String> {
    SILENT.with(|s| s.set(true));
    LAST_PANIC.with(|p| p.borrow_mut().take());
    let failed = panic::catch_unwind(AssertUnwindSafe(|| check(input))).is_err();
    SILENT.with(|s| s.set(false));
    let at = LAST_PANIC.with(|p| p.borrow_mut().take());
    if failed {
        Some(at.unwrap_or_else(|| "unknown location".into()))
    } else {
        None
    }
}

/// Runs `check(generate(rng))` for the target's iterations. `check` calls a
/// reader and asserts what must hold for what it returns; any panic is a
/// bug, and so is an input that takes more memory than the target allows
/// (where that can be measured). A target that makes no progress for
/// `STREETWIDE_FUZZ_HANG_SECS` is reported as hung and the process exits.
pub fn run<G, C>(t: Target, generate: G, check: C)
where
    G: Fn(&mut Rng) -> Vec<u8> + Sync,
    C: Fn(&[u8]) + Sync,
{
    if !t.in_process && ran_in_child() {
        return;
    }
    install_hook();
    let seed = seed();
    let hang_secs = env_u64("STREETWIDE_FUZZ_HANG_SECS").unwrap_or(120).max(1);
    let trace = std::env::var_os("STREETWIDE_FUZZ_TRACE").is_some();
    let measured = address_space().is_some();
    // The iteration running, and a heartbeat that shrinking a failing
    // input also advances (it calls the reader many times).
    let (progress, beats) = (AtomicU64::new(0), AtomicU64::new(0));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let (t, generate, check, progress_ref, beats_ref) = (&t, &generate, &check, &progress, &beats);
    std::thread::scope(|s| {
        let worker = std::thread::Builder::new()
            .name(format!("fuzz {}", t.name))
            .spawn_scoped(s, move || {
                for i in 0..t.iters {
                    progress_ref.store(i, Ordering::Relaxed);
                    beats_ref.fetch_add(1, Ordering::Relaxed);
                    if trace {
                        let _ = writeln!(std::io::stderr(), "fuzz {}: iteration {i}", t.name);
                    }
                    let Ok(input) = panic::catch_unwind(AssertUnwindSafe(|| {
                        generate(&mut iteration_rng(seed, t.name, i))
                    })) else {
                        panic!(
                            "the input generator of fuzz target {:?} failed at iteration \
                             {i} (STREETWIDE_FUZZ_SEED={seed:#x}): a bug in the test, not the reader",
                            t.name
                        );
                    };
                    let before = address_space();
                    if panic::catch_unwind(AssertUnwindSafe(|| check(&input))).is_err() {
                        fail(t, seed, i, &input, check, beats_ref);
                    }
                    if let Some(excess) = excess_since(before)
                        && excess > t.allowance(input.len())
                    {
                        too_much_memory(t, seed, i, &input, excess);
                    }
                }
                let _ = done_tx.send(());
            })
            .expect("spawn the fuzz worker");
        // Watchdog: the worker either finishes, panics (the channel closes)
        // or stops making progress.
        let (mut last, mut idle) = (u64::MAX, 0);
        loop {
            match done_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    let now = beats.load(Ordering::Relaxed);
                    if now == last {
                        idle += 1;
                    } else {
                        (last, idle) = (now, 0);
                    }
                    if idle >= hang_secs {
                        let i = progress.load(Ordering::Relaxed);
                        hung(t, seed, i, hang_secs, generate);
                    }
                }
            }
        }
        if let Err(payload) = worker.join() {
            panic::resume_unwind(payload);
        }
    });
    println!(
        "fuzz {}: {} iterations, seed {seed:#x}, no failures{}",
        t.name,
        t.iters,
        if measured { ", memory checked" } else { "" }
    );
}

fn fail(
    t: &Target,
    seed: u64,
    i: u64,
    input: &[u8],
    check: &(dyn Fn(&[u8]) + Sync),
    beats: &AtomicU64,
) -> ! {
    let location = panic_location(input, check);
    let small = match &location {
        Some(at) => shrink(
            input,
            &mut |candidate| panic_location(candidate, check).as_deref() == Some(at.as_str()),
            t.parts,
            beats,
        ),
        None => input.to_vec(), // flaky: did not panic again
    };
    let saved = save(t.name, seed, i, input);
    panic!(
        "fuzz target {:?} failed at iteration {i} (STREETWIDE_FUZZ_SEED={seed:#x})\n  \
         panic at {}\n  input: {} bytes, saved to {saved}\n  shrunk to {} bytes: {}",
        t.name,
        location.as_deref().unwrap_or("(did not panic again)"),
        input.len(),
        small.len(),
        describe(t, &small)
    );
}

/// An input took more memory than the target allows. It is not shrunk: the
/// process's peak size never comes down, so a smaller input cannot be
/// measured against it again.
fn too_much_memory(t: &Target, seed: u64, i: u64, input: &[u8], excess: u64) -> ! {
    let saved = save(t.name, seed, i, input);
    panic!(
        "fuzz target {:?} failed at iteration {i} (STREETWIDE_FUZZ_SEED={seed:#x})\n  \
         it held {excess} bytes more than before this input, beyond its allowance of {} \
         ({} per input byte plus {})\n  input: {} bytes, saved to {saved}: {}",
        t.name,
        t.allowance(input.len()),
        t.per_byte,
        t.fixed,
        input.len(),
        describe(t, input)
    );
}

fn hung<G: Fn(&mut Rng) -> Vec<u8>>(t: &Target, seed: u64, i: u64, secs: u64, generate: &G) -> ! {
    let input = generate(&mut iteration_rng(seed, t.name, i));
    let saved = save(t.name, seed, i, &input);
    // Written straight to stderr: the test harness would drop captured
    // output when the process exits.
    let _ = writeln!(
        std::io::stderr(),
        "\nfuzz target {:?} HUNG at iteration {i} (STREETWIDE_FUZZ_SEED={seed:#x}): \
         no progress for {secs} s\n  input: {} bytes, saved to {saved}\n  {}",
        t.name,
        input.len(),
        describe(t, &input)
    );
    std::process::exit(101);
}

/// Deletes chunks of `input` while `fails` still holds: first whole parts
/// (but the first) and chunks inside each part, joined back with their
/// lengths right, for targets whose inputs have parts; then chunks of the
/// bytes for the others.
fn shrink(
    input: &[u8],
    fails: &mut dyn FnMut(&[u8]) -> bool,
    parts: Option<Parts>,
    beats: &AtomicU64,
) -> Vec<u8> {
    let mut current = input.to_vec();
    let mut budget = 4000usize;
    let mut tried = |candidate: &[u8], budget: &mut usize| {
        *budget -= 1;
        beats.fetch_add(1, Ordering::Relaxed);
        fails(candidate)
    };
    if let Some(p) = parts
        && let Some(mut pieces) = (p.split)(&current)
    {
        let mut k = 1;
        while k < pieces.len() && budget > 0 {
            let mut candidate = pieces.clone();
            candidate.remove(k);
            let joined = (p.join)(&candidate);
            if tried(&joined, &mut budget) {
                (pieces, current) = (candidate, joined);
            } else {
                k += 1;
            }
        }
        for k in 0..pieces.len() {
            let mut chunk = (pieces[k].len() / 2).max(1);
            while budget > 0 && !pieces[k].is_empty() {
                let (mut at, mut changed) = (0, false);
                while at < pieces[k].len() && budget > 0 {
                    let mut candidate = pieces.clone();
                    let end = (at + chunk).min(candidate[k].len());
                    candidate[k].drain(at..end);
                    let joined = (p.join)(&candidate);
                    if tried(&joined, &mut budget) {
                        (pieces, current) = (candidate, joined);
                        changed = true;
                    } else {
                        at += chunk;
                    }
                }
                if !changed {
                    if chunk == 1 {
                        break;
                    }
                    chunk /= 2;
                }
            }
        }
        return current;
    }
    let mut chunk = (current.len() / 2).max(1);
    while budget > 0 && !current.is_empty() {
        let (mut at, mut changed) = (0, false);
        while at < current.len() && budget > 0 {
            let end = (at + chunk).min(current.len());
            let mut candidate = current[..at].to_vec();
            candidate.extend_from_slice(&current[end..]);
            if tried(&candidate, &mut budget) {
                current = candidate;
                changed = true;
            } else {
                at += chunk;
            }
        }
        if !changed {
            if chunk == 1 {
                break;
            }
            chunk /= 2;
        }
    }
    current
}

fn save(target: &str, seed: u64, i: u64, input: &[u8]) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz-failures");
    let path = dir.join(format!("{target}-{seed:016x}-{i}.bin"));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, input)) {
        Ok(()) => path.display().to_string(),
        Err(e) => format!("(not saved: {e})"),
    }
}

/// `bytes` for a failure report: as one literal and, for targets whose
/// inputs have parts, part by part.
fn describe(t: &Target, bytes: &[u8]) -> String {
    let mut s = printable(bytes);
    if let Some(pieces) = t.parts.and_then(|p| (p.split)(bytes)) {
        for (k, piece) in pieces.iter().enumerate() {
            let _ = write!(
                s,
                "\n  part {k} ({} bytes): {}",
                piece.len(),
                printable(piece)
            );
        }
    }
    s
}

fn printable(bytes: &[u8]) -> String {
    if bytes.len() <= 8192 {
        literal(bytes)
    } else {
        "(too long to print; see the saved file)".into()
    }
}

/// `bytes` as a Rust byte-string literal.
pub fn literal(bytes: &[u8]) -> String {
    let mut s = String::from("b\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(char::from(b)),
            _ => {
                let _ = write!(s, "\\x{b:02x}");
            }
        }
    }
    s.push('"');
    s
}

// ----------------------------------------------------------------- corpus --

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every file under the given directories of the repository, sorted by
/// path (`/`-separated, relative to the repository root).
pub fn fixture_files(dirs: &[&str]) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("readable fixture directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).expect("under the root");
                let name = rel.to_string_lossy().replace('\\', "/");
                out.push((name, std::fs::read(&path).expect("readable fixture")));
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    for d in dirs {
        walk(&root, &root.join(d), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ------------------------------------------------------------- byte level --

pub const INTERESTING_BYTES: &[u8] = &[
    0x00, 0x01, 0x7f, 0x80, 0xff, 0xfe, b' ', b'"', b'\\', b'/', b'{', b'}', b'[', b']', b',',
    b':', b'-', b'+', b'.', b'0', b'1', b'9', b'e', b'E', b'n', b'u', b'\n', 0xc0, 0xed, 0xef,
    0xf4,
];

/// A value that breaks length and offset arithmetic, for a 32-bit field
/// holding `current` in a file of `file_len` bytes.
pub fn interesting_u32(rng: &mut Rng, file_len: usize, current: u32) -> u32 {
    const FIXED: &[u32] = &[
        0,
        1,
        2,
        3,
        4,
        8,
        12,
        16,
        20,
        24,
        0x7f,
        0x80,
        0xff,
        0x100,
        0xffff,
        0x1_0000,
        64 << 20,
        (64 << 20) + 1,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_ffec,
        0xffff_fff8,
        0xffff_fffc,
        0xffff_fffe,
        0xffff_ffff,
    ];
    let len = u32::try_from(file_len).unwrap_or(u32::MAX);
    match rng.below(4) {
        0 | 1 => *rng.pick(FIXED),
        2 => len.wrapping_add(rng.range_i(-28, 28) as u32),
        _ => current.wrapping_add(rng.range_i(-12, 12) as u32),
    }
}

/// Like [`interesting_u32`], for 64-bit fields (ticks, sizes).
pub fn interesting_u64(rng: &mut Rng, current: u64) -> u64 {
    const FIXED: &[u64] = &[
        0,
        1,
        59,
        60,
        61,
        3600,
        3601,
        5_184_000,
        5_184_001,
        1 << 32,
        (1 << 53) - 1,
        1 << 53,
        i64::MAX as u64,
        1 << 63,
        u64::MAX - 1,
        u64::MAX,
    ];
    match rng.below(3) {
        0 | 1 => *rng.pick(FIXED),
        _ => current.wrapping_add(rng.range_i(-12, 12) as u64),
    }
}

/// Applies a few byte-level mutations (usually one). `donor` is another
/// corpus file to splice from.
pub fn mutate_bytes(rng: &mut Rng, data: &mut Vec<u8>, donor: &[u8]) {
    for _ in 0..rng.count(8) {
        mutate_bytes_once(rng, data, donor);
    }
}

fn mutate_bytes_once(rng: &mut Rng, data: &mut Vec<u8>, donor: &[u8]) {
    let len = data.len();
    match rng.below(13) {
        0 if len > 0 => {
            let i = rng.below(len);
            data[i] ^= 1 << rng.below(8);
        }
        1 if len > 0 => {
            let i = rng.below(len);
            data[i] = *rng.pick(INTERESTING_BYTES);
        }
        2 if len > 0 => {
            let i = rng.below(len);
            data[i] = rng.byte();
        }
        3 if len >= 4 => {
            // Most length fields are 4-byte aligned.
            let i = if rng.chance(75) {
                rng.below(len / 4) * 4
            } else {
                rng.below(len - 3)
            };
            let current = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            let v = interesting_u32(rng, len, current);
            data[i..i + 4].copy_from_slice(&v.to_le_bytes());
        }
        4 if len >= 8 => {
            let i = if rng.chance(75) {
                rng.below(len / 8) * 8
            } else {
                rng.below(len - 7)
            };
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[i..i + 8]);
            let v = interesting_u64(rng, u64::from_le_bytes(b));
            data[i..i + 8].copy_from_slice(&v.to_le_bytes());
        }
        5 => {
            let at = rng.below(len + 1);
            let n = rng.range(1, 16);
            let bytes: Vec<u8> = (0..n)
                .map(|_| {
                    if rng.chance(50) {
                        *rng.pick(INTERESTING_BYTES)
                    } else {
                        rng.byte()
                    }
                })
                .collect();
            data.splice(at..at, bytes);
        }
        6 if len > 0 => {
            let (a, b) = rng.span(len, 64);
            data.drain(a..b);
        }
        7 if len > 0 => data.truncate(rng.below(len)),
        8 if len > 0 => {
            // Duplicate a chunk somewhere else.
            let (a, b) = rng.span(len, 256);
            let chunk = data[a..b].to_vec();
            let at = rng.below(len + 1);
            data.splice(at..at, chunk);
        }
        9 if !donor.is_empty() => {
            // Splice: our head, the donor's tail.
            let at = rng.below(len + 1);
            let from = rng.below(donor.len());
            data.truncate(at);
            data.extend_from_slice(&donor[from..]);
        }
        10 if !donor.is_empty() => {
            let (a, b) = rng.span(donor.len(), 512);
            let at = rng.below(len + 1);
            data.splice(at..at, donor[a..b].iter().copied());
        }
        11 if len > 0 => {
            // Repeat a chunk many times.
            let (a, b) = rng.span(len, 64);
            let times = rng.range(2, 64);
            let chunk = data[a..b].repeat(times);
            data.splice(b..b, chunk);
        }
        _ => {
            let at = rng.below(len + 1);
            data.insert(at, rng.byte());
        }
    }
}

// -------------------------------------------------------------- JSON text --

/// Byte spans of the parts of a JSON text.
#[derive(Default, Debug)]
pub struct Spans {
    pub values: Vec<(usize, usize)>,
    /// `"key":value` members of objects.
    pub members: Vec<(usize, usize)>,
    /// Elements of arrays.
    pub elements: Vec<(usize, usize)>,
    /// Strings, with their quotes.
    pub strings: Vec<(usize, usize)>,
    pub numbers: Vec<(usize, usize)>,
}

/// Scans a JSON text without validating escapes, numbers or UTF-8. `None`
/// if it is not JSON-shaped or nests deeper than 256.
pub fn scan(json: &[u8]) -> Option<Spans> {
    let mut s = Scanner {
        b: json,
        i: 0,
        depth: 0,
        out: Spans::default(),
    };
    s.value()?;
    s.ws();
    (s.i == json.len()).then_some(s.out)
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
    out: Spans,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        if self.peek()? == c {
            self.i += 1;
            Some(())
        } else {
            None
        }
    }

    fn string(&mut self) -> Option<()> {
        let start = self.i;
        self.eat(b'"')?;
        loop {
            match self.peek()? {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
        self.out.strings.push((start, self.i));
        Some(())
    }

    fn nest(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= 256).then_some(())
    }

    fn value(&mut self) -> Option<()> {
        self.ws();
        let start = self.i;
        match self.peek()? {
            b'{' => {
                self.nest()?;
                self.i += 1;
                self.ws();
                if self.eat(b'}').is_none() {
                    loop {
                        self.ws();
                        let member = self.i;
                        self.string()?;
                        self.ws();
                        self.eat(b':')?;
                        self.value()?;
                        self.out.members.push((member, self.i));
                        self.ws();
                        if self.eat(b',').is_none() {
                            self.eat(b'}')?;
                            break;
                        }
                    }
                }
                self.depth -= 1;
            }
            b'[' => {
                self.nest()?;
                self.i += 1;
                self.ws();
                if self.eat(b']').is_none() {
                    loop {
                        self.ws();
                        let element = self.i;
                        self.value()?;
                        self.out.elements.push((element, self.i));
                        self.ws();
                        if self.eat(b',').is_none() {
                            self.eat(b']')?;
                            break;
                        }
                    }
                }
                self.depth -= 1;
            }
            b'"' => self.string()?,
            b'-' | b'0'..=b'9' => {
                while matches!(
                    self.peek(),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.i += 1;
                }
                self.out.numbers.push((start, self.i));
            }
            b't' | b'f' | b'n' => {
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.i += 1;
                }
            }
            _ => return None,
        }
        self.out.values.push((start, self.i));
        Some(())
    }
}

/// Number tokens around the canonical-JSON limits and the readers' ranges,
/// and forms canonical JSON forbids.
pub const NUMBER_SNIPPETS: &[&str] = &[
    "0",
    "-0",
    "1",
    "-1",
    "1.0",
    "1.5",
    "-0.5",
    "1e3",
    "1E3",
    "1e+3",
    "1e-3",
    "01",
    "00",
    "-01",
    "1.",
    ".5",
    "+1",
    "-",
    "--1",
    "9007199254740991",
    "-9007199254740991",
    "9007199254740992",
    "-9007199254740992",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775808",
    "-9223372036854775809",
    "18446744073709551615",
    "18446744073709551616",
    "1e308",
    "1e309",
    "-1e309",
    "4.9e-324",
    "123456789012345678901234567890123456789012345678901234567890",
    "4294967295",
    "4294967296",
    "65535",
    "65536",
    "131071",
    "131072",
    "4095",
    "4096",
    "16385",
    "5184000",
    "5184001",
    "3600",
    "3601",
    "60",
    "NaN",
    "Infinity",
    "0x10",
];

/// Raw bytes to put inside a JSON string: escapes canonical JSON forbids,
/// broken escapes, control characters and invalid UTF-8.
const STRING_SNIPPETS: &[&[u8]] = &[
    b"",
    b"\\u0000",
    b"\\u001f",
    b"\\u007f",
    b"\\u0080",
    b"\\ud800",
    b"\\udc00",
    b"\\ud83d\\ude00",
    b"\\udbff\\udfff",
    b"\\/",
    b"\\b",
    b"\\f",
    b"\\n",
    b"\\r",
    b"\\t",
    b"\\u00e9",
    b"\\u00E9",
    b"\\\"",
    b"\\\\",
    b"\\x41",
    b"\\u12",
    b"\\u",
    b"\\",
    b"\\U0041",
    b"\\'",
    b"\x01",
    b"\x1f",
    b"\x7f",
    b"\xc0\x80",
    b"\xc3",
    b"\xed\xa0\x80",
    b"\xf4\x90\x80\x80",
    b"\xf8\x88\x80\x80\x80",
    b"\xff",
    b"\xef\xbb\xbf",
    "\u{e9}".as_bytes(),
    "\u{a9}".as_bytes(),
    "\u{10ffff}".as_bytes(),
    b"blake3:",
    b"A",
    b" ",
    b"/",
    b":",
    b"../",
    b"..\\",
];

fn string_snippet(rng: &mut Rng) -> Vec<u8> {
    match rng.below(6) {
        0 => vec![b'a'; *rng.pick(&[4095, 4096, 4097, 70_000])],
        1 => "\u{e9}".repeat(*rng.pick(&[2048, 4097])).into_bytes(),
        _ => rng.pick(STRING_SNIPPETS).to_vec(),
    }
}

const VALUE_SNIPPETS: &[&str] = &[
    "null",
    "true",
    "false",
    "[]",
    "{}",
    "\"\"",
    "[null]",
    "{\"a\":1}",
    "{\"a\":1,\"a\":1}",
    "{\"a\":1,\"a\":2}",
    "{\"\":1}",
    "{\"A\":1}",
    "{\"a\" :1}",
    "[1,]",
    "[,1]",
    "{,}",
    "{\"a\":}",
    "nul",
    "tru",
    "nulll",
    "[",
    "]",
    "{",
    "}",
    "\"",
    ",",
    ":",
    "[[[[",
    "]]]]",
    "\"\\u0000\"",
    "1 2",
    "{\"a\":1}{\"a\":1}",
];

fn value_snippet(rng: &mut Rng) -> Vec<u8> {
    match rng.below(4) {
        0 => rng.pick(NUMBER_SNIPPETS).as_bytes().to_vec(),
        1 => {
            let mut s = vec![b'"'];
            s.extend(string_snippet(rng));
            s.push(b'"');
            s
        }
        _ => rng.pick(VALUE_SNIPPETS).as_bytes().to_vec(),
    }
}

/// One structural edit of a JSON text: duplicate keys and elements, removed
/// members, odd numbers, strings and escapes, deep nesting, swapped values,
/// whitespace. `None` if `json` cannot be scanned.
pub fn mutate_json_text(rng: &mut Rng, json: &[u8]) -> Option<Vec<u8>> {
    let spans = scan(json)?;
    let mut out = json.to_vec();
    match rng.below(9) {
        0 if !spans.members.is_empty() => {
            // A duplicate key.
            let (a, b) = *rng.pick(&spans.members);
            let mut copy = vec![b','];
            copy.extend_from_slice(&json[a..b]);
            out.splice(b..b, copy);
        }
        1 if !spans.elements.is_empty() => {
            let (a, b) = *rng.pick(&spans.elements);
            let times = if rng.chance(80) { 1 } else { rng.range(2, 60) };
            let mut copy = Vec::new();
            for _ in 0..times {
                copy.push(b',');
                copy.extend_from_slice(&json[a..b]);
            }
            out.splice(b..b, copy);
        }
        2 if !spans.members.is_empty() || !spans.elements.is_empty() => {
            // Remove a member or an element with one neighbouring comma.
            let all: Vec<(usize, usize)> = spans
                .members
                .iter()
                .chain(&spans.elements)
                .copied()
                .collect();
            let (a, b) = *rng.pick(&all);
            let (a, b) = if json.get(b) == Some(&b',') {
                (a, b + 1)
            } else if a > 0 && json[a - 1] == b',' {
                (a - 1, b)
            } else {
                (a, b)
            };
            out.drain(a..b);
        }
        3 if !spans.numbers.is_empty() => {
            let (a, b) = *rng.pick(&spans.numbers);
            out.splice(a..b, rng.pick(NUMBER_SNIPPETS).bytes());
        }
        4 if !spans.strings.is_empty() => {
            // Replace or extend the content of a string (keys included).
            let (a, b) = *rng.pick(&spans.strings);
            let snippet = string_snippet(rng);
            if rng.chance(50) {
                out.splice(a + 1..b - 1, snippet);
            } else {
                out.splice(b - 1..b - 1, snippet);
            }
        }
        5 => {
            // Deep nesting, around the parser's recursion limit and far past it.
            let (a, b) = *rng.pick(&spans.values);
            let depth = *rng.pick(&[1, 2, 16, 126, 127, 128, 129, 300, 5000]);
            let (open, close) = if rng.chance(50) {
                ("[", "]")
            } else {
                ("{\"a\":", "}")
            };
            let mut wrapped = open.repeat(depth).into_bytes();
            wrapped.extend_from_slice(&json[a..b]);
            wrapped.extend_from_slice(close.repeat(depth).as_bytes());
            out.splice(a..b, wrapped);
        }
        6 => {
            let (a, b) = *rng.pick(&spans.values);
            out.splice(a..b, value_snippet(rng));
        }
        7 if spans.values.len() >= 2 => {
            // One value replaced by a copy of another (type confusion).
            let (a, b) = *rng.pick(&spans.values);
            let (c, d) = *rng.pick(&spans.values);
            out.splice(a..b, json[c..d].iter().copied());
        }
        _ => {
            let at = rng.below(json.len() + 1);
            let ws: &str = rng.pick(&[" ", "\n", "\t", "\r\n", "\u{feff}"]);
            out.splice(at..at, ws.bytes());
        }
    }
    Some(out)
}

// ------------------------------------------------------------ JSON values --

/// Integers at the edges of the readers' ranges.
pub const INTERESTING_I64: &[i64] = &[
    0,
    1,
    -1,
    2,
    3,
    59,
    60,
    61,
    127,
    128,
    255,
    256,
    3599,
    3600,
    3601,
    4095,
    4096,
    16_384,
    16_385,
    32_767,
    32_768,
    65_535,
    65_536,
    131_071,
    131_072,
    5_184_000,
    5_184_001,
    85_051_128_780,
    85_051_128_781,
    180_000_000_000,
    180_000_000_001,
    -180_000_000_001,
    2_147_483_647,
    2_147_483_648,
    4_294_967_295,
    4_294_967_296,
    64 << 20,
    (64 << 20) + 1,
    (1 << 53) - 1,
    -((1 << 53) - 1),
    1 << 53,
    -(1 << 53),
    i64::MAX,
    i64::MIN,
];

const WORDS: &[&str] = &[
    "",
    " ",
    "\u{0}",
    "\u{1f}",
    "\u{7f}",
    "\u{80}",
    "\u{ffff}",
    "\u{10ffff}",
    "\u{feff}",
    "\u{e9}",
    "\"",
    "\\",
    "/",
    "..",
    "../x",
    "..\\x",
    "a/b",
    "C:\\x",
    "streetwide.asset",
    "streetwide.tile",
    "streetwide.world",
    "streetwide.change",
    "streetwide.check",
    "streetwide.merge",
    "streetwide.replay",
    "0.1.0",
    "0.1.1",
    "0.2.0",
    "facade",
    "furniture",
    "interactivity",
    "interior",
    "shell",
    "terrain",
    "transport",
    "trimesh",
    "none",
    "add",
    "remove",
    "replace",
    "pass",
    "fail",
    "not_applicable",
    "not_run",
    "review",
    "baseline",
    "merged",
    "takedown",
    "better",
    "not_better",
    "blocked",
    "17/0/0",
    "17/131071/131071",
    "17/131072/0",
    "17/68646/45903",
    "17/68646/45908",
    "17/-1/0",
    "17/01/0",
    "17/+1/0",
    "16/0/0",
    "17/0/0/0",
    "17//0",
    "12/2145/1434",
    "12/4095/4095",
    "12/4096/0",
    "osm:way:1",
    "a:b",
    "a:",
    ":b",
    "github:x",
    "streetwide:ingest",
    "tile:17/1/2",
    "Osm:way:1",
    "https://x",
    "https://",
    "http://x",
    "https://a b",
    "CC-BY-SA-4.0",
    "CC0-1.0",
    "ODbL-1.0",
    "LicenseRef-swisstopo-OGD",
    "CC-BY-SA-4.0 AND LicenseRef-swisstopo-OGD",
    "MIT OR Apache-2.0",
    "\u{a9}",
    "G1",
    "G10",
    "zurich-altstadt",
    "a--b",
    "-a",
    "A",
    "StreetWide world format 0.1",
    "2.0",
    "VEC3",
    "SCALAR",
    "roof",
    "wall",
    "water",
    "marble",
];

/// A string a reader might trip over.
pub fn interesting_string(rng: &mut Rng) -> String {
    match rng.below(6) {
        0 => content_id_like(rng),
        1 => "a".repeat(*rng.pick(&[64, 65, 256, 257, 2048, 2049, 4096, 4097])),
        2 => (0..rng.range(1, 12))
            .map(|_| char::from(rng.range(0x20, 0x7e) as u8))
            .collect(),
        _ => (*rng.pick(WORDS)).to_string(),
    }
}

/// A content id, or something close to one.
pub fn content_id_like(rng: &mut Rng) -> String {
    let hex: String = (0..64)
        .map(|_| char::from(b"0123456789abcdef"[rng.below(16)]))
        .collect();
    match rng.below(7) {
        0 => format!("blake3:{}", &hex[..63]),
        1 => format!("blake3:{hex}0"),
        2 => format!("blake3:{}", hex.to_uppercase()),
        3 => format!("BLAKE3:{hex}"),
        4 => hex,
        _ => format!("blake3:{hex}"),
    }
}

fn interesting_key(rng: &mut Rng) -> String {
    (*rng.pick(&[
        "a", "zz", "A", "_a", "a-b", "", "format", "x1", "extra", "name", "uri",
    ]))
    .to_string()
}

fn mutate_string(rng: &mut Rng, s: &mut String) {
    let mut chars: Vec<char> = s.chars().collect();
    match rng.below(6) {
        0 if !chars.is_empty() => {
            chars.remove(rng.below(chars.len()));
        }
        1 => {
            let at = rng.below(chars.len() + 1);
            let c = *rng.pick(&[
                '\0',
                '\u{7f}',
                'A',
                'z',
                '0',
                '9',
                '-',
                '_',
                ':',
                '/',
                '.',
                ' ',
                '\u{e9}',
                '\u{10ffff}',
            ]);
            chars.insert(at, c);
        }
        2 => chars = s.to_uppercase().chars().collect(),
        3 => chars.extend(s.chars()),
        4 if !chars.is_empty() => {
            // Change one digit or letter: a different id, key or number.
            let i = rng.below(chars.len());
            chars[i] = match chars[i] {
                '0'..='8' => char::from(chars[i] as u8 + 1),
                '9' => '0',
                'a'..='e' => char::from(chars[i] as u8 + 1),
                'f' => 'a',
                _ => 'x',
            };
        }
        _ => chars.clear(),
    }
    *s = chars.into_iter().collect();
}

/// A random node of `root`, usually a leaf or close to one.
fn random_node<'a>(rng: &mut Rng, mut node: &'a mut Value) -> &'a mut Value {
    loop {
        if !rng.chance(88) {
            return node;
        }
        let len = match &*node {
            Value::Array(a) => a.len(),
            Value::Object(m) => m.len(),
            _ => 0,
        };
        if len == 0 {
            return node;
        }
        let k = rng.below(len);
        node = match node {
            Value::Array(a) => &mut a[k],
            Value::Object(m) => m.values_mut().nth(k).expect("k < len"),
            other => return other,
        };
    }
}

fn random_subtree<'a>(rng: &mut Rng, mut node: &'a Value) -> &'a Value {
    loop {
        let children: Vec<&Value> = match node {
            Value::Array(a) => a.iter().collect(),
            Value::Object(m) => m.values().collect(),
            _ => return node,
        };
        if children.is_empty() || !rng.chance(70) {
            return node;
        }
        node = children[rng.below(children.len())];
    }
}

/// One value-level edit somewhere in `root`: usually one that keeps the
/// node's JSON type (so typed parsing succeeds and the format's rules are
/// what is tested), sometimes any edit. `donor` is a document to borrow
/// subtrees from.
pub fn edit_value(rng: &mut Rng, root: &mut Value, donor: Option<&Value>) {
    let node = random_node(rng, root);
    if rng.chance(70) {
        edit_keeping_type(rng, node, donor);
    } else {
        edit_any(rng, node, donor);
    }
}

fn edit_keeping_type(rng: &mut Rng, node: &mut Value, donor: Option<&Value>) {
    match node {
        Value::Number(_) => {
            *node = match (node.as_i64(), rng.below(8)) {
                (Some(i), 0..=3) => Value::from(match rng.below(5) {
                    0 => i.wrapping_add(1),
                    1 => i.wrapping_sub(1),
                    2 => i.wrapping_neg(),
                    3 => i.wrapping_mul(2),
                    _ => i.wrapping_add(rng.range_i(-1000, 1000)),
                }),
                (_, 7) => Value::from(*rng.pick(&[u64::MAX, 1 << 63, (1 << 53) + 1])),
                _ => Value::from(*rng.pick(INTERESTING_I64)),
            };
        }
        Value::String(s) => {
            if rng.chance(50) {
                mutate_string(rng, s);
            } else {
                *s = interesting_string(rng);
            }
        }
        Value::Bool(b) => *b = !*b,
        // Nullable fields are mostly content ids.
        Value::Null => *node = Value::String(content_id_like(rng)),
        Value::Array(items) => {
            if items.is_empty() {
                if let Some(d) = donor {
                    items.push(random_subtree(rng, d).clone());
                }
                return;
            }
            let i = rng.below(items.len());
            match rng.below(6) {
                0 => {
                    items.remove(i);
                }
                1 => {
                    let copy = items[i].clone();
                    items.insert(i, copy);
                }
                2 => {
                    let copy = items[i].clone();
                    let n = rng.range(2, 64);
                    items.extend(std::iter::repeat_n(copy, n));
                }
                3 => {
                    let j = rng.below(items.len());
                    items.swap(i, j);
                }
                4 => items.truncate(i),
                _ => items.clear(),
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                map.insert(interesting_key(rng), Value::Null);
                return;
            }
            let key = map
                .keys()
                .nth(rng.below(map.len()))
                .cloned()
                .expect("non-empty map");
            match rng.below(3) {
                0 => {
                    map.remove(&key);
                }
                1 => {
                    let v = map.remove(&key).expect("present");
                    map.insert(interesting_key(rng), v);
                }
                _ => {
                    map.insert(interesting_key(rng), Value::Null);
                }
            }
        }
    }
}

fn edit_any(rng: &mut Rng, node: &mut Value, donor: Option<&Value>) {
    match rng.below(7) {
        0 => *node = Value::from(*rng.pick(INTERESTING_I64)),
        1 => *node = Value::String(interesting_string(rng)),
        2 => {
            *node = rng
                .pick(&[
                    Value::Null,
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Array(Vec::new()),
                    Value::Object(Map::new()),
                ])
                .clone();
        }
        3 => {
            // Nesting just past the parser's recursion limit.
            let depth = *rng.pick(&[1, 2, 127, 128, 129]);
            for _ in 0..depth {
                let inner = node.take();
                *node = Value::Array(vec![inner]);
            }
        }
        4 | 5 if donor.is_some() => {
            *node = random_subtree(rng, donor.expect("checked")).clone();
        }
        _ => {
            *node = match node.take() {
                Value::String(s) => s.parse::<i64>().map_or(Value::Null, Value::from),
                Value::Number(n) => Value::String(n.to_string()),
                other => Value::Array(vec![other]),
            };
        }
    }
}

/// Applies a few value-level edits to `value` and writes it back as
/// canonical JSON when it can (so the result gets past the canonical-form
/// check into the typed and semantic checks), else as plain JSON.
pub fn mutate_value(rng: &mut Rng, value: &Value, donor: Option<&Value>) -> Vec<u8> {
    let mut v = value.clone();
    for _ in 0..rng.count(4) {
        edit_value(rng, &mut v, donor);
    }
    op_world::cjson::to_vec(&v)
        .unwrap_or_else(|_| serde_json::to_vec(&v).expect("a JSON value serialises"))
}

/// A corpus document mutated at the value, text or byte level (or a mix).
pub fn mutate_json(
    rng: &mut Rng,
    doc: &[u8],
    value: Option<&Value>,
    donor: &[u8],
    donor_value: Option<&Value>,
) -> Vec<u8> {
    let mut out = match rng.below(10) {
        0..=4 => value.map(|v| mutate_value(rng, v, donor_value)),
        5..=7 => mutate_json_text(rng, doc),
        _ => None,
    };
    if out.is_none() || rng.chance(15) {
        let mut bytes = out.take().unwrap_or_else(|| doc.to_vec());
        mutate_bytes(rng, &mut bytes, donor);
        out = Some(bytes);
    }
    out.expect("set above")
}

// -------------------------------------------------------------------- glb --

/// The JSON and BIN chunks of a glb whose lengths are consistent.
pub fn glb_chunks(glb: &[u8]) -> Option<(&[u8], &[u8])> {
    let u32_at = |at: usize| {
        glb.get(at..at.checked_add(4)?)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let json_len = u32_at(12)?;
    let json = glb.get(20..20usize.checked_add(json_len)?)?;
    let bin_len = u32_at(20 + json_len)?;
    let bin = glb.get(28 + json_len..(28 + json_len).checked_add(bin_len)?)?;
    Some((json, bin))
}

/// A glb made of these chunks, with correct lengths and the JSON padded
/// with spaces.
pub fn glb_pack(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let mut json = json.to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

/// A glb's JSON and BIN chunks, as the parts of a target's input.
pub fn glb_split(glb: &[u8]) -> Option<Vec<Vec<u8>>> {
    let (json, bin) = glb_chunks(glb)?;
    Some(vec![json.to_vec(), bin.to_vec()])
}

/// The glb made of these parts (a missing chunk is empty).
pub fn glb_join(parts: &[Vec<u8>]) -> Vec<u8> {
    let chunk = |k: usize| parts.get(k).map_or(&[][..], Vec::as_slice);
    glb_pack(chunk(0), chunk(1))
}

fn interesting_index(rng: &mut Rng, bin_len: usize) -> Value {
    match rng.below(5) {
        0 | 1 => Value::from(*rng.pick(&[
            0i64,
            1,
            2,
            3,
            4,
            12,
            36,
            -1,
            65_535,
            4_294_967_295,
            4_294_967_296,
            1 << 53,
            i64::MAX,
            i64::MIN,
        ])),
        2 => Value::from(u64::MAX),
        3 => Value::from(bin_len as u64 + rng.below(13) as u64),
        _ => Value::from(1.5),
    }
}

fn some_object<'a>(
    rng: &mut Rng,
    doc: &'a mut Value,
    key: &str,
) -> Option<&'a mut Map<String, Value>> {
    let items = doc.get_mut(key)?.as_array_mut()?;
    if items.is_empty() {
        return None;
    }
    let k = rng.below(items.len());
    items[k].as_object_mut()
}

/// One edit of the glb fields the reader indexes and sizes with, or of the
/// nodes and primitives it walks.
pub fn edit_glb_json(rng: &mut Rng, doc: &mut Value, bin_len: usize) {
    match rng.below(7) {
        0 => {
            // Every node shows mesh 0 (shared meshes).
            if let Some(nodes) = doc.get_mut("nodes").and_then(Value::as_array_mut) {
                for n in nodes.iter_mut().filter_map(Value::as_object_mut) {
                    n.insert("mesh".into(), Value::from(0));
                }
            }
        }
        1 => {
            // More nodes, copies of the first.
            if let Some(nodes) = doc.get_mut("nodes").and_then(Value::as_array_mut)
                && let Some(first) = nodes.first().cloned()
            {
                let n = rng.range(1, 64);
                nodes.extend(std::iter::repeat_n(first, n));
            }
        }
        2 => {
            // More primitives in a mesh, copies of its first.
            if let Some(mesh) = some_object(rng, doc, "meshes")
                && let Some(prims) = mesh.get_mut("primitives").and_then(Value::as_array_mut)
                && let Some(first) = prims.first().cloned()
            {
                let n = rng.range(1, 64);
                prims.extend(std::iter::repeat_n(first, n));
            }
        }
        3 => {
            let field = *rng.pick(&["count", "bufferView", "componentType", "type"]);
            let v = interesting_index(rng, bin_len);
            if let Some(acc) = some_object(rng, doc, "accessors") {
                acc.insert(field.into(), v);
            }
        }
        4 => {
            let field = *rng.pick(&["byteOffset", "byteLength", "buffer", "target"]);
            let v = interesting_index(rng, bin_len);
            if let Some(view) = some_object(rng, doc, "bufferViews") {
                view.insert(field.into(), v);
            }
        }
        5 => {
            let v = interesting_index(rng, bin_len);
            let field = *rng.pick(&["indices", "material", "mode", "POSITION", "NORMAL"]);
            if let Some(mesh) = some_object(rng, doc, "meshes")
                && let Some(prims) = mesh.get_mut("primitives").and_then(Value::as_array_mut)
                && !prims.is_empty()
            {
                let k = rng.below(prims.len());
                if let Some(prim) = prims[k].as_object_mut() {
                    if field.starts_with(char::is_uppercase) {
                        if let Some(attrs) =
                            prim.get_mut("attributes").and_then(Value::as_object_mut)
                        {
                            attrs.insert(field.into(), v);
                        }
                    } else {
                        prim.insert(field.into(), v);
                    }
                }
            }
        }
        _ => {
            let v = if rng.chance(50) {
                ("mesh", interesting_index(rng, bin_len))
            } else {
                ("name", Value::String(interesting_string(rng)))
            };
            if let Some(node) = some_object(rng, doc, "nodes") {
                node.insert(v.0.into(), v.1);
            }
        }
    }
}

/// Overwrites one 4-byte value of a BIN chunk: odd floats for positions and
/// normals, odd integers for indices.
pub fn edit_glb_bin(rng: &mut Rng, bin: &mut [u8]) {
    if bin.len() < 4 {
        return;
    }
    let at = rng.below(bin.len() / 4) * 4;
    let current = u32::from_le_bytes([bin[at], bin[at + 1], bin[at + 2], bin[at + 3]]);
    let v = match rng.below(3) {
        0 => rng
            .pick(&[
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                -0.0,
                f32::MIN_POSITIVE,
                f32::from_bits(1),
                f32::MAX,
                1.0e10,
                9.0e12,
                0.0005,
                1.0005,
            ])
            .to_bits(),
        1 => *rng.pick(&[0, 1, 2, 3, u32::MAX, u32::MAX - 1, 0x8000_0000, 0x7fff_ffff]),
        _ => current.wrapping_add(rng.range_i(-2, 2) as u32),
    };
    bin[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
