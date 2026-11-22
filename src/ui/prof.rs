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
    Resize(i32),
    Drag(i32, i32),
    Caret(usize, usize),
    /// Вставить метку в текст и сразу закрыть окно, как Ctrl+Q.
    TypeAndClose,
    /// Изменить открытый файл на диске другой программой; `true` — в буфере
    /// перед этим есть несохранённая правка.
    ExternalWrite(bool),
    /// Вставить строку в первую таблицу, затем отменить (Ctrl+Z).
    TableRowUndo,
    /// Правка, переход в другой файл и обратно, затем Ctrl+Z.
    UndoAfterSwitch(std::path::PathBuf, u8),
    /// Правка ячейки, выход из неё, затем Ctrl+Z.
    CellUndo,
    /// Открыть текущий файл и во второй панели, доложить о сетках таблиц.
    SplitSame(std::path::PathBuf),
    /// Снимок окна через 1,5 с (страница WebKit успевает загрузиться).
    Shot(String),
    /// Фокус уходит в поле поиска и возвращается в редактор (как Esc из
    /// ячейки или закрытие всплывающего окна).
    FocusBounce,
    /// Выделить весь текст и прокрутить редактор на 30 шагов.
    SelectScroll(u32),
    /// Вставить несколько строк разом (как Ctrl+V) и проверить оформление
    /// вставленных строк вне строки курсора.
    PasteCheck,
    Quit,
}

