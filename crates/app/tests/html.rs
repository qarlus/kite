use kite::html::render;

#[test]
fn renders_html_mail_and_resolves_links() {
    let r = render(
        r#"<h1>Q3 plan</h1><p>See <a href="https://example.com/doc">the doc</a> and reply.</p>"#,
        600.,
        1.,
        true,
    );
    assert!(r.height > 40., "content laid out: {}", r.height);
    // The link sits on the first line of the paragraph, below the heading.
    let hit = (0..r.height as i32)
        .step_by(2)
        .flat_map(|y| (0..300).step_by(4).map(move |x| (x as f32, y as f32)))
        .find_map(|(x, y)| r.link_at(x, y));
    assert_eq!(hit.as_deref(), Some("https://example.com/doc"));
    assert_eq!(r.link_at(590., 5.), None);
}

#[cfg(not(debug_assertions))]
#[test]
fn typical_email_renders_within_budget() {
    let body = "<p>Hi team,</p><p>Notes from today's sync below.</p><table><tr><td><b>Owner</b></td><td>Task</td></tr><tr><td>Ana</td><td>Ship v1</td></tr></table><blockquote>On Mon, Bob wrote: sounds good</blockquote>".repeat(4);
    render(&body, 700., 2., true); // warm fonts and Stylo
    let mut times: Vec<_> = (0..5)
        .map(|_| {
            let t = std::time::Instant::now();
            render(&body, 700., 2., true);
            t.elapsed()
        })
        .collect();
    times.sort();
    eprintln!("Median HTML render: {:?}", times[2]);
    assert!(
        times[2].as_millis() <= 15,
        "median Blitz render {:?}, budget 15 ms",
        times[2]
    );
}
