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
        let renderer = ctx
            .widgets
            .window
            .renderer()
            .map(|r| r.type_().name().to_string())
            .unwrap_or_default();
        eprintln!(
            "PROF renderer {renderer} (GSK_RENDERER={})",
            std::env::var("GSK_RENDERER").unwrap_or_default()
        );
        print_totals("PROF", &take_totals());
        start_bench(ctx);
    });
}

fn hook_first_paint(clock: &gtk4::gdk::FrameClock) {
    let handler: std::rc::Rc<RefCell<Option<glib::SignalHandlerId>>> = Default::default();
    let slot = handler.clone();
    let id = clock.connect_after_paint(move |clock| {
        let (main_cpu, all_cpu, threads) = cpu_times_ms();
        eprintln!(
            "PROF first_paint {:.1} ms since main, epoch_ms {}, cpu_main {:.1} ms, cpu_all {:.1} ms, threads {}",
            since_main_ms(),
            epoch_ms(),
            main_cpu,
            all_cpu,
            threads
        );
        if std::env::var_os("VERST_EXIT_AFTER_PAINT").is_some() {
            std::process::exit(0);
        }
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
    Split(String),
    Scroll,
    CursorShot,
    CellShot,
    TableLeave(std::path::PathBuf),
    LinkEdit(std::path::PathBuf, u8),
    Action(String),
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
    if let Some(scroll) = std::env::var_os("VERST_BENCH_SCROLL") {
        for path in std::env::split_paths(&scroll) {
            steps.push(Step::Go(path, 0));
            steps.push(Step::Scroll);
            steps.push(Step::Report("scroll".to_string()));
        }
    }
    if let Ok(actions) = std::env::var("VERST_BENCH_ACTIONS") {
        for name in actions.split(',').filter(|n| !n.is_empty()) {
            steps.push(Step::Action(name.to_string()));
        }
    }
    if let Some(pair) = std::env::var_os("VERST_BENCH_LINKS") {
        let paths: Vec<_> = std::env::split_paths(&pair).collect();
        if paths.len() == 2 {
            steps.push(Step::Go(paths[0].clone(), 0));
            for stage in 0..3 {
                steps.push(Step::LinkEdit(paths[1].clone(), stage));
            }
        }
    }
    if let Some(pair) = std::env::var_os("VERST_BENCH_TABLE_LEAVE") {
        let paths: Vec<_> = std::env::split_paths(&pair).collect();
        if paths.len() == 2 {
            steps.push(Step::Go(paths[0].clone(), 0));
            steps.push(Step::TableLeave(paths[1].clone()));
        }
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_CELL") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::CellShot);
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_CURSOR") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::CursorShot);
    }
    if let Some(split) = std::env::var_os("VERST_BENCH_SPLIT") {
        let path = std::path::PathBuf::from(split);
        steps.push(Step::Go(path, 0));
        steps.push(Step::Split("split-on".to_string()));
        steps.push(Step::Split("split-off".to_string()));
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
                eprintln!("PROF lists {}", ctx.lists_report());
                if std::env::var_os("VERST_MARKUP").is_some() {
                    eprintln!("PROF markup {}", ctx.markup_report());
                }
                if round == 1 {
                    save_shot(&ctx, &format!("{name}"));
                }
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
        Step::Scroll => {
            take_totals();
            reset_stall();
            scroll_stress(ctx, steps, 0, 0);
        }
        Step::Action(name) => {
            let before = ctx.focused_path();
            if let Some(app) = ctx.widgets.window.application() {
                gtk4::gio::prelude::ActionGroupExt::activate_action(&app, &name, None);
            }
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                let label = |p: Option<std::path::PathBuf>| {
                    p.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                        .unwrap_or_default()
                };
                eprintln!(
                    "PROF action {name}: {} -> {} | mode {} | toggle {} | lists {}",
                    label(before),
                    label(ctx.focused_path()),
                    ctx.state.borrow().view_mode.as_str(),
                    ctx.widgets.view_toggle.icon_name().unwrap_or_default(),
                    ctx.lists_report()
                );
                run_steps(ctx, steps);
            });
        }
        Step::LinkEdit(probe, stage) => {
            let name = probe
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let marker = format!("ПРОБА [[{name}]] ");
            let buffer = ctx.focused_view().buffer();
            match stage {
                1 => {
                    if let Some(mut iter) = buffer.iter_at_line(2) {
                        buffer.insert(&mut iter, &marker);
                    }
                }
                2 => {
                    if let Some(mut a) = buffer.iter_at_line(2) {
                        let mut b = a.clone();
                        b.forward_chars(marker.chars().count() as i32);
                        if buffer.text(&a, &b, true) == marker {
                            buffer.delete(&mut a, &mut b);
                        }
                    }
                }
                _ => {}
            }
            glib::timeout_add_local_once(Duration::from_millis(900), move || {
                eprintln!("PROF links stage {stage}: {}", ctx.links_report(&probe));
                run_steps(ctx, steps);
            });
        }
        Step::TableLeave(next) => {
            let view = ctx.focused_view();
            let buffer = view.buffer();
            let mut line = 0;
            while line < buffer.line_count() {
                let Some(start) = buffer.iter_at_line(line) else { break };
                let mut end = start.clone();
                end.forward_to_line_end();
                if buffer.text(&start, &end, true).starts_with("| ") {
                    break;
                }
                line += 1;
            }
            if let Some(iter) = buffer.iter_at_line(line + 3) {
                buffer.place_cursor(&iter);
            }
            let typed = ctx.bench_type_in_cell("МЕТКА_ЯЧЕЙКИ");
            let from = ctx.focused_path();
            ctx.go(next.clone(), None);
            glib::timeout_add_local_once(Duration::from_millis(1200), move || {
                let editable = ctx.focused_view().is_editable();
                let here = ctx.focused_path().and_then(|p| ctx.buffer_text(&p)).unwrap_or_default();
                let there = from.as_ref().and_then(|p| ctx.buffer_text(p)).unwrap_or_default();
                eprintln!(
                    "PROF table_leave: typed {typed}, cell_active_after {}, editable {editable}, mark_in_old {}, mark_in_new {}",
                    ctx.any_table_cell_focused(),
                    there.contains("МЕТКА_ЯЧЕЙКИ"),
                    here.contains("МЕТКА_ЯЧЕЙКИ")
                );
                run_steps(ctx, steps);
            });
        }
        Step::CellShot => {
            save_shot(&ctx, "cell-before");
            let view = ctx.focused_view();
            let buffer = view.buffer();
            let mut line = 0;
            while line < buffer.line_count() {
                let Some(start) = buffer.iter_at_line(line) else { break };
                let mut end = start.clone();
                end.forward_to_line_end();
                if buffer.text(&start, &end, true).starts_with("| ") {
                    break;
                }
                line += 1;
            }
            let ctx2 = ctx.clone();
            glib::timeout_add_local_once(Duration::from_millis(800), move || {
                // Строка данных таблицы: курсор в ней активирует ячейку.
                if let Some(iter) = buffer.iter_at_line(line + 2) {
                    buffer.place_cursor(&iter);
                }
                glib::timeout_add_local_once(Duration::from_millis(800), move || {
                    eprintln!(
                        "PROF cell active {:?}, editable {}",
                        ctx2.focused_table_cell(),
                        ctx2.focused_view().is_editable()
                    );
                    save_shot(&ctx2, "cell-editing");
                    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                        run_steps(ctx2, steps)
                    });
                });
            });
        }
        Step::CursorShot => {
            let view = ctx.focused_view();
            let buffer = view.buffer();
            let mut line = buffer.line_count() / 2;
            while line < buffer.line_count() {
                let Some(start) = buffer.iter_at_line(line) else { break };
                let mut end = start.clone();
                end.forward_to_line_end();
                let text = buffer.text(&start, &end, true);
                if text.starts_with("- **") {
                    break;
                }
                line += 1;
            }
            if let Some(mut iter) = buffer.iter_at_line(line) {
                buffer.place_cursor(&iter);
                view.scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.5);
            }
            glib::timeout_add_local_once(Duration::from_millis(800), move || {
                eprintln!("PROF cursor line {line}");
                save_shot(&ctx, "cursor");
                glib::timeout_add_local_once(Duration::from_millis(1500), move || run_steps(ctx, steps));
            });
        }
        Step::Split(label) => {
            take_totals();
            reset_stall();
            let t0 = Instant::now();
            ctx.toggle_split();
            let sync = t0.elapsed();
            glib::timeout_add_local_once(Duration::from_millis(settle_ms()), move || {
                let (blocked, max_gap) = read_stall();
                eprintln!(
                    "PROF {label}: sync {:>8.1} ms | blocked {:>8.1} ms | max_gap {:>8.1} ms",
                    ms(sync),
                    ms(blocked),
                    ms(max_gap)
                );
                print_totals("PROF   ", &take_totals());
                let (tables, second, mapped) = ctx.table_grid_report();
                eprintln!("PROF {label}: tables {tables}, pane2 grids {second}, mapped {mapped}");
                save_shot(&ctx, &label);
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

/// Снимок окна в PNG, если задан VERST_SHOT_DIR. Пока окно не разложено, повторяет.
fn save_shot(ctx: &Ctx, name: &str) {
    if std::env::var_os("VERST_SHOT_DIR").is_none() {
        return;
    }
    try_shot(ctx.clone(), name.to_string(), 0);
}

fn try_shot(ctx: Ctx, name: String, attempt: u32) {
    if shoot(&ctx, &name) || attempt >= 10 {
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(100), move || {
        try_shot(ctx, name, attempt + 1);
    });
}

fn shoot(ctx: &Ctx, name: &str) -> bool {
    let Some(dir) = std::env::var_os("VERST_SHOT_DIR") else {
        return true;
    };
    let window = &ctx.widgets.window;
    // Всё окно, вместе с заголовком: VERST_SHOT_ROOT=1 снимает только содержимое.
    let target: gtk4::Widget = if std::env::var_os("VERST_SHOT_ROOT").is_some() {
        let Some(root) = window.child() else {
            return false;
        };
        root
    } else {
        window.clone().upcast()
    };
    let width = target.width();
    let height = target.height();
    if width <= 0 || height <= 0 {
        return false;
    }
    let paintable = gtk4::WidgetPaintable::new(Some(&target));
    let snapshot = gtk4::Snapshot::new();
    gtk4::gdk::prelude::PaintableExt::snapshot(&paintable, &snapshot, f64::from(width), f64::from(height));
    let Some(node) = snapshot.to_node() else {
        return false;
    };
    let Some(renderer) = window.renderer() else {
        return false;
    };
    let texture = renderer.render_texture(&node, None);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    match texture.save_to_png(&path) {
        Ok(()) => {
            eprintln!("PROF shot {}", path.display());
            true
        }
        Err(error) => {
            eprintln!("PROF shot failed: {error}");
            false
        }
    }
}

/// Прокрутка вниз и вверх с курсором на каждой строке-шаге и правкой текста:
/// служебные символы на строке курсора то показываются, то прячутся.
fn scroll_stress(ctx: Ctx, steps: Vec<Step>, tick: u32, line: i32) {
    let view = ctx.focused_view();
    let buffer = view.buffer();
    let total = buffer.line_count();
    let (next, done) = if tick < 400 {
        let down = (tick / 100) % 2 == 0;
        let step = if down { 29 } else { -23 };
        let mut next = line + step;
        if next >= total {
            next = total - 1;
        }
        if next < 0 {
            next = 0;
        }
        (next, false)
    } else {
        (line, true)
    };
    if done {
        run_steps(ctx, steps);
        return;
    }
    if let Some(mut iter) = buffer.iter_at_line(next) {
        buffer.place_cursor(&iter);
        view.scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.3);
        if tick % 17 == 0 {
            buffer.insert(&mut iter, "z");
            let mut a = buffer.iter_at_mark(&buffer.get_insert());
            let b = a.clone();
            a.backward_char();
            let (mut a, mut b) = (a, b);
            buffer.delete(&mut a, &mut b);
        }
    }
    glib::timeout_add_local_once(Duration::from_millis(12), move || {
        scroll_stress(ctx, steps, tick + 1, next);
    });
}

/// Время на процессоре: главный поток и все потоки процесса, в мс (/proc/.../schedstat).
fn cpu_times_ms() -> (f64, f64, usize) {
    let read = |path: &std::path::Path| -> f64 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()))
            .map(|ns| ns / 1e6)
            .unwrap_or(0.0)
    };
    let main = read(std::path::Path::new("/proc/thread-self/schedstat"));
    let mut all = 0.0;
    let mut threads = 0;
    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
        for entry in dir.flatten() {
            all += read(&entry.path().join("schedstat"));
            threads += 1;
        }
    }
    (main, all, threads)
}
