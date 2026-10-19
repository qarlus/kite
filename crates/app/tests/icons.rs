//! Every icon Kite names must be in the bundle it ships: one that is missing renders as an
//! empty square, which no other test would notice.
use gpui_kit::AssetSource as _;

/// `IconName::ReplyAll` -> `icons/reply-all.svg`, `Settings2` -> `icons/settings-2.svg`.
fn icon_path(name: &str) -> String {
    let mut kebab = String::new();
    let mut prev: Option<char> = None;
    for ch in name.chars() {
        // A new word starts at a capital, and a number is its own word (`Settings2`).
        let starts_word = ch.is_ascii_uppercase()
            || (ch.is_ascii_digit() && prev.is_some_and(|p| !p.is_ascii_digit()));
        if starts_word && prev.is_some() {
            kebab.push('-');
        }
        kebab.push(ch.to_ascii_lowercase());
        prev = Some(ch);
    }
    format!("icons/{kebab}.svg")
}

fn sources(dir: &std::path::Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(std::fs::read_to_string(&path).expect("read source"));
        }
    }
}

#[test]
fn every_icon_kite_uses_is_bundled() {
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut names: Vec<String> = files
        .iter()
        .flat_map(|source| source.split("IconName::").skip(1))
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect()
        })
        .filter(|name: &String| name.starts_with(|c: char| c.is_ascii_uppercase()))
        .collect();
    names.sort();
    names.dedup();
    assert!(
        names.len() > 20,
        "found too few icons ({names:?}); the scan is broken"
    );
    let missing: Vec<_> = names
        .iter()
        .filter(|name| {
            !matches!(kite::Assets.load(&icon_path(name)), Ok(Some(bytes)) if !bytes.is_empty())
        })
        .collect();
    assert!(missing.is_empty(), "these icons render blank: {missing:?}");
}
