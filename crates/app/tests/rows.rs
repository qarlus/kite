//! The list row: which account it came from, and the hover actions that must never hide its text.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Bounds, Pixels, TestAppContext, px};
use kite_core::api::Mail as _;
use kite_core::model::{Action, Id};

fn overlaps(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    a.origin.x < b.right()
        && b.origin.x < a.right()
        && a.origin.y < b.bottom()
        && b.origin.y < a.bottom()
}

#[gpui_kit::test]
fn a_combined_row_marks_its_account_and_keeps_the_address_in_the_label(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // Two accounts share the list, so every row says which one it belongs to — as a mark,
        // not as an address, which is what the label and the tooltip are for.
        assert_eq!(
            window.find(("row-account", 0usize)).label(),
            Some("karl@proton.me")
        );
        assert_eq!(
            window.find(("row-account", 1usize)).label(),
            Some("karl@outlook.com")
        );
        let mark = window.find(("row-account", 0usize)).bounds();
        assert!(
            mark.size.width <= px(16.) && mark.size.height <= px(16.),
            "the account mark is an icon, not a text address: {mark:?}"
        );
        let time = window.find(("row-time", 0usize)).bounds();
        assert!(
            (mark.center().y - time.center().y).abs() <= px(1.),
            "the mark is centred with the date: {mark:?} vs {time:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_single_account_row_has_no_account_mark(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("row-account", 0usize)).is_none(),
            "one account on screen needs no mark; only a mixed inbox does"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn hover_actions_replace_the_meta_without_covering_or_moving_the_text(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            !window.find(("row-actions", 0usize)).visible(),
            "the actions stay out of the way until the pointer arrives"
        );
        let before: Vec<_> = ["row-sender", "row-subject", "row-snippet", "row-time"]
            .iter()
            .map(|id| window.find((*id, 0usize)).bounds())
            .collect();

        window.hover(("row", 0usize), cx);
        window.render_frame(cx);
        assert!(window.find(("row-actions", 0usize)).visible());
        let after: Vec<_> = ["row-sender", "row-subject", "row-snippet", "row-time"]
            .iter()
            .map(|id| window.find((*id, 0usize)).bounds())
            .collect();
        assert_eq!(before, after, "hovering must not move or reflow any text");

        let subject = window.find(("row-subject", 0usize)).bounds();
        let snippet = window.find(("row-snippet", 0usize)).bounds();
        for action in ["row-archive", "row-trash", "row-star", "row-read"] {
            let bounds = window.find((action, 0usize)).bounds();
            assert!(
                !overlaps(bounds, subject) && !overlaps(bounds, snippet),
                "{action} ({bounds:?}) covers the row's text: {subject:?} / {snippet:?}"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn hovering_the_account_mark_names_the_account(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(("row-account", 0usize), cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(
            window
                .try_find("row-account-tip")
                .and_then(|tip| tip.label().map(str::to_string)),
            Some("karl@proton.me".to_string())
        );
    })
    .unwrap();
}

/// Tags are a set, so a row shows all of them — not just the first — and the ones that do not fit
/// become a `+N` chip. Either way the row keeps its fixed height, which `uniform_list` depends on.
#[gpui_kit::test]
fn a_row_shows_every_tag_that_fits_and_counts_the_rest(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    tag(&fixture, 1, &["Team", "Urgent", "Work"]);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        for (chip, name) in ["Team", "Urgent", "Work"].into_iter().enumerate() {
            let expected = format!("Tag: {name}");
            assert_eq!(
                window.find(kite::list::row_tag_id(0, chip)).label(),
                Some(expected.as_str()),
                "every tag on the conversation paints, chip {chip}"
            );
        }
        assert!(
            window.try_find(("row-tag-more", 0usize)).is_none(),
            "three short tags all fit, so there is nothing left to count"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_row_with_more_tags_than_fit_counts_the_remainder(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let tags = [
        "Newsletter",
        "To Reply",
        "Waiting on Reply",
        "Accounting 2026",
        "Travel",
        "Receipts",
    ];
    tag(&fixture, 1, &tags);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let shown = (0..tags.len())
            .take_while(|chip| window.try_find(kite::list::row_tag_id(0, *chip)).is_some())
            .count();
        assert!(
            shown >= 2,
            "a row shows every tag that fits, not just the first: {shown} of {}",
            tags.len()
        );
        let more = window
            .find(("row-tag-more", 0usize))
            .label()
            .map(str::to_string);
        assert_eq!(
            more,
            Some(format!("+{}", tags.len() - shown)),
            "the tags that do not fit are counted"
        );
        assert_eq!(
            window.find(("row", 0usize)).bounds().size.height,
            window.find(("row", 1usize)).bounds().size.height,
            "tag chips never make a row taller than the list expects"
        );
        let subject = window.find(("row-subject", 0usize)).bounds();
        assert!(
            subject.size.width > px(0.),
            "the subject keeps its room beside the chips: {subject:?}"
        );
    })
    .unwrap();
}

/// Tags a conversation the way the tag menu does, through `Mail::act`.
fn tag(fixture: &std::sync::Arc<Fixture>, thread: Id, tags: &[&str]) {
    for name in tags {
        fixture
            .act(
                &[thread],
                &Action::Label {
                    tag: (*name).into(),
                },
            )
            .unwrap();
    }
}