thread_local! {
    static UNDO_ORIGINAL: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
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
    let shots = std::env::var_os("VERST_BENCH_SHOT_EACH").is_some();
    for round in 1..=rounds {
        for file in &files {
            steps.push(Step::Go(file.clone(), round));
            if shots {
                let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                steps.push(Step::Shot(name));
            }
        }
    }
    if let Some(scroll) = std::env::var_os("VERST_BENCH_SCROLL") {
        for path in std::env::split_paths(&scroll) {
            steps.push(Step::Go(path, 0));
            steps.push(Step::Scroll);
            steps.push(Step::Report("scroll".to_string()));
        }
    }
    if let Ok(widths) = std::env::var("VERST_BENCH_RESIZE") {
        for item in widths.split(',') {
            let item = item.trim();
            // drag:<ширина навигации>:<ширина блока справа> — как перетаскивание.
            if let Some(rest) = item.strip_prefix("drag:") {
                let mut parts = rest.split(':').filter_map(|p| p.parse().ok());
                if let (Some(nav), Some(info)) = (parts.next(), parts.next()) {
                    steps.push(Step::Drag(nav, info));
                }
            } else if let Ok(w) = item.parse() {
                steps.push(Step::Resize(w));
            }
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
    if let Some(file) = std::env::var_os("VERST_BENCH_CARET") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        for (row, col) in [(0, 0), (1, 0), (1, 1), (2, 3)] {
            steps.push(Step::Caret(row, col));
        }
        // Масштаб меняется, когда курсоры уже созданы.
        for _ in 0..3 {
            steps.push(Step::Action("text_zoom_in".to_string()));
        }
        steps.push(Step::Caret(1, 1));
        steps.push(Step::Caret(0, 0));
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_TYPE_CLOSE") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::TypeAndClose);
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_EXTERNAL") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::ExternalWrite(false));
        steps.push(Step::ExternalWrite(true));
    }
    if let Some(pair) = std::env::var_os("VERST_BENCH_UNDO_SWITCH") {
        let paths: Vec<_> = std::env::split_paths(&pair).collect();
        if paths.len() == 2 {
            steps.push(Step::Go(paths[0].clone(), 0));
            steps.push(Step::UndoAfterSwitch(paths[0].clone(), 0));
            steps.push(Step::Go(paths[1].clone(), 0));
            steps.push(Step::Go(paths[0].clone(), 0));
            steps.push(Step::UndoAfterSwitch(paths[0].clone(), 1));
        }
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_PASTE") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::PasteCheck);
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_SELECT_SCROLL") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::SelectScroll(30));
        steps.push(Step::Report("select_scroll".to_string()));
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_FOCUS") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        for _ in 0..3 {
            steps.push(Step::FocusBounce);
            steps.push(Step::Report("focus_bounce".to_string()));
        }
    }
    if let Some(pair) = std::env::var_os("VERST_BENCH_SPLIT_OTHER") {
        let paths: Vec<_> = std::env::split_paths(&pair).collect();
        if paths.len() == 2 {
            steps.push(Step::Go(paths[0].clone(), 0));
            steps.push(Step::Action("split".to_string()));
            steps.push(Step::SplitSame(paths[1].clone()));
            // Обратно в первую панель: её таблицы должны вернуться.
            steps.push(Step::Go(paths[0].clone(), 0));
            steps.push(Step::Report("back_to_first".to_string()));
        }
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_SPLIT_SAME") {
        let file = std::path::PathBuf::from(file);
        steps.push(Step::Go(file.clone(), 0));
        steps.push(Step::Action("split".to_string()));
        steps.push(Step::SplitSame(file));
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_CELL_UNDO") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::CellUndo);
    }
    if let Some(file) = std::env::var_os("VERST_BENCH_ROW_UNDO") {
        steps.push(Step::Go(std::path::PathBuf::from(file), 0));
        steps.push(Step::TableRowUndo);
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
        let count = std::env::var("VERST_BENCH_TYPE_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        for _ in 0..std::env::var("VERST_BENCH_TYPE_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1usize) {
            steps.push(Step::Type(count));
            steps.push(Step::Report("typing".to_string()));
        }
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
                if std::env::var_os("VERST_MEM").is_some() {
                    eprintln!("PROF mem {}", mem_report());
                }
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
                if std::env::var_os("VERST_MEM").is_some() {
                    eprintln!("PROF mem {}", mem_report());
                }
                print_totals("PROF   ", &take_totals());
                run_steps(ctx, steps);
            });
        }
        Step::Scroll => {
            take_totals();
            reset_stall();
            scroll_stress(ctx, steps, 0, 0);
        }
        Step::Caret(row, col) => {
            ctx.bench_caret_in_cell(0, row, col);
            glib::timeout_add_local_once(Duration::from_millis(700), move || {
                eprintln!("PROF caret {}", ctx.bench_caret_report());
                save_shot(&ctx, &format!("caret-{row}-{col}"));
                glib::timeout_add_local_once(Duration::from_millis(500), move || run_steps(ctx, steps));
            });
        }
        Step::Drag(nav, info) => {
            // Как перетаскивание разделителя мышью: без dock_adjust.
            ctx.widgets.main_paned.set_position(nav);
            let right = ctx.widgets.right_paned.clone();
            right.set_position(right.width() - info);
            glib::timeout_add_local_once(Duration::from_millis(700), move || {
                let (saved_nav, saved_info) = {
                    let session = &ctx.state.borrow().session;
                    (session.nav_width, session.info_width)
                };
                eprintln!(
                    "PROF drag to nav {nav} info {info}: nav {} | info {} | saved nav {saved_nav} info {saved_info}",
                    ctx.widgets.main_paned.position(),
                    ctx.widgets.right_paned.width() - ctx.widgets.right_paned.position()
                );
                run_steps(ctx, steps);
            });
        }
        Step::Resize(width) => {
            let window = ctx.widgets.window.clone();
            let height = window.height().max(600);
            window.set_default_size(width, height);
            glib::timeout_add_local_once(Duration::from_millis(900), move || {
                let main = &ctx.widgets.main_paned;
                let right = &ctx.widgets.right_paned;
                let (saved_nav, saved_info) = {
                    let session = &ctx.state.borrow().session;
                    (session.nav_width, session.info_width)
                };
                eprintln!(
                    "PROF resize want {width}: window {} | nav {} | center {} | info {} | saved nav {} info {}",
                    ctx.widgets.window.width(),
                    main.position(),
                    right.position(),
                    right.width() - right.position(),
                    saved_nav,
                    saved_info
                );
                run_steps(ctx, steps);
            });
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
                save_shot(&ctx, &format!("action-{name}-{}", ctx.state.borrow().view_mode.as_str()));
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
                let disk = from
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                eprintln!(
                    "PROF table_leave: typed {typed}, cell_active_after {}, editable {editable}, mark_in_old {}, mark_in_new {}, mark_on_disk {}",
                    ctx.any_table_cell_focused(),
                    there.contains("МЕТКА_ЯЧЕЙКИ"),
                    here.contains("МЕТКА_ЯЧЕЙКИ"),
                    disk.contains("МЕТКА_ЯЧЕЙКИ")
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
                let same = ctx.widgets.views[0].buffer() == ctx.widgets.views[1].buffer();
                let pane2 = ctx.state.borrow().session.pane_paths.get(1).cloned().flatten();
                eprintln!("PROF {label}: tables {tables}, pane2 grids {second}, mapped {mapped}, pane2_same_buffer {same}, pane2_file {pane2:?}");
                save_shot(&ctx, &label);
                run_steps(ctx, steps);
            });
        }
        Step::ExternalWrite(edited) => {
            let Some(path) = ctx.focused_path() else {
                run_steps(ctx, steps);
                return;
            };
            if edited {
                ctx.focused_view().buffer().insert_at_cursor("МОЯ_ПРАВКА ");
            }
            let tables_before = ctx.live_table_count();
            let mut disk = std::fs::read_to_string(&path).unwrap_or_default();
            disk.push_str("\nВНЕШНЯЯ_ПРАВКА\n");
            std::fs::write(&path, &disk).unwrap();
            glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                let text = ctx.buffer_text(&path).unwrap_or_default();
                let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
                let (dialog, dirty) = {
                    let state = ctx.state.borrow();
                    (
                        state.conflict.is_some(),
                        state.buffers.get(&path).is_some_and(|e| e.dirty),
                    )
                };
                eprintln!(
                    "PROF external edited={edited}: dialog {dialog}, dirty {dirty}, buffer_has_external {}, disk_has_external {}, disk_has_mine {}, tables {} -> {}",
                    text.contains("ВНЕШНЯЯ_ПРАВКА"),
                    on_disk.contains("ВНЕШНЯЯ_ПРАВКА"),
                    on_disk.contains("МОЯ_ПРАВКА"),
                    tables_before,
                    ctx.live_table_count()
                );
                run_steps(ctx, steps);
            });
        }
        Step::UndoAfterSwitch(path, stage) => {
            let buffer = ctx.focused_view().buffer();
            if stage == 0 {
                let original = ctx.buffer_text(&path).unwrap_or_default();
                UNDO_ORIGINAL.with(|o| *o.borrow_mut() = original);
                // Правка в конце файла, после всех таблиц.
                let mut end = buffer.end_iter();
                buffer.begin_user_action();
                buffer.insert(&mut end, "\nМЕТКА_ОТМЕНЫ\n");
                buffer.end_user_action();
                eprintln!("PROF undo_switch typed, can_undo {}", buffer.can_undo());
                glib::timeout_add_local_once(Duration::from_millis(900), move || run_steps(ctx, steps));
                return;
            }
            glib::timeout_add_local_once(Duration::from_millis(900), move || {
                let can = buffer.can_undo();
                for _ in 0..4 {
                    if !buffer.can_undo() {
                        break;
                    }
                    buffer.undo();
                    if !ctx.buffer_text(&path).unwrap_or_default().contains("МЕТКА_ОТМЕНЫ") {
                        break;
                    }
                }
                glib::timeout_add_local_once(Duration::from_millis(900), move || {
                    let after = ctx.buffer_text(&path).unwrap_or_default();
                    let original = UNDO_ORIGINAL.with(|o| o.borrow().clone());
                    let slice = buffer.slice(&buffer.start_iter(), &buffer.end_iter(), true);
                    let literal = slice.matches('\u{FFFC}').count() as i64 - ctx.live_table_count() as i64;
                    let diff = original
                        .chars()
                        .zip(after.chars())
                        .position(|(a, b)| a != b);
                    eprintln!(
                        "PROF undo_switch: can_undo {can}, mark_left {}, equal_original {}, len {} -> {}, first_diff {:?}, extra_fffc {literal}",
                        after.contains("МЕТКА_ОТМЕНЫ"),
                        after == original,
                        original.chars().count(),
                        after.chars().count(),
                        diff
                    );
                    run_steps(ctx, steps);
                });
            });
        }
        Step::PasteCheck => {
            let buffer = ctx.focused_view().buffer();
            let mut at = buffer.iter_at_line(1).unwrap_or_else(|| buffer.start_iter());
            buffer.place_cursor(&at);
            buffer.begin_user_action();
            buffer.insert(
                &mut at,
                "\n# Вставленный заголовок\n\nтекст **вставленный жирный** конец\n\nпоследняя строка вставки\n",
            );
            buffer.end_user_action();
            glib::timeout_add_local_once(Duration::from_millis(1200), move || {
                let report = |needle: &str| -> String {
                    let mut line = 0;
                    while line < buffer.line_count() {
                        let Some(start) = buffer.iter_at_line(line) else { break };
                        let mut end = start;
                        end.forward_to_line_end();
                        let text = buffer.text(&start, &end, true);
                        if let Some(col) = text.find(needle) {
                            let chars = text[..col].chars().count() as i32;
                            let mut iter = start;
                            iter.forward_chars(chars + 1);
                            let names: Vec<String> = iter
                                .tags()
                                .iter()
                                .filter_map(|t| t.name().map(|n| n.to_string()))
                                .filter(|n| n.starts_with("tw-"))
                                .collect();
                            let mut first = start;
                            let hidden_first = first
                                .tags()
                                .iter()
                                .any(|t| t.name().as_deref() == Some("tw-hide"));
                            first.forward_char();
                            return format!("line {line} tags {names:?} first_hidden {hidden_first}");
                        }
                        line += 1;
                    }
                    "не найдено".into()
                };
                let cursor = buffer.iter_at_mark(&buffer.get_insert()).line();
                eprintln!(
                    "PROF paste: cursor line {cursor}; heading: {}; bold: {}; old heading: {}",
                    report("Вставленный заголовок"),
                    report("вставленный жирный"),
                    report("Раздел 1:")
                );
                run_steps(ctx, steps);
            });
        }
        Step::SelectScroll(left) => {
            let view = ctx.focused_view();
            if left == 30 {
                take_totals();
                reset_stall();
                let buffer = view.buffer();
                buffer.select_range(&buffer.start_iter(), &buffer.end_iter());
            }
            if left == 0 {
                run_steps(ctx, steps);
                return;
            }
            if let Some(adj) = view.vadjustment() {
                adj.set_value(adj.value() + 400.0);
            }
            glib::timeout_add_local_once(Duration::from_millis(30), move || {
                let mut steps = steps;
                steps.push(Step::SelectScroll(left - 1));
                run_steps(ctx, steps);
            });
        }
        Step::FocusBounce => {
            take_totals();
            reset_stall();
            ctx.widgets.search_entry.grab_focus();
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                ctx.focused_view().grab_focus();
                run_steps(ctx, steps);
            });
        }
        Step::Shot(name) => {
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                save_shot(&ctx, &name);
                glib::timeout_add_local_once(Duration::from_millis(500), move || run_steps(ctx, steps));
            });
        }
        Step::SplitSame(file) => {
            ctx.widgets.views[1].grab_focus();
            ctx.go(file, None);
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                let (tables, second, mapped) = ctx.table_grid_report();
                let first = ctx.first_pane_grid_count();
                let same = ctx.widgets.views[0].buffer() == ctx.widgets.views[1].buffer();
                eprintln!("PROF split_same: same_buffer {same}, tables {tables}, pane1 grids {first}, pane2 grids {second}, mapped {mapped}");
                run_steps(ctx, steps);
            });
        }
        Step::CellUndo => {
            let Some(path) = ctx.focused_path() else {
                run_steps(ctx, steps);
                return;
            };
            let view = ctx.focused_view();
            let buffer = view.buffer();
            let before = ctx.buffer_text(&path).unwrap_or_default();
            let mut line = 0;
            while line < buffer.line_count() {
                let Some(start) = buffer.iter_at_line(line) else { break };
                let mut end = start;
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
            ctx.deactivate_table_cells();
            let edited = ctx.buffer_text(&path).unwrap_or_default().contains("МЕТКА_ЯЧЕЙКИ");
            glib::timeout_add_local_once(Duration::from_millis(900), move || {
                let can = buffer.can_undo();
                buffer.undo();
                glib::timeout_add_local_once(Duration::from_millis(900), move || {
                    let after = ctx.buffer_text(&path).unwrap_or_default();
                    let can_redo = buffer.can_redo();
                    buffer.redo();
                    let redone = ctx.buffer_text(&path).unwrap_or_default();
                    eprintln!(
                        "PROF cell_undo: typed {typed}, edited {edited}, can_undo {can}, restored {}, redo {can_redo} brings_back {}, tables live {}",
                        after == before,
                        redone.contains("МЕТКА_ЯЧЕЙКИ"),
                        ctx.live_table_count()
                    );
                    run_steps(ctx, steps);
                });
            });
        }
        Step::TableRowUndo => {
            let Some(path) = ctx.focused_path() else {
                run_steps(ctx, steps);
                return;
            };
            let buffer = ctx.focused_view().buffer();
            let before = ctx.buffer_text(&path).unwrap_or_default();
            let rows = |text: &str| {
                text_workspace::tables::parse_tables(text)
                    .first()
                    .map_or(0, |t| t.row_count())
            };
            buffer.set_enable_undo(true);
            let undo_before = buffer.can_undo();
            ctx.table_add_row(path.clone(), 0);
            let added = ctx.buffer_text(&path).unwrap_or_default();
            let can_undo = buffer.can_undo();
            glib::timeout_add_local_once(Duration::from_millis(700), move || {
                // Как пользователь: Esc из ячейки, затем Ctrl+Z в тексте.
                let cell_was_active = ctx.any_table_cell_focused();
                ctx.deactivate_table_cells();
                eprintln!("PROF row_undo cell_was_active {cell_was_active}");
                buffer.undo();
                let buffer = buffer.clone();
                let added = added.clone();
                glib::timeout_add_local_once(Duration::from_millis(900), move || {
                    let after = ctx.buffer_text(&path).unwrap_or_default();
                    let literal = {
                        let mut iter = buffer.start_iter();
                        let mut n = 0;
                        loop {
                            if iter.char() == '\u{FFFC}' && iter.child_anchor().is_none() {
                                n += 1;
                            }
                            if !iter.forward_char() {
                                break;
                            }
                        }
                        n
                    };
                    eprintln!("PROF row_undo literal_fffc {literal}");
                    if after != before {
                        let at = before.chars().zip(after.chars()).position(|(a, b)| a != b).unwrap_or(0);
                        let b: String = before.chars().skip(at.saturating_sub(20)).take(60).collect();
                        let a: String = after.chars().skip(at.saturating_sub(20)).take(60).collect();
                        eprintln!("PROF row_undo diff at {at}: before {b:?} after {a:?}");
                        let m: String = added.chars().skip(at.saturating_sub(20)).take(120).collect();
                        eprintln!("PROF row_undo added near: {m:?}");
                        let at2 = before.chars().rev().zip(after.chars().rev()).position(|(a, b)| a != b).unwrap_or(0);
                        eprintln!("PROF row_undo tail diff from end {at2}, lens {} {} {}", before.chars().count(), added.chars().count(), after.chars().count());
                    }
                    eprintln!(
                        "PROF row_undo: can_undo before {undo_before}, after add {can_undo}, rows {} -> {} -> {}, restored {}, tables live {}",
                        rows(&before),
                        rows(&added),
                        rows(&after),
                        after == before,
                        ctx.live_table_count()
                    );
                    run_steps(ctx, steps);
                });
            });
        }
        Step::TypeAndClose => {
            let buffer = ctx.focused_view().buffer();
            buffer.insert_at_cursor("МЕТКА_ВЫХОДА ");
            eprintln!("PROF type_and_close: inserted, closing");
            ctx.widgets.window.close();
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

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// Анонимная память процесса (куча и прочее) до и после возврата свободного malloc.
fn mem_report() -> String {
    let anon = || -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("RssAnon:"))
                    .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
            })
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    };
    let before = anon();
    if std::env::var_os("VERST_MEM_TRIM").is_some() {
        unsafe {
            malloc_trim(0);
        }
    }
    let after = anon();
    format!("rss_anon {before:.1} MB, after trim {after:.1} MB")
}

/// Для замеров: решение по переходу в предпросмотре.
pub(super) fn note_nav(uri: &str, clicked: bool, allowed: bool) {
    if std::env::var_os("VERST_PROF").is_some() {
        let short: String = uri.chars().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
        eprintln!("PROF nav clicked={clicked} allowed={allowed} {short}");
    }
}
