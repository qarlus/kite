//! Performance budgets (AGENTS.md). Asserted in release only, on a 10k-thread fixture.
#![cfg(not(debug_assertions))]
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use std::time::{Duration, Instant};

const THREADS: usize = 10_000;

#[gpui_kit::test]
fn first_frame_of_a_ten_thousand_thread_inbox_is_fast(cx: &mut TestAppContext) {
    let fixture = Fixture::new(THREADS);
    let (window, _) = mount(cx, fixture);
    let started = Instant::now();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let elapsed = started.elapsed();
    eprintln!("10k inbox first frame: {elapsed:?}");
    assert!(
        elapsed <= Duration::from_millis(50),
        "first frame took {elapsed:?}, budget 50 ms"
    );
}

#[gpui_kit::test]
fn j_repaints_within_four_milliseconds(cx: &mut TestAppContext) {
    let fixture = Fixture::new(THREADS);
    let (window, kite) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let mut times = Vec::with_capacity(20);
    for _ in 0..20 {
        let started = Instant::now();
        cx.simulate_keystrokes(window.into(), "j");
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        times.push(started.elapsed());
    }
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 20)
    });
    times.sort();
    eprintln!("Median j repaint: {:?}", times[10]);
    assert!(
        times[10] <= Duration::from_millis(4),
        "median j frame {:?}, budget 4 ms",
        times[10]
    );
}

#[gpui_kit::test]
fn only_visible_rows_are_laid_out(cx: &mut TestAppContext) {
    let fixture = Fixture::new(THREADS);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("row", 0usize)).is_some());
        assert!(
            window.try_find(("row", 500usize)).is_none(),
            "rows far below the viewport are never built"
        );
    })
    .unwrap();
}
