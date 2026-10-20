use kite::html::{Rendered, render};

/// The RGB of one pixel of a render made `width` device pixels wide.
fn pixel(r: &Rendered, width: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let data = r.image.as_bytes(0).expect("a rendered frame");
    let at = ((y * width + x) * 4) as usize;
    (data[at + 2], data[at + 1], data[at])
}

/// A pixel's contrast with the white paper, by DESIGN's own rule.
fn contrast_with_paper((r, g, b): (u8, u8, u8)) -> f32 {
    let hex = ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff;
    kite::theme::contrast(gpui_kit::rgba(hex), gpui_kit::rgba(0xffffffff))
}

/// Every pixel of the render, so a test can look at the ink a body actually painted.
fn pixels(r: &Rendered) -> impl Iterator<Item = (u8, u8, u8)> + '_ {
    let data = r.image.as_bytes(0).expect("a rendered frame");
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|px| (px[2], px[1], px[0]))
}

#[test]
fn renders_html_mail_and_resolves_links() {
    let r = render(
        r#"<h1>Q3 plan</h1><p>See <a href="https://example.com/doc">the doc</a> and reply.</p>"#,
        600.,
        1.,
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

#[test]
fn a_body_without_its_own_background_paints_on_white_paper() {
    // One render serves both appearances, so the body is the mail's own sheet: white with dark
    // ink, never the chrome's pane or panel colour and never light text on top of it.
    let r = render("<p>Hi team,</p><p>Notes from today's sync.</p>", 600., 2.);
    let paper = pixel(&r, 1200, 1180, 8);
    assert_eq!(paper, (255, 255, 255), "the body's canvas is paper");
    for theme in [kite::theme::pick(true), kite::theme::pick(false)] {
        for (name, chrome) in [("pane", theme.bg), ("panel", theme.bg2)] {
            let chrome = (
                (chrome.r * 255.).round() as u8,
                (chrome.g * 255.).round() as u8,
                (chrome.b * 255.).round() as u8,
            );
            assert_ne!(paper, chrome, "the body never paints the theme's {name}");
        }
    }
    let ink = pixels(&r).map(contrast_with_paper).fold(0.0_f32, f32::max);
    assert!(
        ink >= 4.5,
        "the default text is dark ink on the paper: {ink:.1}:1"
    );
}

#[test]
fn an_authors_own_colours_still_win_on_the_paper() {
    // The signature strip that showed as a white band on a dark pane: the author's background
    // must keep painting, because the paper under it is the same white.
    let r = render(
        r#"<div style="background:#eef3ff;color:#a02020">Karl Jones LLB (Hons), LLM.</div>"#,
        600.,
        2.,
    );
    let painted: Vec<_> = pixels(&r).collect();
    assert!(
        painted.contains(&(0xee, 0xf3, 0xff)),
        "the author's background paints"
    );
    assert!(
        painted
            .iter()
            .any(|&(red, green, blue)| red > 0x90 && green < 0x40 && blue < 0x40),
        "the author's own text colour paints"
    );
}

#[test]
fn links_read_on_the_paper_and_dark_scheme_mail_stays_on_it() {
    // The glyph core is the bluest pixel the link painted, and it must read on the paper: a
    // dark appearance's link accent is only 3.3:1 against white.
    let r = render(r#"<a href="https://example.com/doc">the doc</a>"#, 600., 2.);
    let link = pixels(&r)
        .max_by_key(|&(red, green, blue)| blue.saturating_sub(red.max(green)))
        .map(contrast_with_paper)
        .expect("the link painted pixels");
    assert!(link >= 4.5, "a link reads on the paper: {link:.2}:1");
    // Mail that asks for a dark scheme must not paint one: its canvas is paper.
    let r = render(
        "<style>@media (prefers-color-scheme: dark){html{background:#000;color:#fff}}</style><p>Hi</p>",
        600.,
        1.,
    );
    assert_eq!(
        pixel(&r, 600, 590, 4),
        (255, 255, 255),
        "prefers-color-scheme: dark mail still sits on the paper"
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn typical_email_renders_within_budget() {
    let body = "<p>Hi team,</p><p>Notes from today's sync below.</p><table><tr><td><b>Owner</b></td><td>Task</td></tr><tr><td>Ana</td><td>Ship v1</td></tr></table><blockquote>On Mon, Bob wrote: sounds good</blockquote>".repeat(4);
    render(&body, 700., 2.); // warm fonts and Stylo
    let mut times: Vec<_> = (0..5)
        .map(|_| {
            let t = std::time::Instant::now();
            render(&body, 700., 2.);
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
