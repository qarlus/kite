use kite::html::{Rendered, render};

/// One pixel of a render, premultiplied BGRA as GPUI's sprites read it.
fn pixel(r: &Rendered, x: u32, y: u32) -> [u8; 4] {
    let width = r.image.size(0).width.0 as u32;
    let data = r.image.as_bytes(0).expect("a rendered frame");
    let at = ((y * width + x) * 4) as usize;
    let [b, g, red, a] = [data[at], data[at + 1], data[at + 2], data[at + 3]];
    [red, g, b, a]
}

fn size(r: &Rendered) -> (u32, u32) {
    let size = r.image.size(0);
    (size.width.0 as u32, size.height.0 as u32)
}

/// Every pixel of the render, so a test can look at the ink a body actually painted.
fn pixels(r: &Rendered) -> impl Iterator<Item = [u8; 4]> + '_ {
    let data = r.image.as_bytes(0).expect("a rendered frame");
    data.as_chunks::<4>().0.iter().map(|px| {
        let [b, g, red, a] = [px[0], px[1], px[2], px[3]];
        [red, g, b, a]
    })
}

fn bytes(color: gpui_kit::Rgba) -> [u8; 4] {
    let byte = |v: f32| (v * 255.).round() as u8;
    [byte(color.r), byte(color.g), byte(color.b), 255]
}

/// How far a painted pixel is from a theme colour, channel by channel.
fn distance(pixel: [u8; 4], color: gpui_kit::Rgba) -> u8 {
    let want = bytes(color);
    (0..3)
        .map(|channel| want[channel].abs_diff(pixel[channel]))
        .max()
        .unwrap_or(0)
}

/// The closest an opaque pixel got to `color`, which is the colour a glyph core paints.
fn closest(r: &Rendered, color: gpui_kit::Rgba) -> u8 {
    pixels(r)
        .filter(|px| px[3] > 200)
        .map(|px| distance(px, color))
        .min()
        .unwrap_or(u8::MAX)
}

