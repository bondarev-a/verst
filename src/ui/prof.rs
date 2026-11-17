//! Замеры для ветки perf-measure. Включаются переменной VERST_PROF.
//! VERST_BENCH_FILES=a.md:b.md:… запускает автоматический прогон переключений.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gtk4::glib;
use gtk4::prelude::*;

use super::Ctx;

static MAIN_START: OnceLock<Instant> = OnceLock::new();

thread_local! {
    static ON: bool = std::env::var_os("VERST_PROF").is_some();
    static TOTALS: RefCell<BTreeMap<&'static str, (Duration, u32)>> = RefCell::new(BTreeMap::new());
    static STALL: RefCell<Stall> = RefCell::new(Stall::default());
    static FIRST_PAINT: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct Stall {
    last: Option<Instant>,
    blocked: Duration,
    max_gap: Duration,
}

pub fn enabled() -> bool {
    ON.with(|on| *on)
}

pub fn mark_main() {
    let _ = MAIN_START.set(Instant::now());
}

fn since_main_ms() -> f64 {
    MAIN_START
        .get()
        .map(|start| start.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(-1.0)
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub struct Span {
    name: &'static str,
    start: Option<Instant>,
}

pub fn span(name: &'static str) -> Span {
    Span {
        name,
        start: enabled().then(Instant::now),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let took = start.elapsed();
        TOTALS.with(|totals| {
            let mut totals = totals.borrow_mut();
            let slot = totals.entry(self.name).or_insert((Duration::ZERO, 0));
            slot.0 += took;
            slot.1 += 1;
        });
    }
}

fn take_totals() -> Vec<(&'static str, Duration, u32)> {
    let map = TOTALS.with(|totals| std::mem::take(&mut *totals.borrow_mut()));
    let mut list: Vec<_> = map.into_iter().map(|(k, (d, n))| (k, d, n)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    list
}

fn reset_stall() {
    STALL.with(|stall| {
        let mut stall = stall.borrow_mut();
        stall.blocked = Duration::ZERO;
        stall.max_gap = Duration::ZERO;
        stall.last = Some(Instant::now());
    });
}

fn read_stall() -> (Duration, Duration) {
    STALL.with(|stall| {
        let stall = stall.borrow();
        (stall.blocked, stall.max_gap)
    })
}

/// Таймер 5 мс. Если главный цикл занят, интервал между срабатываниями растёт.
fn start_heartbeat() {
    reset_stall();
    glib::timeout_add_local(Duration::from_millis(5), || {
        let now = Instant::now();
        STALL.with(|stall| {
            let mut stall = stall.borrow_mut();
            if let Some(last) = stall.last {
                let gap = now - last;
                if gap > Duration::from_millis(20) {
                    stall.blocked += gap - Duration::from_millis(5);
                }
                if gap > stall.max_gap {
                    stall.max_gap = gap;
                }
            }
            stall.last = Some(now);
        });
        glib::ControlFlow::Continue
    });
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn print_totals(prefix: &str, list: &[(&'static str, Duration, u32)]) {
    for (name, took, count) in list.iter().take(14) {
        if ms(*took) < 0.5 {
            continue;
        }
        eprintln!("{prefix}  {name:<28} {:>9.1} ms  x{count}", ms(*took));
    }
}

pub(super) fn after_activate(ctx: &Ctx) {
    if !enabled() {
        return;
    }
    if FIRST_PAINT.with(|done| done.replace(true)) {
        return;
    }
    eprintln!("PROF activate_done {:.1} ms since main", since_main_ms());
    start_heartbeat();
    if let Some(clock) = ctx.widgets.window.frame_clock() {
        hook_first_paint(&clock);
    } else {
        ctx.widgets.window.connect_realize(|window| {
            if let Some(clock) = window.frame_clock() {
                hook_first_paint(&clock);
            }
        });
    }
    let startup = take_totals();
    eprintln!("PROF startup_sync (до показа окна):");
    print_totals("PROF", &startup);
    let ctx = ctx.clone();
    // Первые 3 секунды после показа окна: сколько главный поток был занят.
    glib::timeout_add_local_once(Duration::from_millis(3000), move || {
        let (blocked, max_gap) = read_stall();
        eprintln!(
            "PROF startup_after_show 3s: blocked {:.1} ms, max_gap {:.1} ms",
            ms(blocked),
            ms(max_gap)
        );
        print_totals("PROF", &take_totals());
        start_bench(ctx);
    });
}

fn hook_first_paint(clock: &gtk4::gdk::FrameClock) {
    let handler: std::rc::Rc<RefCell<Option<glib::SignalHandlerId>>> = Default::default();
    let slot = handler.clone();
    let id = clock.connect_after_paint(move |clock| {
        eprintln!(
            "PROF first_paint {:.1} ms since main, epoch_ms {}",
            since_main_ms(),
            epoch_ms()
        );
        if let Some(id) = slot.borrow_mut().take() {
            clock.disconnect(id);
        }
    });
    *handler.borrow_mut() = Some(id);
}

enum Step {
    Go(std::path::PathBuf, usize),
    Type(usize),
    Report(String),
    Quit,
}

fn start_bench(ctx: Ctx) {
    let Some(raw) = std::env::var_os("VERST_BENCH_FILES") else {
        return;
    };
    let files: Vec<std::path::PathBuf> = std::env::split_paths(&raw).collect();
    let rounds: usize = std::env::var("VERST_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut steps = Vec::new();
    for round in 1..=rounds {
        for file in &files {
            steps.push(Step::Go(file.clone(), round));
        }
    }
    if let Some(typing) = std::env::var_os("VERST_BENCH_TYPE") {
        let path = std::path::PathBuf::from(typing);
        steps.push(Step::Go(path.clone(), 0));
        steps.push(Step::Type(20));
        steps.push(Step::Report("typing".to_string()));
    }
    steps.push(Step::Quit);
    steps.reverse();
    run_steps(ctx, steps);
}

fn settle_ms() -> u64 {
    std::env::var("VERST_BENCH_SETTLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1200)
}

fn run_steps(ctx: Ctx, mut steps: Vec<Step>) {
    let Some(step) = steps.pop() else {
        return;
    };
    match step {
        Step::Go(path, round) => {
            take_totals();
            reset_stall();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let t0 = Instant::now();
            ctx.go(path, None);
            let sync = t0.elapsed();
            glib::timeout_add_local_once(Duration::from_millis(settle_ms()), move || {
                let (blocked, max_gap) = read_stall();
                eprintln!(
                    "PROF switch round={round} -> {name:<14} sync {:>8.1} ms | blocked {:>8.1} ms | max_gap {:>8.1} ms",
                    ms(sync),
                    ms(blocked),
                    ms(max_gap)
                );
                print_totals("PROF   ", &take_totals());
                run_steps(ctx, steps);
            });
        }
        Step::Type(count) => {
            take_totals();
            reset_stall();
            type_chars(ctx, steps, count);
        }
        Step::Report(label) => {
            glib::timeout_add_local_once(Duration::from_millis(settle_ms()), move || {
                let (blocked, max_gap) = read_stall();
                eprintln!(
                    "PROF {label}: blocked {:>8.1} ms | max_gap {:>8.1} ms",
                    ms(blocked),
                    ms(max_gap)
                );
                print_totals("PROF   ", &take_totals());
                run_steps(ctx, steps);
            });
        }
        Step::Quit => {
            eprintln!("PROF done");
            ctx.save_session_now();
            std::process::exit(0);
        }
    }
}

/// Набор символов с интервалом 200 мс в середине документа, как обычная печать.
fn type_chars(ctx: Ctx, steps: Vec<Step>, left: usize) {
    if left == 0 {
        run_steps(ctx, steps);
        return;
    }
    let view = ctx.focused_view();
    let buffer = view.buffer();
    let line = buffer.line_count() / 2;
    if let Some(mut iter) = buffer.iter_at_line(line) {
        buffer.place_cursor(&iter);
        buffer.insert(&mut iter, "x");
    }
    glib::timeout_add_local_once(Duration::from_millis(200), move || {
        type_chars(ctx, steps, left - 1);
    });
}
