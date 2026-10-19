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

#[test]
fn a_blocked_body_keeps_its_links_and_loses_its_remote_images() {
    use std::collections::HashMap;
    let source = r#"<a href="https://example.com/doc">the doc</a><img src="https://cdn.example.com/a.png"><p style="background: url('https://cdn.example.com/b.png')">x</p>"#;
    assert_eq!(
        kite::html::remote_images(source),
        vec![
            "https://cdn.example.com/a.png".to_string(),
            "https://cdn.example.com/b.png".to_string()
        ]
    );
    let blocked = kite::html::inline_images(source, &HashMap::new());
    assert!(
        !blocked.contains("cdn.example.com"),
        "no remote reference survives a blocked body: {blocked}"
    );
    assert!(
        blocked.contains(r#"href="https://example.com/doc""#),
        "a link stays a link, whatever the image policy: {blocked}"
    );
    let mut images = HashMap::new();
    images.insert(
        "https://cdn.example.com/a.png".to_string(),
        "data:image/png;base64,AAAA".to_string(),
    );
    let loaded = kite::html::inline_images(source, &images);
    assert!(loaded.contains("data:image/png;base64,AAAA"));
    assert!(
        loaded.contains("data:image/gif;base64"),
        "an image that was not fetched is a transparent pixel: {loaded}"
    );
    assert!(
        kite::html::data_url(b"\x89PNG\r\n\x1a\n").starts_with("data:image/png;base64,"),
        "a fetched image becomes a data URL of its own type"
    );
}

#[test]
fn a_body_with_non_ascii_text_still_finds_and_replaces_its_images() {
    use std::collections::HashMap;
    // Turkish dotted capital I lowercases to two characters: a lowercased copy of the body
    // would shift every byte offset after it.
    let source = "İstanbul'dan selam — <IMG SRC='https://cdn.example.com/a.png'> ÄÖÜ";
    assert_eq!(
        kite::html::remote_images(source),
        vec!["https://cdn.example.com/a.png".to_string()]
    );
    let blocked = kite::html::inline_images(source, &HashMap::new());
    assert!(!blocked.contains("cdn.example.com"));
    assert!(
        blocked.starts_with("İstanbul'dan selam — "),
        "the prose is untouched: {blocked}"
    );
}