/// Relative luminance, as WCAG defines it: how light a painted pixel reads.
fn luminance([red, green, blue, _]: [u8; 4]) -> f32 {
    let linear = |v: u8| {
        let v = v as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn is_red([red, green, blue, alpha]: [u8; 4]) -> bool {
    alpha > 200 && red > 0xc0 && green < 0x40 && blue < 0x40
}

#[test]
fn renders_html_mail_and_resolves_links() {
    let r = render(
        r##"<h1>Q3 plan</h1><p>See <a href="https://example.com/doc">the doc</a> and reply.</p>"##,
        600.,
        1.,
        true,
    );
    assert!(r.height > 40., "content laid out: {}", r.height);
    // The link sits in the paragraph, below the heading.
    let hit = (0..r.height as i32)
        .step_by(2)
        .flat_map(|y| (0..300).step_by(4).map(move |x| (x as f32, y as f32)))
        .find_map(|(x, y)| r.link_at(x, y));
    assert_eq!(hit.as_deref(), Some("https://example.com/doc"));
    assert_eq!(r.link_at(590., 5.), None);
}

#[test]
fn a_dark_pane_turns_a_white_table_into_its_own_canvas() {
    // An Outlook reply quoting a newsletter is one white table with black ink: on a dark pane
    // it must merge into the pane instead of cutting a white box out of it.
    let html = r##"<table width="400" bgcolor="#ffffff"><tr><td style="background:#fff;color:#000">Hi</td></tr></table>"##;
    let theme = kite::theme::pick(true);
    let r = render(html, 600., 2., true);
    let pane = bytes(theme.bg);
    let painted: Vec<_> = pixels(&r).collect();
    let on_pane = painted.iter().filter(|px| **px == pane).count();
    assert!(
        on_pane * 10 > painted.len() * 9,
        "the table's white background is exactly the pane ({on_pane} of {} pixels)",
        painted.len()
    );
    assert!(
        painted.iter().all(|px| *px != [0xff, 0xff, 0xff, 0xff]),
        "no white is left anywhere in the body"
    );
    // The author's black ink became the pane's light ink.
    let ink = closest(&r, theme.fg);
    assert!(
        ink <= 8,
        "the text is the pane's own ink, not the author's black: off by {ink}"
    );
    let brightest = painted
        .iter()
        .filter(|px| px[3] > 200)
        .map(|px| luminance(*px))
        .fold(0., f32::max);
    assert!(
        brightest > 0.6,
        "the text reads light on a dark pane: {brightest:.2}"
    );
}

#[test]
fn a_light_grey_card_stays_a_card_but_sits_in_the_dark() {
    // Newsletters stack cards on a near-white page. Both must become dark, but the card has to
    // stay distinct from the pane or the layout is lost.
    let theme = kite::theme::pick(true);
    let r = render(
        r##"<div style="height:40px;background:#f5f5f5"></div>"##,
        600.,
        2.,
        true,
    );
    let card = pixel(&r, 40, 20);
    assert!(
        distance(card, theme.bg) >= 3,
        "the card is distinct from the pane: {card:?} against {:?}",
        bytes(theme.bg)
    );
    assert!(
        card.iter().take(3).all(|channel| *channel < 64),
        "and it is dark, not the author's near-white: {card:?}"
    );
    assert!(
        luminance(card) < 0.25,
        "the card reads as a dark surface: {:.3}",
        luminance(card)
    );
}

#[test]
fn mail_that_brings_its_own_dark_styles_is_left_alone() {
    // A mail that declares a dark appearance has already decided what its colours mean.
    let html = r##"<meta name="color-scheme" content="light dark"><table width="400" bgcolor="#ffffff"><tr><td style="color:#000">Hi</td></tr></table>"##;
    let r = render(html, 600., 2., true);
    assert_eq!(
        pixel(&r, 20, 10),
        [0xff, 0xff, 0xff, 0xff],
        "its own white background is its own business"
    );
    let darkest = pixels(&r)
        .filter(|px| px[3] > 200)
        .map(luminance)
        .fold(f32::MAX, f32::min);
    assert!(
        darkest < 0.2,
        "and its black ink is still black: {darkest:.2}"
    );
}

#[test]
fn a_light_appearance_keeps_the_authors_white_page() {
    // Light themes change nothing: the table is white and its ink is black, as authored.
    let theme = kite::theme::pick(false);
    let html = r##"<table width="400" bgcolor="#ffffff"><tr><td style="background:#fff;color:#000">Hi</td></tr></table>"##;
    let r = render(html, 600., 2., false);
    let (width, _) = size(&r);
    assert_eq!(
        pixel(&r, 20, 10),
        [0xff, 0xff, 0xff, 0xff],
        "the author's white background paints"
    );
    assert_eq!(
        pixel(&r, width - 2, 2),
        bytes(theme.bg),
        "and the page behind it is the appearance's own"
    );
}

#[test]
fn an_unbroken_line_is_fitted_instead_of_clipped() {
    // A long URL or an unbroken word is wider than the column: fit to width instead of cutting
    // the end off at the pane's right edge. The link sits at the end of the line, so if it is
    // inside the image the whole line is.
    let word = "Supercalifragilisticexpialidocious".repeat(6);
    let html = format!(
        r##"<p style="white-space:nowrap">{word}<a href="https://example.com/end">end</a></p>"##
    );
    let r = render(&html, 600., 2., true);
    let (width, _) = size(&r);
    assert!(
        r.width <= 600. && r.width > 580.,
        "the page is displayed at the column's width: {}",
        r.width
    );
    assert_eq!(width, 1200, "the bitmap is the column at the window's DPI");
    let hit = (0..r.height.ceil() as u32)
        .step_by(2)
        .flat_map(|y| (0..r.width.ceil() as u32).step_by(2).map(move |x| (x, y)))
        .find(|&(x, y)| r.link_at(x as f32, y as f32).is_some())
        .expect("the end of the line painted inside the page, not past its edge");
    assert!(
        hit.0 as f32 > r.width * 0.7,
        "the end of the unbroken line is at the page's right edge: {hit:?} of {}",
        r.width
    );
}

#[test]
fn a_wide_rule_is_fitted_instead_of_clipped() {
    // A fixed-width `<hr>` or `<div>` is the other way real mail overflows the column.
    let r = render(
        r##"<hr style="width:900px;height:6px;background:#ff0000;border:0"><p>Hi</p>"##,
        600.,
        2.,
        true,
    );
    let (width, height) = size(&r);
    assert_eq!(width, 1200, "the bitmap is the column at the window's DPI");
    let end = (0..height)
        .flat_map(|y| (width - 8..width).map(move |x| (x, y)))
        .any(|(x, y)| is_red(pixel(&r, x, y)));
    assert!(end, "the rule's far end paints inside the image");
}

#[test]
fn a_page_wider_than_the_column_fits_instead_of_clipping() {
    // A 640px newsletter table in a 400px column: fitted to the column as one page, so the text
    // that used to be cut off mid-word at the right edge is all there.
    let html = r##"<table width="640" cellspacing="0" cellpadding="0"><tr><td>Weekend reads, and a look at what is coming next week.</td><td width="180" align="right"><a href="https://example.com/end">Read the last one</a></td><td width="20" style="background:#ff0000">&nbsp;</td></tr></table>"##;
    let r = render(html, 400., 2., true);
    assert!(
        r.width <= 400. && r.width > 380.,
        "the page is displayed at the column's width: {}",
        r.width
    );
    let (width, height) = size(&r);
    assert_eq!(width, 800, "the bitmap is the column at the window's DPI");
    // The table's last cell is red: it must paint inside the right edge of the image.
    let red = (0..height)
        .flat_map(|y| (width - 8..width).map(move |x| (x, y)))
        .any(|(x, y)| is_red(pixel(&r, x, y)));
    assert!(red, "the rightmost cell of the table is inside the image");
    // And it is the whole page, scaled down, not the same page re-wrapped into 400px.
    let whole = render(html, 700., 2., true);
    assert_eq!(whole.width, 700., "the page fits a wide column as it is");
    let (fitted, whole) = (r.height / r.width, whole.height / whole.width);
    assert!(
        (fitted - whole).abs() < 0.05,
        "the fitted page keeps the page's own proportions: {fitted:.2} vs {whole:.2}"
    );
    // The link beside it is still reachable, in the fitted coordinates the pane hit-tests in.
    let hit = (0..r.height.ceil() as u32)
        .step_by(2)
        .flat_map(|y| (0..r.width.ceil() as u32).step_by(2).map(move |x| (x, y)))
        .find(|&(x, y)| r.link_at(x as f32, y as f32).is_some())
        .expect("the link painted, so it can be clicked");
    assert!(
        hit.0 as f32 > r.width * 0.7 && (hit.0 as f32) < r.width,
        "the link near the right edge is inside the fitted page: {hit:?} of {}",
        r.width
    );
}

#[test]
fn an_unstyled_body_reads_in_the_panes_own_ink_and_link_colour() {
    // Nothing authored: the page is the pane, the text its ink, and a link the theme's accent.
    let theme = kite::theme::pick(true);
    let r = render(
        r##"<p>Hi</p><a href="https://example.com/doc">the doc</a>"##,
        600.,
        2.,
        true,
    );
    let (width, _) = size(&r);
    assert_eq!(
        pixel(&r, width - 4, 2),
        bytes(theme.bg),
        "the page is the pane's own colour"
    );
    assert!(
        closest(&r, theme.fg) <= 8,
        "unstyled text is the pane's ink"
    );
    assert!(
        closest(&r, theme.accent) <= 8,
        "an unstyled link is the accent"
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn typical_email_renders_within_budget() {
    let prose = "<p>Hi team,</p><p>Notes from today's sync below.</p><blockquote>On Mon, Bob wrote: sounds good</blockquote>".repeat(4);
    let newsletter = r##"<table width="600" cellpadding="0" cellspacing="0" bgcolor="#ffffff"><tr><td style="background:#f5f5f5;color:#222222;font-family:Arial,sans-serif"><h1 style="color:#1a0dab">Weekend reads</h1><p style="color:#555555">The week in twelve links, and what is coming next.</p><hr style="border-color:#dddddd"><a href="https://example.com/one" style="color:#0b57d0">One</a> <a href="https://example.com/two" style="color:#0b57d0">Two</a><table width="560"><tr><td style="background:#ffffff;border:1px solid #eeeeee;color:#333333">A card with a border</td><td style="background:#fafafa;color:#666666">And another</td></tr></table></td></tr></table>"##.repeat(3);
    render(&prose, 700., 2., true); // warm fonts and Stylo
    for (kind, body) in [
        ("prose", prose.as_str()),
        ("newsletter", newsletter.as_str()),
    ] {
        let mut times: Vec<_> = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                render(body, 700., 2., true);
                t.elapsed()
            })
            .collect();
        times.sort();
        eprintln!("Median {kind} HTML render: {:?}", times[2]);
        assert!(
            times[2].as_millis() <= 15,
            "median {kind} Blitz render {:?}, budget 15 ms",
            times[2]
        );
    }
}

#[test]
fn a_blocked_body_keeps_its_links_and_loses_its_remote_images() {
    use std::collections::HashMap;
    let source = r##"<a href="https://example.com/doc">the doc</a><img src="https://cdn.example.com/a.png"><p style="background: url('https://cdn.example.com/b.png')">x</p>"##;
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
        blocked.contains(r##"href="https://example.com/doc""##),
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
